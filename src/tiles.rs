//! Region tiles and 3D surfaces on demand: served from the database while the region file's
//! size and modification time are unchanged, otherwise read from Wings and made again, a few at
//! a time panel-wide and once per region even when several viewers ask together.
use crate::{
    db::{self, Tile},
    map::{Map, RegionFile, not_found},
    render, surface,
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
/// Bumped when the 3D surface format or extraction changes, so cached surfaces are made again.
pub const SURFACE_VERSION: u32 = 1;
/// Render slots; a render takes `PERMITS / render_concurrency` of them.
const PERMITS: u32 = 16;

static RENDERING: Semaphore = Semaphore::const_new(PERMITS as usize);

/// What is made of a region file. Part of the in-flight key, so a region's tile and surface
/// never wait for each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Product {
    Tile,
    Surface,
}

type Key = (Uuid, String, Product);
type Locks = Mutex<HashMap<Key, Arc<tokio::sync::Mutex<()>>>>;
static IN_FLIGHT: LazyLock<Locks> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a tile depends on besides the region file.
pub fn variant(ceiling: Option<i32>) -> String {
    match ceiling {
        Some(ceiling) => format!("v{RENDER_VERSION}:ceiling={ceiling}"),
        None => format!("v{RENDER_VERSION}"),
    }
}

/// What a 3D surface depends on besides the region file; never equal to a tile's [`variant`].
pub fn surface_variant(ceiling: Option<i32>) -> String {
    match ceiling {
        Some(ceiling) => format!("s{SURFACE_VERSION}:ceiling={ceiling}"),
        None => format!("s{SURFACE_VERSION}"),
    }
}

/// A panel-wide work slot for reading and processing one region file, shared by tile rendering,
/// 3D surfaces, overlays and chunk tool jobs: `concurrency` regions at a time (the render
/// setting).
pub async fn slot(concurrency: u32) -> Result<tokio::sync::SemaphorePermit<'static>, ApiResponse> {
    RENDERING
        .acquire_many(PERMITS / concurrency.clamp(1, PERMITS))
        .await
        .map_err(|_| ApiResponse::error("the renderer is shutting down"))
}

/// Whether a cached entry made from a file of `modified` and `size` with `cached` (its variant)
/// is still what `file` and `variant` give.
fn current((modified, size, cached): (i64, i64, &str), file: &RegionFile, variant: &str) -> bool {
    modified == file.modified && size == file.size as i64 && cached == variant
}

/// Runs `make` while holding the lock of `key`, so concurrent requests for the same product of
/// a region wait for the first one instead of all reading the file.
async fn exclusively<T>(
    key: Key,
    make: impl Future<Output = Result<T, ApiResponse>>,
) -> Result<T, ApiResponse> {
    let lock = IN_FLIGHT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entry(key.clone())
        .or_default()
        .clone();
    let made = {
        let _region = lock.lock().await;
        make.await
    };
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .remove(&key);
    made
}

/// Reads the region file at `path` in a work slot and hands it to `work` off the async
/// runtime. Fails with 422 when nothing in it can be read because it is older than 1.13.
async fn process<T: Send + 'static>(
    map: &Map,
    path: &str,
    work: impl FnOnce(Vec<u8>) -> Result<(T, render::Stats), String> + Send + 'static,
) -> Result<T, ApiResponse> {
    let _slot = slot(map.settings.render_concurrency).await?;
    let data = map
        .read(path, map.region_limit())
        .await?
        .ok_or_else(|| not_found("region not found"))?;
    let (output, stats) = tokio::task::spawn_blocking(move || work(data))
        .await
        .map_err(|_| ApiResponse::error("rendering the region failed"))?
        .map_err(|err| {
            ApiResponse::error(format!("the region file could not be read: {err}"))
                .with_status(StatusCode::UNPROCESSABLE_ENTITY)
        })?;
    tracing::debug!(
        path,
        "read {} chunks ({} unfinished, {} too old, {} unreadable, {} external)",
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
    Ok(output)
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
        && current((tile.modified, tile.size, tile.variant.as_str()), &file, &variant)
    {
        db::touch_tile(&state.database, map.server, path).await?;
        return Ok(tile.image);
    }
    let key = (map.server, path.to_string(), Product::Tile);
    exclusively(key, render_tile(state, map, path, file, ceiling, variant)).await
}

async fn render_tile(
    state: &State,
    map: &Map,
    path: &str,
    file: RegionFile,
    ceiling: Option<i32>,
    variant: String,
) -> Result<Vec<u8>, ApiResponse> {
    // someone else may have rendered it while this request waited
    if let Some(tile) = db::tile(&state.database, map.server, path).await?
        && current((tile.modified, tile.size, tile.variant.as_str()), &file, &variant)
    {
        return Ok(tile.image);
    }
    let image = process(map, path, move |data| {
        let image = render::region(&data, ceiling)?;
        let size = render::SIZE as u32;
        Ok((render::png(&image.rgba, size, size)?, image.stats))
    })
    .await?;
    let tile = Tile {
        modified: file.modified,
        size: file.size as i64,
        variant,
        image,
    };
    db::store_tile(&state.database, map.server, path, &tile).await?;
    Ok(tile.image)
}

/// The zlib-compressed 3D surface of the region file at `path` (listed as `file`), see
/// [`surface`](crate::surface).
pub async fn surface(
    state: &State,
    map: &Map,
    path: &str,
    file: RegionFile,
    ceiling: Option<i32>,
) -> Result<Vec<u8>, ApiResponse> {
    let variant = surface_variant(ceiling);
    if let Some(cached) = db::surface(&state.database, map.server, path).await?
        && current((cached.modified, cached.size, cached.variant.as_str()), &file, &variant)
    {
        db::touch_surface(&state.database, map.server, path).await?;
        return Ok(cached.data);
    }
    let key = (map.server, path.to_string(), Product::Surface);
    exclusively(key, make_surface(state, map, path, file, ceiling, variant)).await
}

async fn make_surface(
    state: &State,
    map: &Map,
    path: &str,
    file: RegionFile,
    ceiling: Option<i32>,
    variant: String,
) -> Result<Vec<u8>, ApiResponse> {
    // someone else may have made it while this request waited
    if let Some(cached) = db::surface(&state.database, map.server, path).await?
        && current((cached.modified, cached.size, cached.variant.as_str()), &file, &variant)
    {
        return Ok(cached.data);
    }
    let encoded = process(map, path, move |data| {
        let body = surface::region(&data, ceiling)?;
        Ok((surface::encode(&body.bytes)?, body.stats))
    })
    .await?;
    let cached = db::Surface {
        modified: file.modified,
        size: file.size as i64,
        variant,
        data: encoded,
    };
    db::store_surface(&state.database, map.server, path, &cached).await?;
    Ok(cached.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: RegionFile = RegionFile {
        x: 0,
        z: 0,
        size: 8192,
        modified: 100,
    };

    #[test]
    fn tiles_depend_on_the_file_and_the_renderer() {
        let file = FILE;
        let tile = Tile {
            modified: 100,
            size: 8192,
            variant: variant(None),
            image: Vec::new(),
        };
        let stamp = (tile.modified, tile.size, tile.variant.as_str());
        assert!(current(stamp, &file, &variant(None)));
        assert!(!current(stamp, &file, &variant(Some(100))));
        assert!(!current(stamp, &RegionFile { modified: 101, ..file }, &variant(None)));
        assert!(!current(stamp, &RegionFile { size: 12288, ..file }, &variant(None)));
        assert_ne!(variant(Some(100)), variant(Some(90)));
    }

    #[test]
    fn surfaces_never_pass_for_tiles() {
        for ceiling in [None, Some(100), Some(90)] {
            assert_ne!(surface_variant(ceiling), variant(ceiling));
            let tile = variant(ceiling);
            let surface = surface_variant(ceiling);
            // a row of one cache never counts as current for the other
            assert!(!current((100, 8192, tile.as_str()), &FILE, &surface));
            assert!(!current((100, 8192, surface.as_str()), &FILE, &tile));
            assert!(current((100, 8192, surface.as_str()), &FILE, &surface));
        }
        assert_ne!(surface_variant(Some(100)), surface_variant(Some(90)));
        assert_ne!(surface_variant(Some(100)), surface_variant(None));
        // the same region's tile and surface are made under different locks
        let path = "world/region/r.0.0.mca".to_string();
        let server = Uuid::nil();
        assert_ne!(
            (server, path.clone(), Product::Tile),
            (server, path, Product::Surface)
        );
    }
}
