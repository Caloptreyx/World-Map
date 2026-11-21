//! The map API, mounted at `/api/client/servers/{server}/world-map`. The panel resolves the
//! server and the user's permissions before these handlers run; each handler checks its
//! [`Action`] first.
use crate::{
    access::{self, Action, Power},
    anvil, cache::Memo,
    coords::{self, ChunkRect},
    db, entities,
    map::{Map, SMALL_FILE, not_found},
    online, players, render, tiles,
    worlds::{self, OLDEST_DATA_VERSION},
};
use axum::{
    body::Body,
    extract::Query,
    http::StatusCode,
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::json;
use shared::{
    GetState, Payload, State,
    models::{
        server::{GetServer, GetServerActivityLogger},
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::{
    collections::BTreeSet,
    sync::{Arc, LazyLock},
    time::Duration,
};
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Player files read for the player layer, newest first.
const MAX_PLAYERS: usize = 300;
/// Entity region files read for one view.
const ENTITY_REGIONS: usize = 9;
/// Heatmap chunks sent for one view, the busiest first.
const HEAT_CELLS: i64 = 20_000;
/// The world border's reach.
const MAX_COORDINATE: f64 = 30_000_000.0;

type Version = (Uuid, String, i64, u64);
static POSITIONS: LazyLock<Memo<Version, Option<players::Position>>> =
    LazyLock::new(|| Memo::new(Duration::from_secs(3600), 20_000));
static ENTITIES: LazyLock<Memo<Version, Arc<Vec<entities::Entity>>>> =
    LazyLock::new(|| Memo::new(Duration::from_secs(3600), 256));

fn bad_request(message: impl AsRef<str>) -> ApiResponse {
    ApiResponse::error(message).with_status(StatusCode::BAD_REQUEST)
}

async fn power(map: &Map) -> Option<Power> {
    map.wings.power_state().await.ok().map(Power::from)
}

fn power_name(power: Option<Power>) -> &'static str {
    match power {
        Some(Power::Offline) => "offline",
        Some(Power::Starting) => "starting",
        Some(Power::Running) => "running",
        Some(Power::Stopping) => "stopping",
        None => "unknown",
    }
}

async fn overview(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(true).await?;
    if layout.unsupported.is_none() {
        db::track(&state.database, server.uuid).await?;
    }
    let settings = &map.settings;
    ApiResponse::new_serialized(json!({
        "unsupported": layout.unsupported,
        "too_old": layout.level.data_version.is_some_and(|version| version < OLDEST_DATA_VERSION),
        "level_name": layout.level_name,
        "level": layout.level,
        "worlds": layout.worlds,
        "power": power_name(power(&map).await),
        "capabilities": access::capabilities(&*permissions),
        "heatmap": {
            "enabled": settings.heatmap_enabled,
            "interval_minutes": settings.heatmap_interval_minutes,
            "retention_days": settings.heatmap_retention_days,
        },
        "limits": {
            "delete_chunks": settings.max_delete_chunks,
            "entity_regions": ENTITY_REGIONS,
        },
    }))
    .ok()
}

#[derive(Deserialize)]
struct Place {
    world: String,
    dimension: String,
}

async fn regions(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Query(place): Query<Place>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (_, dimension) = layout.dimension(&place.world, &place.dimension)?;
    let regions = map.regions(&dimension.folder, "region", false).await?;
    ApiResponse::new_serialized(json!({ "regions": *regions })).ok()
}

#[derive(Deserialize)]
struct TileQuery {
    world: String,
    dimension: String,
    x: i32,
    z: i32,
}

async fn tile(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Query(query): Query<TileQuery>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (_, dimension) = layout.dimension(&query.world, &query.dimension)?;
    let file = *map
        .regions(&dimension.folder, "region", false)
        .await?
        .iter()
        .find(|file| file.x == query.x && file.z == query.z)
        .ok_or_else(|| not_found("region not found"))?;
    let path = format!(
        "{}/region/{}",
        dimension.folder,
        coords::region_file_name(file.x, file.z)
    );
    let ceiling = (dimension.id == worlds::NETHER).then_some(map.settings.nether_ceiling);
    let png = tiles::png(&state, &map, &path, file, ceiling).await?;
    Ok(ApiResponse::new(Body::from(png))
        .with_header("Content-Type", "image/png")
        .with_header("Cache-Control", "private, max-age=86400"))
}

async fn player_list(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let Some(folder) = layout.worlds.first().and_then(|world| world.players.clone()) else {
        return ApiResponse::new_serialized(json!({ "players": [] })).ok();
    };
    let mut files: Vec<_> = map
        .wings
        .list(&format!("/{folder}"))
        .await?
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| !entry.directory)
        .filter_map(|entry| Some((players::file_uuid(&entry.name)?, entry)))
        .collect();
    files.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.modified));
    files.truncate(MAX_PLAYERS);
    let names = map
        .read("usercache.json", SMALL_FILE)
        .await?
        .map(|bytes| players::usercache(&bytes))
        .unwrap_or_default();

    let map = &map;
    let folder = &folder;
    let server_uuid = server.uuid;
    let positions: Vec<_> = stream::iter(files)
        .map(|(uuid, entry)| async move {
            let key = (
                server_uuid,
                format!("{folder}/{}", entry.name),
                entry.modified.timestamp(),
                entry.size,
            );
            let position = match POSITIONS.get(&key) {
                Some(position) => position,
                None => {
                    let position = match map.read(&key.1, SMALL_FILE).await {
                        Ok(Some(bytes)) => players::position(&bytes).ok(),
                        _ => None,
                    };
                    POSITIONS.insert(key, position.clone());
                    position
                }
            };
            (uuid, entry.modified.timestamp(), position)
        })
        .buffer_unordered(8)
        .collect()
        .await;

    let mut list: Vec<_> = positions
        .into_iter()
        .filter_map(|(uuid, seen, position)| {
            let position = position?;
            let (world, dimension) = layout.locate(position.world, &position.dimension)?;
            Some(json!({
                "uuid": uuid,
                "name": names.get(&uuid),
                "world": world.name,
                "dimension": dimension.id,
                "x": position.x,
                "y": position.y,
                "z": position.z,
                "yaw": position.yaw,
                "last_seen": seen,
            }))
        })
        .collect();
    list.sort_by_key(|player| std::cmp::Reverse(player["last_seen"].as_i64()));
    ApiResponse::new_serialized(json!({ "players": list })).ok()
}

async fn online_players(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    if power(&map).await != Some(Power::Running) {
        return ApiResponse::new_serialized(json!({ "running": false, "online": null })).ok();
    }
    let layout = map.layout(false).await?;
    // the console fallback sends `list`: only for users who may teleport anyway
    let console = Action::Teleport.granted(&*permissions) && access::may_list_by_console(&*permissions);
    let mut sent = Vec::new();
    let online = online::java(&map.wings, layout.properties.as_deref(), console, &mut sent).await;
    for command in sent {
        activity_logger
            .log("server:console.command", json!({ "command": command }))
            .await;
    }
    ApiResponse::new_serialized(json!({ "running": true, "online": online? })).ok()
}

#[derive(Deserialize)]
struct Area {
    world: String,
    dimension: String,
    x1: f64,
    z1: f64,
    x2: f64,
    z2: f64,
}

async fn entity_list(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Query(area): Query<Area>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    if ![area.x1, area.z1, area.x2, area.z2]
        .iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_COORDINATE)
    {
        return Err(bad_request("the area is outside the world"));
    }
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (_, dimension) = layout.dimension(&area.world, &area.dimension)?;
    let wanted = coords::regions_in_blocks((area.x1, area.z1), (area.x2, area.z2), ENTITY_REGIONS)
        .ok_or_else(|| bad_request("zoom in to see entities"))?;
    let files = map.regions(&dimension.folder, "entities", false).await?;
    let (min_x, max_x) = (area.x1.min(area.x2), area.x1.max(area.x2));
    let (min_z, max_z) = (area.z1.min(area.z2), area.z1.max(area.z2));
    let limit = map.settings.entity_limit as usize;
    let mut found = Vec::new();
    let mut truncated = false;
    for file in files.iter().filter(|file| wanted.contains(&(file.x, file.z))) {
        let path = format!(
            "{}/entities/{}",
            dimension.folder,
            coords::region_file_name(file.x, file.z)
        );
        let key = (server.uuid, path, file.modified, file.size);
        let list = match ENTITIES.get(&key) {
            Some(list) => list,
            None => {
                let Some(data) = map.read(&key.1, map.region_limit()).await? else {
                    continue;
                };
                let list = tokio::task::spawn_blocking(move || entities::region(&data))
                    .await
                    .map_err(|_| ApiResponse::error("reading entities failed"))?
                    .unwrap_or_default();
                let list = Arc::new(list);
                ENTITIES.insert(key, list.clone());
                list
            }
        };
        for entity in list.iter() {
            if (min_x..=max_x).contains(&entity.x) && (min_z..=max_z).contains(&entity.z) {
                if found.len() >= limit {
                    truncated = true;
                    break;
                }
                found.push(entity.clone());
            }
        }
    }
    ApiResponse::new_serialized(json!({ "entities": found, "truncated": truncated })).ok()
}

#[derive(Deserialize)]
struct HeatQuery {
    world: String,
    dimension: String,
    #[serde(default)]
    days: Option<u32>,
}

async fn heatmap(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Query(query): Query<HeatQuery>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let settings = crate::settings::load(&state).await?;
    let retention = settings.heatmap_retention_days.max(1);
    let days = query.days.unwrap_or(retention).clamp(1, retention);
    let since = chrono::Utc::now().date_naive() - chrono::Days::new(u64::from(days - 1));
    let cells = db::heat(
        &state.database,
        server.uuid,
        &query.world,
        &query.dimension,
        since,
        HEAT_CELLS,
    )
    .await?;
    let max = cells.iter().map(|&(_, _, count)| count).max().unwrap_or(0);
    ApiResponse::new_serialized(json!({
        "cells": cells,
        "max": max,
        "days": days,
        "enabled": settings.heatmap_enabled,
    }))
    .ok()
}

async fn reset_heatmap(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
) -> ApiResponseResult {
    access::authorize(Action::ResetHeatmap, &permissions, None)?;
    let rows = db::reset_heat(&state.database, server.uuid).await?;
    activity_logger
        .log("server:worldmap.reset-heatmap", json!({ "rows": rows }))
        .await;
    ApiResponse::new_serialized(json!({ "deleted": rows })).ok()
}

#[derive(Deserialize)]
struct DeleteChunks {
    world: String,
    dimension: String,
    rects: Vec<ChunkRect>,
}

async fn delete_chunks(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(data): Payload<DeleteChunks>,
) -> ApiResponseResult {
    let map = Map::open(&state, &server).await?;
    access::authorize(Action::DeleteChunks, &permissions, power(&map).await)?;
    if data.rects.is_empty() || data.rects.len() > 1000 {
        return Err(bad_request("select between 1 and 1000 areas"));
    }
    let limit = u64::from(map.settings.max_delete_chunks);
    let groups = coords::by_region(&data.rects, limit)
        .ok_or_else(|| bad_request(format!("select at most {limit} chunks at once")))?;
    let selected: usize = groups.values().map(BTreeSet::len).sum();
    let layout = map.layout(true).await?;
    let (world, dimension) = layout.dimension(&data.world, &data.dimension)?;

    let mut deleted = 0;
    let mut files = Vec::new();
    // terrain, then the entities and points of interest stored for the same chunks
    for kind in ["region", "entities", "poi"] {
        let present = map.regions(&dimension.folder, kind, true).await?;
        for ((x, z), slots) in &groups {
            if !present.iter().any(|file| file.x == *x && file.z == *z) {
                continue;
            }
            // the server may have been started meanwhile
            access::authorize(Action::DeleteChunks, &permissions, power(&map).await)?;
            let path = format!(
                "{}/{kind}/{}",
                dimension.folder,
                coords::region_file_name(*x, *z)
            );
            let Some(bytes) = map.read(&path, map.region_limit()).await? else {
                continue;
            };
            let slots = slots.clone();
            let (rewritten, removed) =
                tokio::task::spawn_blocking(move || anvil::without_chunks(&bytes, &slots))
                    .await
                    .map_err(|_| ApiResponse::error("rewriting the region failed"))?
                    .map_err(|err| {
                        ApiResponse::error(format!("{path} could not be read: {err}"))
                            .with_status(StatusCode::UNPROCESSABLE_ENTITY)
                    })?;
            if removed == 0 {
                continue;
            }
            let revision = map.write(&path, user.uuid, rewritten).await?;
            activity_logger
                .log(
                    "server:file.write",
                    json!({ "file": format!("/{path}"), "revision_id": revision }),
                )
                .await;
            if kind == "region" {
                deleted += removed;
            }
            files.push(path);
        }
    }
    map.forget(&dimension.folder);
    activity_logger
        .log(
            "server:worldmap.delete-chunks",
            json!({
                "world": world.name,
                "dimension": dimension.id,
                "selected": selected,
                "deleted": deleted,
                "files": files,
            }),
        )
        .await;
    ApiResponse::new_serialized(json!({
        "selected": selected,
        "deleted": deleted,
        "files": files.len(),
    }))
    .ok()
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Target {
    Player {
        name: String,
    },
    Position {
        world: String,
        dimension: String,
        x: f64,
        #[serde(default)]
        y: Option<f64>,
        z: f64,
    },
}

#[derive(Deserialize)]
struct Teleport {
    player: String,
    target: Target,
}

/// Dimension ids as commands accept them (`namespace:path`).
fn valid_dimension(id: &str) -> bool {
    id.split_once(':').is_some_and(|(namespace, path)| {
        !namespace.is_empty()
            && !path.is_empty()
            && namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
            && path
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-./".contains(&b))
    })
}

/// The console command for a teleport to a block position: the block's centre, in the target
/// dimension.
fn position_command(player: &str, dimension: &str, x: f64, y: f64, z: f64) -> String {
    format!(
        "execute in {dimension} run tp {player} {} {} {}",
        x.floor() + 0.5,
        y.floor(),
        z.floor() + 0.5
    )
}

async fn teleport(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(data): Payload<Teleport>,
) -> ApiResponseResult {
    let map = Map::open(&state, &server).await?;
    access::authorize(Action::Teleport, &permissions, power(&map).await)?;
    if !players::valid_name(&data.player) {
        return Err(bad_request("not a valid player name"));
    }
    let command = match data.target {
        Target::Player { name } => {
            if !players::valid_name(&name) {
                return Err(bad_request("not a valid player name"));
            }
            format!("tp {} {name}", data.player)
        }
        Target::Position {
            world,
            dimension,
            x,
            y,
            z,
        } => {
            if ![x, z].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
                || y.is_some_and(|y| !y.is_finite() || y.abs() > 4096.0)
            {
                return Err(bad_request("the position is outside the world"));
            }
            let layout = map.layout(false).await?;
            let (_, found) = layout.dimension(&world, &dimension)?;
            if !valid_dimension(&found.id) {
                return Err(bad_request("this dimension cannot be teleported to"));
            }
            let y = match y {
                Some(y) => y,
                None => {
                    let (block_x, block_z) = (x.floor() as i64, z.floor() as i64);
                    let (region_x, region_z) = (
                        coords::region_of_chunk(coords::chunk_of_block(x)),
                        coords::region_of_chunk(coords::chunk_of_block(z)),
                    );
                    let path = format!(
                        "{}/region/{}",
                        found.folder,
                        coords::region_file_name(region_x, region_z)
                    );
                    let ceiling = (found.id == worlds::NETHER).then_some(map.settings.nether_ceiling);
                    let data = map
                        .read(&path, map.region_limit())
                        .await?
                        .ok_or_else(|| bad_request("this part of the world was never generated: enter a height"))?;
                    let (local_x, local_z) = (block_x.rem_euclid(512) as usize, block_z.rem_euclid(512) as usize);
                    let height = tokio::task::spawn_blocking(move || {
                        render::standing_height(&data, local_x, local_z, ceiling)
                    })
                    .await
                    .map_err(|_| ApiResponse::error("reading the region failed"))?
                    .map_err(bad_request)?;
                    height.ok_or_else(|| {
                        bad_request("this chunk was never generated: enter a height")
                    })? as f64
                }
            };
            position_command(&data.player, &found.id, x, y, z)
        }
    };
    map.wings.commands(&[&command]).await?;
    activity_logger
        .log("server:console.command", json!({ "command": command }))
        .await;
    ApiResponse::new_serialized(json!({ "command": command })).ok()
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .route("/", get(overview))
        .route("/regions", get(regions))
        .route("/tile", get(tile))
        .route("/players", get(player_list))
        .route("/online", get(online_players))
        .route("/entities", get(entity_list))
        .route("/heatmap", get(heatmap).delete(reset_heatmap))
        .route("/chunks/delete", post(delete_chunks))
        .route("/teleport", post(teleport))
        .with_state(state.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleport_commands_only_take_checked_parts() {
        assert_eq!(
            position_command("Steve", "minecraft:the_nether", -10.2, 64.9, 3.0),
            "execute in minecraft:the_nether run tp Steve -10.5 64 3.5"
        );
        assert!(valid_dimension("minecraft:overworld"));
        assert!(valid_dimension("my_pack:caves/deep"));
        assert!(!valid_dimension("minecraft"));
        assert!(!valid_dimension("minecraft:over world"));
        assert!(!valid_dimension("Minecraft:Overworld"));
        assert!(!valid_dimension("a:b\nop Steve"));
    }

    #[test]
    fn teleport_targets_parse() {
        let player: Teleport = serde_json::from_value(json!({
            "player": "Steve",
            "target": { "type": "player", "name": "Alex" },
        }))
        .unwrap();
        assert!(matches!(player.target, Target::Player { ref name } if name == "Alex"));
        let position: Teleport = serde_json::from_value(json!({
            "player": "Steve",
            "target": { "type": "position", "world": "world", "dimension": "minecraft:overworld", "x": 1.5, "z": -2 },
        }))
        .unwrap();
        assert!(matches!(position.target, Target::Position { y: None, .. }));
    }
}
