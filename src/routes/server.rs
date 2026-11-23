//! The map API, mounted at `/api/client/servers/{server}/world-map`. The panel resolves the
//! server and the user's permissions before these handlers run; each handler checks its
//! [`Action`] first.
use crate::{
    access::{self, Action, Power},
    cache::Memo,
    coords, db, entities,
    map::{Layout, Map, SMALL_FILE, not_found},
    online::{self, Presence},
    ops,
    players::{self, Visibility},
    render, tiles,
    worlds::{self, OLDEST_DATA_VERSION},
};
use axum::{
    body::Body,
    extract::Query,
    http::{HeaderMap, StatusCode, header},
    routing::{get, post, put},
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::json;
use shared::{
    GetState, Payload, State,
    models::{
        server::{GetServer, GetServerActivityLogger},
        user::GetPermissionManager,
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
        "too_old": layout
            .main()
            .or(layout.worlds.first())
            .and_then(|world| world.level.data_version)
            .is_some_and(|version| version < OLDEST_DATA_VERSION),
        "level_name": layout.level_name,
        "main_folder": layout.main_folder,
        "world_container": layout.world_container,
        "worlds": layout.worlds,
        "extra_worlds": layout.extra_worlds,
        "power": power_name(power(&map).await),
        "capabilities": access::capabilities(&*permissions),
        "player_visibility": db::player_visibility(&state.database, server.uuid).await?,
        "heatmap": {
            "enabled": settings.heatmap_enabled,
            "interval_minutes": settings.heatmap_interval_minutes,
            "retention_days": settings.heatmap_retention_days,
        },
        "limits": {
            "operation_chunks": settings.max_delete_chunks,
            "entity_regions": ENTITY_REGIONS,
            "extra_worlds": worlds::MAX_EXTRA,
        },
        "backups": {
            "retention_days": settings.backup_retention_days,
            "folder": ops::BACKUPS,
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
    // regions with only entities or points of interest still count for inverted selections
    let mut others = BTreeSet::new();
    for kind in ["entities", "poi"] {
        for file in map.regions(&dimension.folder, kind, false).await?.iter() {
            if !regions.iter().any(|region| region.x == file.x && region.z == file.z) {
                others.insert((file.x, file.z));
            }
        }
    }
    ApiResponse::new_serialized(json!({ "regions": *regions, "others": others })).ok()
}

#[derive(Deserialize)]
struct TileQuery {
    world: String,
    dimension: String,
    x: i32,
    z: i32,
}

/// A region tile. Browsers revalidate it every time (`ETag` from the file's state and the
/// renderer), so a re-rendered tile is never served stale from their cache.
async fn tile(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    headers: HeaderMap,
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
    let ceiling = dimension.ceiling.then_some(map.settings.nether_ceiling);
    let etag = format!(
        "\"{}-{}-{}\"",
        tiles::variant(ceiling).replace(['"', ' '], ""),
        file.modified,
        file.size
    );
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag))
    {
        return Ok(ApiResponse::new(Body::empty())
            .with_status(StatusCode::NOT_MODIFIED)
            .with_header("ETag", &etag));
    }
    let png = tiles::png(&state, &map, &path, file, ceiling).await?;
    Ok(ApiResponse::new(Body::from(png))
        .with_header("Content-Type", "image/png")
        .with_header("Cache-Control", "private, no-cache")
        .with_header("ETag", &etag))
}

/// Who is online, as far as the server says without its console.
async fn presence(map: &Map, properties: Option<&str>) -> Presence {
    match power(map).await {
        Some(Power::Running) => match online::quietly(&map.wings, properties).await {
            Ok(online) => Presence::Answered(online),
            Err(err) => Presence::Unknown(err.message().to_string()),
        },
        Some(_) => Presence::NotRunning,
        None => Presence::Unknown("the node did not report the server's state".into()),
    }
}

/// Every player with a saved position in a world the map knows, newest first.
async fn markers(
    map: &Map,
    layout: &Layout,
    server_uuid: Uuid,
) -> Result<Vec<players::Marker>, ApiResponse> {
    let Some(folder) = layout.main().and_then(|world| world.players.clone()) else {
        return Ok(Vec::new());
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
    let mut names = map
        .read("usercache.json", SMALL_FILE)
        .await?
        .map(|bytes| players::usercache(&bytes))
        .unwrap_or_default();

    let folder = &folder;
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
            Some(players::Marker {
                uuid,
                name: names.remove(&uuid),
                world: world.name.clone(),
                dimension: dimension.id.clone(),
                x: position.x,
                y: position.y,
                z: position.z,
                yaw: position.yaw,
                last_seen: seen,
                online: false,
            })
        })
        .collect();
    list.sort_by_key(|player| std::cmp::Reverse(player.last_seen));
    Ok(list)
}

/// The player layer: each player's last saved position and whether they are online now, only
/// of the kinds (online, offline) the server's setting shows.
async fn player_list(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let visibility = db::player_visibility(&state.database, server.uuid).await?;
    if !visibility.online && !visibility.offline {
        return ApiResponse::new_serialized(json!({ "players": [], "presence": null })).ok();
    }
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (presence, markers) = tokio::join!(
        presence(&map, layout.properties.as_deref()),
        markers(&map, &layout, server.uuid)
    );
    let shown = players::visible(markers?, &presence, visibility);
    ApiResponse::new_serialized(json!({
        "players": shown,
        "presence": presence.summary(visibility.online),
    }))
    .ok()
}

/// Who is online by name, for teleports; the console's `list` for users who may teleport.
async fn online_players(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let running = power(&map).await == Some(Power::Running);
    if !db::player_visibility(&state.database, server.uuid).await?.online {
        return ApiResponse::new_serialized(
            json!({ "running": running, "online": null, "hidden": true }),
        )
        .ok();
    }
    if !running {
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

async fn update_player_visibility(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(data): Payload<Visibility>,
) -> ApiResponseResult {
    access::authorize(Action::ManagePlayers, &permissions, None)?;
    db::set_player_visibility(&state.database, server.uuid, data).await?;
    activity_logger
        .log("server:worldmap.update-player-visibility", json!(data))
        .await;
    ApiResponse::new_serialized(json!({ "player_visibility": data })).ok()
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
            let Some(key) = found.key.clone().filter(|key| worlds::valid_key(key)) else {
                return Err(bad_request(
                    "the server does not load this world, so nobody can be teleported there",
                ));
            };
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
                    let ceiling = found.ceiling.then_some(map.settings.nether_ceiling);
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
            position_command(&data.player, &key, x, y, z)
        }
    };
    map.wings.commands(&[&command]).await?;
    activity_logger
        .log("server:console.command", json!({ "command": command }))
        .await;
    ApiResponse::new_serialized(json!({ "command": command })).ok()
}

#[derive(Deserialize)]
struct WorldSettings {
    extra_worlds: Vec<String>,
}

/// The world folders added by hand: each must be a folder below the server root.
fn extra_world_paths(input: &[String]) -> Result<Vec<String>, String> {
    if input.len() > worlds::MAX_EXTRA {
        return Err(format!("add at most {} world folders", worlds::MAX_EXTRA));
    }
    let mut paths = Vec::new();
    for raw in input {
        let path = worlds::relative_path(raw.trim()).ok_or_else(|| {
            format!(
                "{:?} is not a folder inside the server: use a path like worlds/survival, without .. or a leading /",
                raw.trim()
            )
        })?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

async fn update_worlds(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(data): Payload<WorldSettings>,
) -> ApiResponseResult {
    access::authorize(Action::ManageWorlds, &permissions, None)?;
    let paths = extra_world_paths(&data.extra_worlds).map_err(bad_request)?;
    db::set_extra_worlds(&state.database, server.uuid, &paths).await?;
    let map = Map::open(&state, &server).await?;
    map.forget_layout();
    let layout = map.layout(true).await?;
    activity_logger
        .log("server:worldmap.update-worlds", json!({ "extra_worlds": paths }))
        .await;
    ApiResponse::new_serialized(json!({ "extra_worlds": layout.extra_worlds })).ok()
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
        .route("/teleport", post(teleport))
        .route("/worlds", put(update_worlds))
        .route("/players/visibility", put(update_player_visibility))
        .with_state(state.clone())
        .merge(super::tools::router(state))
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
        // the key comes from the world's layout, never from the request
        assert!(!worlds::valid_key("a:b\nop Steve"));
    }

    #[test]
    fn hand_added_world_folders_stay_inside_the_server() {
        let paths = |input: &[&str]| {
            extra_world_paths(&input.iter().map(|path| path.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            paths(&["worlds/survival", " ./arena/ ", "worlds/survival"]).unwrap(),
            vec!["worlds/survival", "arena"]
        );
        assert_eq!(paths(&[]).unwrap(), Vec::<String>::new());
        for bad in ["../other-server", "/etc", "worlds/../../x", "", "C:\\maps"] {
            assert!(paths(&[bad]).is_err(), "{bad:?}");
        }
        let too_many: Vec<String> = (0..=worlds::MAX_EXTRA).map(|i| format!("w{i}")).collect();
        assert!(extra_world_paths(&too_many).is_err());
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
