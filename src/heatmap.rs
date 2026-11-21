//! The player heatmap: while a server runs, the game saves every online player's file (every 5
//! minutes by default, and on logout). Each sample reads the player files saved since the last
//! one and counts one sample for the chunk each of those players stood in. Only servers whose
//! map was opened are sampled; samples are kept per chunk and day for the retention period.
use crate::{
    access::Power,
    coords, db,
    map::{Map, SMALL_FILE},
    players,
    settings::{self, Settings},
};
use chrono::{DateTime, Utc};
use shared::{
    State,
    models::{ByUuid, server::Server},
    response::ApiResponse,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};
use uuid::Uuid;

/// Player files read per server and sample, to bound one sample's work.
const MAX_FILES: usize = 500;
/// Seconds between prunes of old samples and the tile cache.
const PRUNE_EVERY: i64 = 3600;
static LAST_PRUNE: AtomicI64 = AtomicI64::new(0);

fn failed(what: &'static str) -> impl FnOnce(ApiResponse) -> anyhow::Error {
    move |_| anyhow::anyhow!("world map heatmap: could not {what}")
}

/// One loop of the background task: sample, prune, then wait for the next sample.
pub async fn run(state: &State) -> Result<(), anyhow::Error> {
    let settings = settings::load(state).await?;
    if settings.heatmap_enabled {
        let now = Utc::now();
        for (server, sampled) in db::tracked(&state.database).await? {
            if let Err(err) = sample(state, &settings, server, sampled, now).await {
                tracing::debug!(%server, "{err:#}");
            }
        }
    }
    let now = Utc::now().timestamp();
    if now - LAST_PRUNE.load(Ordering::Relaxed) >= PRUNE_EVERY {
        LAST_PRUNE.store(now, Ordering::Relaxed);
        prune(state, &settings).await?;
    }
    tokio::time::sleep(Duration::from_secs(
        u64::from(settings.heatmap_interval_minutes.max(1)) * 60,
    ))
    .await;
    Ok(())
}

async fn prune(state: &State, settings: &Settings) -> Result<(), anyhow::Error> {
    let keep_from = Utc::now().date_naive()
        - chrono::Days::new(u64::from(settings.heatmap_retention_days.max(1)));
    let samples = db::prune_heat(&state.database, keep_from).await?;
    let tiles = db::prune_tiles(&state.database, i64::from(settings.tile_cache_mb) << 20).await?;
    if samples > 0 || tiles > 0 {
        tracing::info!("world map: pruned {samples} old heatmap rows and {tiles} cached tiles");
    }
    Ok(())
}

/// The chunks of `(world, dimension, x, z)` position samples, grouped by world and dimension.
pub fn group(
    samples: impl IntoIterator<Item = (String, String, f64, f64)>,
) -> HashMap<(String, String), Vec<(i32, i32)>> {
    let mut groups: HashMap<(String, String), Vec<(i32, i32)>> = HashMap::new();
    for (world, dimension, x, z) in samples {
        groups
            .entry((world, dimension))
            .or_default()
            .push((coords::chunk_of_block(x), coords::chunk_of_block(z)));
    }
    groups
}

async fn sample(
    state: &State,
    settings: &Settings,
    server: Uuid,
    sampled: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), anyhow::Error> {
    // deleted servers lose their row by cascade
    let Ok(server) = Server::by_uuid(&state.database, server).await else {
        return Ok(());
    };
    if server.suspended || !settings.allows_egg(server.egg.uuid) {
        return Ok(());
    }
    let map = Map::open(state, &server)
        .await
        .map_err(failed("reach the server"))?;
    let power = map
        .wings
        .power_state()
        .await
        .map_err(failed("read the power state"))?;
    let since = sampled
        .unwrap_or(now - chrono::Duration::minutes(i64::from(settings.heatmap_interval_minutes)))
        .timestamp();
    if Power::from(power) != Power::Running {
        db::set_sampled(&state.database, server.uuid, now).await?;
        return Ok(());
    }
    let layout = map.layout(false).await.map_err(failed("read the worlds"))?;
    let Some(folder) = layout.worlds.first().and_then(|world| world.players.clone()) else {
        db::set_sampled(&state.database, server.uuid, now).await?;
        return Ok(());
    };
    let mut changed: Vec<_> = map
        .wings
        .list(&format!("/{folder}"))
        .await
        .map_err(failed("list the player files"))?
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            !entry.directory
                && players::file_uuid(&entry.name).is_some()
                && entry.modified.timestamp() > since
                && entry.modified.timestamp() <= now.timestamp()
        })
        .collect();
    changed.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
    changed.truncate(MAX_FILES);

    let mut samples = Vec::new();
    for entry in changed {
        let Ok(Some(bytes)) = map
            .read(&format!("{folder}/{}", entry.name), SMALL_FILE)
            .await
        else {
            continue;
        };
        let Ok(position) = players::position(&bytes) else {
            continue;
        };
        if let Some((world, dimension)) = layout.locate(position.world, &position.dimension) {
            samples.push((
                world.name.clone(),
                dimension.id.clone(),
                position.x,
                position.z,
            ));
        }
    }
    let day = now.date_naive();
    for ((world, dimension), chunks) in group(samples) {
        db::add_heat(&state.database, server.uuid, &world, &dimension, day, &chunks).await?;
    }
    db::set_sampled(&state.database, server.uuid, now).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_counted_per_chunk_and_dimension() {
        let groups = group([
            ("world".into(), "minecraft:overworld".into(), 10.0, -1.0),
            ("world".into(), "minecraft:overworld".into(), 15.9, -16.0),
            ("world".into(), "minecraft:the_nether".into(), -17.0, 40.0),
        ]);
        assert_eq!(
            groups[&("world".into(), "minecraft:overworld".into())],
            vec![(0, -1), (0, -1)]
        );
        assert_eq!(
            groups[&("world".into(), "minecraft:the_nether".into())],
            vec![(-2, 2)]
        );
    }
}
