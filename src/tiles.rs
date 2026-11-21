//! Region tiles on demand: served from the database while the region file's size and
//! modification time are unchanged, otherwise read from Wings and rendered, a few at a time
//! panel-wide and once per region even when several viewers ask together.
use crate::{
    db::{self, Tile},
    map::{Map, RegionFile, not_found},
    render,
};
use axum::http::StatusCode;
use shared::{State, response::ApiResponse};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Bumped when rendering changes, so cached tiles are drawn again.
pub const RENDER_VERSION: u32 = 2;
/// Render slots; a render takes `PERMITS / render_concurrency` of them.
const PERMITS: u32 = 16;

static RENDERING: Semaphore = Semaphore::const_new(PERMITS as usize);
type Locks = Mutex<HashMap<(Uuid, String), Arc<tokio::sync::Mutex<()>>>>;
static IN_FLIGHT: LazyLock<Locks> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a tile depends on besides the region file.
pub fn variant(ceiling: Option<i32>) -> String {
    match ceiling {
        Some(ceiling) => format!("v{RENDER_VERSION}:ceiling={ceiling}"),
        None => format!("v{RENDER_VERSION}"),
    }
}

fn current(tile: &Tile, file: &RegionFile, variant: &str) -> bool {
    tile.modified == file.modified && tile.size == file.size as i64 && tile.variant == variant
}

/// The PNG of the region file at `path` (listed as `file`).
pub async fn png(
    state: &State,
    map: &Map,
    path: &str,
    file: RegionFile,
    ceiling: Option<i32>,
) -> Result<Vec<u8>, ApiResponse> {
    let variant = variant(ceiling);
    if let Some(tile) = db::tile(&state.database, map.server, path).await?
        && current(&tile, &file, &variant)
    {
        db::touch_tile(&state.database, map.server, path).await?;
        return Ok(tile.image);
    }

    let key = (map.server, path.to_string());
    let lock = IN_FLIGHT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entry(key.clone())
        .or_default()
        .clone();
    let rendered: Result<Vec<u8>, ApiResponse> = async {
        let _region = lock.lock().await;
        // someone else may have rendered it while this request waited
        if let Some(tile) = db::tile(&state.database, map.server, path).await?
            && current(&tile, &file, &variant)
        {
            return Ok(tile.image);
        }
        let share = PERMITS / map.settings.render_concurrency.clamp(1, PERMITS);
        let _slot = RENDERING
            .acquire_many(share)
            .await
            .map_err(|_| ApiResponse::error("the renderer is shutting down"))?;
        let data = map
            .read(path, map.region_limit())
            .await?
            .ok_or_else(|| not_found("region not found"))?;
        let (image, stats) = tokio::task::spawn_blocking(move || {
            let image = render::region(&data, ceiling)?;
            let size = render::SIZE as u32;
            Ok::<_, String>((render::png(&image.rgba, size, size)?, image.stats))
        })
        .await
        .map_err(|_| ApiResponse::error("rendering the region failed"))?
        .map_err(|err| {
            ApiResponse::error(format!("the region file could not be read: {err}"))
                .with_status(StatusCode::UNPROCESSABLE_ENTITY)
        })?;
        tracing::debug!(
            path,
            "rendered {} chunks ({} unfinished, {} too old, {} unreadable, {} external)",
            stats.rendered,
            stats.unfinished,
            stats.unsupported,
            stats.failed,
            stats.external
        );
        if stats.rendered == 0 && stats.unsupported > 0 {
            return Err(ApiResponse::error(
                "this region was saved by a Minecraft version older than 1.13, which the map cannot draw",
            )
            .with_status(StatusCode::UNPROCESSABLE_ENTITY));
        }
        let tile = Tile {
            modified: file.modified,
            size: file.size as i64,
            variant,
            image,
        };
        db::store_tile(&state.database, map.server, path, &tile).await?;
        Ok(tile.image)
    }
    .await;
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .remove(&key);
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_depend_on_the_file_and_the_renderer() {
        let file = RegionFile {
            x: 0,
            z: 0,
            size: 8192,
            modified: 100,
        };
        let tile = Tile {
            modified: 100,
            size: 8192,
            variant: variant(None),
            image: Vec::new(),
        };
        assert!(current(&tile, &file, &variant(None)));
        assert!(!current(&tile, &file, &variant(Some(100))));
        assert!(!current(&tile, &RegionFile { modified: 101, ..file }, &variant(None)));
        assert!(!current(&tile, &RegionFile { size: 12288, ..file }, &variant(None)));
        assert_ne!(variant(Some(100)), variant(Some(90)));
    }
}
