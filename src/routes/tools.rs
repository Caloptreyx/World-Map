//! The chunk tools API (MCA Selector's features), mounted with the map API at
//! `/api/client/servers/{server}/world-map`: filters, overlays, background jobs for deleting,
//! changing and importing chunks, the zip export, upload staging and the operation history with
//! its backups. Every handler checks its [`Action`] before any Wings call.
use crate::{
    access::{self, Action, Power},
    changer, db, filter,
    jobs::{self, Kind, Stop},
    map::{Map, not_found},
    ops::{self, IMPORTS},
    overlay,
    process::{self, ImportPlan, Listing},
    relocate::{self, Offset},
    selection::{Selection, WireSelection},
    tiles, worlds,
};
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use shared::{
    GetState, Payload, State,
    models::{
        server::{GetServer, GetServerActivityLogger, ServerActivityLogger},
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::sync::Arc;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Longest query or change text accepted.
const MAX_TEXT: usize = 8192;

fn bad_request(message: impl AsRef<str>) -> ApiResponse {
    ApiResponse::error(message).with_status(StatusCode::BAD_REQUEST)
}

async fn power(map: &Map) -> Option<Power> {
    map.wings.power_state().await.ok().map(Power::from)
}

fn selection(wire: &WireSelection) -> Result<Selection, ApiResponse> {
    Selection::from_wire(wire).map_err(bad_request)
}

/// A staging folder the request may name: `.worldmap-imports/<name>[/<sub>]`, inside the server.
fn staging_folder(raw: &str) -> Result<String, ApiResponse> {
    let path = worlds::relative_path(raw.trim())
        .filter(|path| path.starts_with(&format!("{IMPORTS}/")) && path.len() > IMPORTS.len() + 1)
        .ok_or_else(|| bad_request(format!("the import folder must be inside {IMPORTS}/")))?;
    Ok(path)
}

fn new_staging_name() -> String {
    format!(
        "{IMPORTS}/{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

#[derive(Deserialize)]
struct QueryText {
    query: String,
}

async fn check_filter(
    permissions: GetPermissionManager,
    Payload(data): Payload<QueryText>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    if data.query.len() > MAX_TEXT {
        return Err(bad_request("the query is too long"));
    }
    ApiResponse::new_serialized(match filter::parse(&data.query) {
        Ok(parsed) => {
            let needs = parsed.needs();
            json!({
                "ok": true,
                "normalized": parsed.to_query(),
                "needs": { "region": needs.region, "entities": needs.entities, "poi": needs.poi },
            })
        }
        Err(err) => json!({ "ok": false, "error": err.message, "position": err.position }),
    })
    .ok()
}

#[derive(Deserialize)]
struct ChangesText {
    changes: String,
}

async fn check_changes(
    permissions: GetPermissionManager,
    Payload(data): Payload<ChangesText>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    if data.changes.len() > MAX_TEXT {
        return Err(bad_request("the changes are too long"));
    }
    ApiResponse::new_serialized(match changer::parse(&data.changes) {
        Ok(parsed) => json!({ "ok": true, "normalized": parsed.to_query() }),
        Err(err) => json!({ "ok": false, "error": err.message, "position": err.position }),
    })
    .ok()
}

#[derive(Deserialize)]
struct Place {
    world: String,
    dimension: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Source {
    Place(Place),
    Folder { folder: String },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JobRequest {
    Filter {
        world: String,
        dimension: String,
        query: String,
        #[serde(default)]
        selection: Option<WireSelection>,
    },
    Delete {
        world: String,
        dimension: String,
        selection: WireSelection,
    },
    Change {
        world: String,
        dimension: String,
        selection: WireSelection,
        changes: String,
        #[serde(default)]
        force: bool,
    },
    Import {
        source: Source,
        target: Place,
        #[serde(default)]
        offset: Offset,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        source_selection: Option<WireSelection>,
        #[serde(default)]
        target_selection: Option<WireSelection>,
        #[serde(default)]
        sections: Option<String>,
    },
}

/// Logs a finished write job in the server's activity log.
async fn log_result(
    logger: &ServerActivityLogger,
    event: &str,
    details: serde_json::Value,
    result: &Result<serde_json::Value, Stop>,
) {
    let mut details = details;
    match result {
        Ok(value) => {
            details["chunks"] = value["chunks"].clone();
            details["files"] = value["files"].clone();
            details["backup"] = value["operation"]["backup"].clone();
        }
        Err(Stop::Failed(error)) => details["error"] = json!(error),
        Err(Stop::Cancelled) => details["cancelled"] = json!(true),
    }
    logger.log(event, details).await;
}

async fn create_job(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(request): Payload<JobRequest>,
) -> ApiResponseResult {
    // permissions first, before any Wings call or parsing work
    let action = match &request {
        JobRequest::Filter { .. } => Action::View,
        JobRequest::Delete { .. } => Action::DeleteChunks,
        JobRequest::Change { .. } => Action::EditChunks,
        JobRequest::Import { .. } => Action::ImportChunks,
    };
    access::authorize_permissions(action, &permissions)?;
    let map = Map::open(&state, &server).await?;
    if action != Action::View {
        access::authorize(action, &permissions, power(&map).await)?;
    }
    let layout = map.layout(false).await?;
    let logger = activity_logger.0;
    let user = user.uuid;
    let server_uuid = server.uuid;

    let job = match request {
        JobRequest::Filter {
            world,
            dimension,
            query,
            selection: within,
        } => {
            if query.len() > MAX_TEXT {
                return Err(bad_request("the query is too long"));
            }
            let parsed = filter::parse(&query).map_err(|err| {
                bad_request(format!(
                    "{} (at character {})",
                    err.message,
                    err.position + 1
                ))
            })?;
            let within = within.as_ref().map(selection).transpose()?;
            let (_, found) = layout.dimension(&world, &dimension)?;
            let folder = found.folder.clone();
            jobs::start(
                server_uuid,
                Some(user),
                Kind::Filter,
                &world,
                &dimension,
                move |handle| async move {
                    process::scan(&handle, &map, &folder, Arc::new(parsed), within).await
                },
            )?
        }
        JobRequest::Delete {
            world,
            dimension,
            selection: wire,
        } => {
            let chosen = selection(&wire)?;
            let (_, found) = layout.dimension(&world, &dimension)?;
            let folder = found.folder.clone();
            jobs::start(
                server_uuid,
                Some(user),
                Kind::Delete,
                &world.clone(),
                &dimension.clone(),
                move |handle| async move {
                    let result = process::delete(
                        &handle,
                        &map,
                        user,
                        (&world, &dimension),
                        &folder,
                        &chosen,
                    )
                    .await;
                    log_result(
                        &logger,
                        "server:worldmap.delete-chunks",
                        json!({ "world": world, "dimension": dimension }),
                        &result,
                    )
                    .await;
                    result
                },
            )?
        }
        JobRequest::Change {
            world,
            dimension,
            selection: wire,
            changes,
            force,
        } => {
            if changes.len() > MAX_TEXT {
                return Err(bad_request("the changes are too long"));
            }
            let parsed = changer::parse(&changes).map_err(|err| {
                bad_request(format!(
                    "{} (at character {})",
                    err.message,
                    err.position + 1
                ))
            })?;
            if parsed.is_empty() {
                return Err(bad_request("choose at least one field to change"));
            }
            let chosen = selection(&wire)?;
            let (_, found) = layout.dimension(&world, &dimension)?;
            let folder = found.folder.clone();
            jobs::start(
                server_uuid,
                Some(user),
                Kind::Change,
                &world.clone(),
                &dimension.clone(),
                move |handle| async move {
                    let normalized = parsed.to_query();
                    let result = process::change(
                        &handle,
                        &map,
                        user,
                        (&world, &dimension),
                        &folder,
                        &chosen,
                        Arc::new(parsed),
                        force,
                    )
                    .await;
                    log_result(
                    &logger,
                    "server:worldmap.change-chunks",
                    json!({ "world": world, "dimension": dimension, "changes": normalized, "force": force }),
                    &result,
                )
                .await;
                    result
                },
            )?
        }
        JobRequest::Import {
            source,
            target,
            offset,
            overwrite,
            source_selection,
            target_selection,
            sections,
        } => {
            if offset.x.abs() > 1_875_000 || offset.z.abs() > 1_875_000 || offset.y.abs() > 64 {
                return Err(bad_request("the offset is outside the world"));
            }
            let ranges = match sections.as_deref().map(str::trim) {
                None | Some("") => relocate::Ranges::all(),
                Some(text) => relocate::parse_ranges(text).map_err(bad_request)?,
            };
            let source_selection = source_selection.as_ref().map(selection).transpose()?;
            let target_selection = target_selection.as_ref().map(selection).transpose()?;
            let (_, target_dimension) = layout.dimension(&target.world, &target.dimension)?;
            let (source_folder, source_label, staged) = match source {
                Source::Place(place) => {
                    let (_, found) = layout.dimension(&place.world, &place.dimension)?;
                    (
                        found.folder.clone(),
                        json!({ "world": place.world, "dimension": place.dimension }),
                        None,
                    )
                }
                Source::Folder { folder } => {
                    let folder = staging_folder(&folder)?;
                    (folder.clone(), json!({ "folder": folder }), Some(folder))
                }
            };
            let plan = ImportPlan {
                source_folder,
                target_folder: target_dimension.folder.clone(),
                offset,
                overwrite,
                source_selection,
                target_selection,
                ranges,
            };
            let (world, dimension) = (target.world, target.dimension);
            jobs::start(
                server_uuid,
                Some(user),
                Kind::Import,
                &world.clone(),
                &dimension.clone(),
                move |handle| async move {
                    let snapshot = new_staging_name() + "-source";
                    let result =
                        process::import(&handle, &map, user, (&world, &dimension), plan, &snapshot)
                            .await;
                    // an uploaded source is not needed any more once it was imported
                    if let Some(folder) = staged
                        && result.is_ok()
                    {
                        let top = folder.split('/').take(2).collect::<Vec<_>>().join("/");
                        if let Err(err) = map.wings.delete("/", &[top]).await {
                            tracing::warn!(
                                "world map: could not remove {folder}: {}",
                                jobs::describe(err).await
                            );
                        }
                    }
                    log_result(
                    &logger,
                    "server:worldmap.import-chunks",
                    json!({ "source": source_label, "world": world, "dimension": dimension, "offset": offset, "overwrite": overwrite }),
                    &result,
                )
                .await;
                    result
                },
            )?
        }
    };
    Ok(ApiResponse::new_serialized(json!({ "job": job })).with_status(StatusCode::ACCEPTED))
}

async fn list_jobs(permissions: GetPermissionManager, server: GetServer) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    ApiResponse::new_serialized(json!({ "jobs": jobs::list(server.uuid) })).ok()
}

async fn get_job(
    permissions: GetPermissionManager,
    server: GetServer,
    Path(id): Path<Uuid>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let job = jobs::get(server.uuid, id).ok_or_else(|| not_found("job not found"))?;
    ApiResponse::new_serialized(json!({ "job": job })).ok()
}

async fn cancel_job(
    permissions: GetPermissionManager,
    server: GetServer,
    Path(id): Path<Uuid>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let job = jobs::get(server.uuid, id).ok_or_else(|| not_found("job not found"))?;
    // stopping someone's write job needs the right to start it
    let action = match job.kind {
        Kind::Filter => Action::View,
        Kind::Delete => Action::DeleteChunks,
        Kind::Change => Action::EditChunks,
        Kind::Import | Kind::Restore => Action::ImportChunks,
    };
    if job.kind != Kind::Restore {
        access::authorize_permissions(action, &permissions)?;
    }
    let job = jobs::cancel(server.uuid, id).ok_or_else(|| not_found("job not found"))?;
    ApiResponse::new_serialized(json!({ "job": job })).ok()
}

#[derive(Deserialize)]
struct OverlayQuery {
    world: String,
    dimension: String,
    x: i32,
    z: i32,
    overlay: String,
    #[serde(default)]
    parameter: Option<String>,
}

/// One region's overlay values, cached like tiles while its files keep their size and time.
async fn overlay_region(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Query(query): Query<OverlayQuery>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let parsed = overlay::parse(
        &query.overlay,
        query.parameter.as_deref().filter(|p| !p.is_empty()),
    )
    .map_err(bad_request)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (_, dimension) = layout.dimension(&query.world, &query.dimension)?;
    let needs = parsed.needs();
    let mut version = format!("v{}:{}", overlay::OVERLAY_VERSION, parsed.key());
    let mut any = false;
    for kind in process::KINDS {
        let wanted = kind == "region"
            || (kind == "entities" && needs.entities)
            || (kind == "poi" && needs.poi);
        if !wanted {
            continue;
        }
        let files = map.regions(&dimension.folder, kind, false).await?;
        match files
            .iter()
            .find(|file| file.x == query.x && file.z == query.z)
        {
            Some(file) => {
                any = true;
                version.push_str(&format!(":{kind}={}-{}", file.modified, file.size));
            }
            None => version.push_str(&format!(":{kind}=-")),
        }
    }
    if !any {
        return Err(not_found("region not found"));
    }
    let path = process::file_path(&dimension.folder, "region", (query.x, query.z));
    let key = parsed.key();
    let values = match db::overlay(&state.database, server.uuid, &path, &key).await? {
        Some((stored, values)) if stored == version => {
            db::touch_overlay(&state.database, server.uuid, &path, &key).await?;
            values
        }
        _ => {
            let _slot = tiles::slot(map.settings.render_concurrency).await?;
            let listing =
                Listing::read(&map, &dimension.folder)
                    .await
                    .map_err(|stop| match stop {
                        Stop::Failed(error) => {
                            ApiResponse::error(error).with_status(StatusCode::BAD_GATEWAY)
                        }
                        Stop::Cancelled => ApiResponse::error("cancelled"),
                    })?;
            let values = process::overlay_values(
                &map,
                &dimension.folder,
                &listing,
                (query.x, query.z),
                Arc::new(parsed),
            )
            .await
            .map_err(|stop| match stop {
                Stop::Failed(error) => {
                    ApiResponse::error(error).with_status(StatusCode::UNPROCESSABLE_ENTITY)
                }
                Stop::Cancelled => ApiResponse::error("cancelled"),
            })?;
            db::store_overlay(&state.database, server.uuid, &path, &key, &version, &values).await?;
            values
        }
    };
    let finite = values.iter().flatten().copied();
    let min = finite.clone().reduce(f64::min);
    let max = finite.reduce(f64::max);
    ApiResponse::new_serialized(json!({ "values": values, "min": min, "max": max })).ok()
}

#[derive(Deserialize)]
struct ExportRequest {
    world: String,
    dimension: String,
    selection: WireSelection,
}

/// The selected chunks as a zip of region files, streamed one region at a time.
async fn export(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Payload(data): Payload<ExportRequest>,
) -> ApiResponseResult {
    access::authorize(Action::ExportChunks, &permissions, None)?;
    let chosen = selection(&data.selection)?;
    let map = Map::open(&state, &server).await?;
    let layout = map.layout(false).await?;
    let (world, dimension) = layout.dimension(&data.world, &data.dimension)?;
    let folder = dimension.folder.clone();
    let listing = Listing::read(&map, &folder)
        .await
        .map_err(|stop| match stop {
            Stop::Failed(error) => ApiResponse::error(error).with_status(StatusCode::BAD_GATEWAY),
            Stop::Cancelled => ApiResponse::error("cancelled"),
        })?;
    let resolved = chosen.resolve(&listing.all());
    if resolved.is_empty() {
        return Err(bad_request(
            "the selection holds no chunk of this dimension",
        ));
    }
    // the files read are an upper bound of the zip's size
    let mut bytes = 0u64;
    for kind in process::KINDS {
        let files = map.regions(&folder, kind, false).await?;
        bytes += files
            .iter()
            .filter(|file| resolved.contains_key(&(file.x, file.z)))
            .map(|file| file.size)
            .sum::<u64>();
    }
    if resolved.len() * 3 > process::ZipWriter::MAX_ENTRIES || bytes > process::ZipWriter::MAX_BYTES
    {
        return Err(bad_request(
            "the selection is too large for one zip (4 GiB or 65535 files): select fewer regions",
        ));
    }
    let chunks: u64 = resolved.values().map(|set| u64::from(set.count())).sum();
    activity_logger
        .log(
            "server:worldmap.export-chunks",
            json!({ "world": world.name, "dimension": dimension.id, "chunks": chunks, "regions": resolved.len() }),
        )
        .await;
    let name = format!(
        "{}-{}-chunks.zip",
        world.name.replace(['/', '\\', '"'], "_"),
        dimension.id.replace([':', '/', '"'], "_")
    );
    let (sink, source) = tokio::io::duplex(256 << 10);
    tokio::spawn(async move {
        if let Err(err) = process::export(&map, &folder, resolved, &listing, sink).await {
            tracing::warn!("world map: export stopped: {err}");
        }
    });
    Ok(ApiResponse::new_stream(source)
        .with_header("Content-Type", "application/zip")
        .with_header(
            "Content-Disposition",
            format!("attachment; filename=\"{name}\""),
        ))
}

/// Creates an empty staging folder for uploads to import.
async fn create_staging(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
) -> ApiResponseResult {
    access::authorize_permissions(Action::ImportChunks, &permissions)?;
    let map = Map::open(&state, &server).await?;
    let folder = new_staging_name();
    let (parent, name) = folder
        .rsplit_once('/')
        .expect("staging names have a parent");
    map.wings
        .create_directory(&format!("/{parent}"), name)
        .await?;
    ApiResponse::new_serialized(json!({ "folder": folder })).ok()
}

#[derive(Deserialize)]
struct Extract {
    folder: String,
    file: String,
}

/// Extracts an uploaded zip in its staging folder and finds the folder holding `region/`,
/// `entities/` or `poi/` (the zip's root or a world folder up to two levels down).
async fn extract_staging(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
    Payload(data): Payload<Extract>,
) -> ApiResponseResult {
    access::authorize_permissions(Action::ImportChunks, &permissions)?;
    // unpacking an archive is the panel's files.archive permission
    permissions.has_server_permission("files.archive")?;
    let folder = staging_folder(&data.folder)?;
    if data.file.contains(['/', '\\'])
        || data.file.starts_with('.')
        || !data.file.to_lowercase().ends_with(".zip")
    {
        return Err(bad_request("upload one .zip file into the staging folder"));
    }
    let map = Map::open(&state, &server).await?;
    map.wings
        .decompress(&format!("/{folder}"), &data.file)
        .await?;
    let mut queue = vec![(folder.clone(), 0)];
    while let Some((current, depth)) = queue.pop() {
        let entries = map
            .wings
            .list(&format!("/{current}"))
            .await?
            .unwrap_or_default();
        if entries
            .iter()
            .any(|entry| entry.directory && process::KINDS.contains(&entry.name.as_str()))
        {
            return ApiResponse::new_serialized(json!({ "folder": current })).ok();
        }
        if depth < 2 {
            for entry in entries.iter().filter(|entry| entry.directory) {
                queue.push((format!("{current}/{}", entry.name), depth + 1));
            }
        }
    }
    Err(bad_request(
        "the zip holds no region, entities or poi folder",
    ))
}

async fn list_operations(
    state: GetState,
    permissions: GetPermissionManager,
    server: GetServer,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let operations = ops::list(&map, 20).await?;
    ApiResponse::new_serialized(json!({ "operations": operations })).ok()
}

async fn restore_operation(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    activity_logger: GetServerActivityLogger,
    Path(id): Path<Uuid>,
) -> ApiResponseResult {
    access::authorize(Action::View, &permissions, None)?;
    let map = Map::open(&state, &server).await?;
    let operation = ops::get(&map, id).await?;
    let action = access::restore_action(operation.kind).ok_or_else(|| {
        ApiResponse::error("a restore cannot be restored itself").with_status(StatusCode::CONFLICT)
    })?;
    access::authorize(action, &permissions, power(&map).await)?;
    if !operation.restorable {
        return Err(ApiResponse::error(
            "this operation cannot be restored: it was restored or pruned already, or a newer operation changed the same files",
        )
        .with_status(StatusCode::CONFLICT));
    }
    let logger = activity_logger.0;
    let user = user.uuid;
    let (world, dimension) = (operation.world.clone(), operation.dimension.clone());
    let job = jobs::start(
        server.uuid,
        Some(user),
        Kind::Restore,
        &world.clone(),
        &dimension.clone(),
        move |handle| async move {
            handle.message(format!("restoring from {}", operation.backup));
            let _lock = ops::lock(map.server).await;
            let result = match ops::restore(&map, user, id).await {
                Ok(restored) => Ok(
                    json!({ "chunks": operation.summary["chunks"], "files": restored.files, "operation": restored }),
                ),
                Err(err) => Err(Stop::Failed(jobs::describe(err).await)),
            };
            if let Ok(layout) = map.layout(false).await
                && let Ok((_, found)) = layout.dimension(&world, &dimension)
            {
                map.forget(&found.folder);
            }
            log_result(
                &logger,
                "server:worldmap.restore",
                json!({ "operation": id, "backup": operation.backup }),
                &result,
            )
            .await;
            result
        },
    )?;
    Ok(ApiResponse::new_serialized(json!({ "job": job })).with_status(StatusCode::ACCEPTED))
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .route("/filter/check", post(check_filter))
        .route("/changes/check", post(check_changes))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/overlay", get(overlay_region))
        .route("/export", post(export))
        .route("/imports", post(create_staging))
        .route("/imports/extract", post(extract_staging))
        .route("/operations", get(list_operations))
        .route("/operations/{id}/restore", post(restore_operation))
        .with_state(state.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_folders_stay_in_the_imports_folder() {
        assert_eq!(
            staging_folder(".worldmap-imports/20261124-101010-abcdef12").unwrap(),
            ".worldmap-imports/20261124-101010-abcdef12"
        );
        assert_eq!(
            staging_folder(".worldmap-imports/x/world").unwrap(),
            ".worldmap-imports/x/world"
        );
        for bad in [
            ".worldmap-imports",
            ".worldmap-imports/",
            "world",
            "/etc",
            ".worldmap-imports/../world",
            "../.worldmap-imports/x",
        ] {
            assert!(staging_folder(bad).is_err(), "{bad:?}");
        }
        assert!(new_staging_name().starts_with(".worldmap-imports/"));
    }

    #[test]
    fn job_requests_parse() {
        let import: JobRequest = serde_json::from_value(json!({
            "kind": "import",
            "source": { "folder": ".worldmap-imports/a" },
            "target": { "world": "world", "dimension": "minecraft:overworld" },
            "offset": { "x": 33, "y": 0, "z": -2 },
            "overwrite": true,
        }))
        .unwrap();
        assert!(matches!(
            import,
            JobRequest::Import {
                source: Source::Folder { .. },
                overwrite: true,
                ..
            }
        ));
        let from_world: JobRequest = serde_json::from_value(json!({
            "kind": "import",
            "source": { "world": "world", "dimension": "minecraft:the_nether" },
            "target": { "world": "creative", "dimension": "minecraft:overworld" },
        }))
        .unwrap();
        assert!(matches!(
            from_world,
            JobRequest::Import {
                source: Source::Place(_),
                overwrite: false,
                ..
            }
        ));
        let filter: JobRequest = serde_json::from_value(json!({
            "kind": "filter", "world": "world", "dimension": "minecraft:overworld",
            "query": "InhabitedTime < \"5 minutes\"",
        }))
        .unwrap();
        assert!(matches!(
            filter,
            JobRequest::Filter {
                selection: None,
                ..
            }
        ));
        assert!(serde_json::from_value::<JobRequest>(json!({ "kind": "format" })).is_err());
    }
}
