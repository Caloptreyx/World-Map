//! Safe writes for the chunk tools: every changed world file is replaced atomically with its
//! original kept as a backup, and whole operations can be restored.
//!
//! Wings' `files/write` truncates and writes in place, `files/rename` never overwrites (it
//! skips and reports `renamed: 0`) and creates parent folders, `files/delete` is permanent.
//! So each file goes through:
//! 1. a fresh power check (not the panel's 15 second cache): the server must be offline, else
//!    the whole operation stops (409);
//! 2. `files/write` of the new content to `<dir>/.<name>.worldmap-tmp`;
//! 3. a rename of the original (when there is one) into the operation's backup folder
//!    `.worldmap-backups/<YYYYMMDD-HHMMSS>-<kind>-<8 hex>/<original path>`: the move IS the
//!    backup, instant and atomic;
//! 4. a rename of the temporary file to the original path; on failure the backup is moved back.
//!
//! Removing a file is steps 1 and 3. A crash can at worst leave a file missing (its backup is
//! there and a restore puts it back), never half-written. The backup folder gets a
//! `worldmap-operation.json` manifest when the operation ends, and every operation has a row in
//! `dev_caloptreyx_worldmap_operations` (inserted when it begins, its files recorded after each
//! one, so even a crashed operation can be restored).
//!
//! Concurrency: nothing in this module locks. Callers hold [`lock`] (or [`try_lock`]) for the
//! server around a whole write job that uses a [`Writer`] and around [`restore`]; [`prune`]
//! skips servers whose lock is held.
use crate::{access::Power, db, files::Wings, map::Map, settings};
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::{
    State,
    models::{ByUuid, server::Server},
    response::ApiResponse,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, PoisonError},
};
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

/// The folder below the server root holding every operation's backup folder.
pub const BACKUPS: &str = ".worldmap-backups";
/// The folder below the server root holding upload staging folders for imports.
pub const IMPORTS: &str = ".worldmap-imports";
/// The manifest written into each backup folder.
pub const MANIFEST: &str = "worldmap-operation.json";
/// Suffix of the temporary files new content is written to.
pub const TEMP_SUFFIX: &str = ".worldmap-tmp";
/// Hours an import staging folder is kept after its last change.
pub const IMPORT_RETENTION_HOURS: i64 = 24;

/// What an operation did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Delete,
    Change,
    Import,
    Restore,
}

impl OperationKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Change => "change",
            Self::Import => "import",
            Self::Restore => "restore",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Delete, Self::Change, Self::Import, Self::Restore]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

/// How an operation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// Every file was written.
    Done,
    /// Still running, failed or aborted midway: what was done is kept and can be restored.
    Partial,
    /// Its files were put back by a restore.
    Restored,
    /// Its backup folder was deleted after the retention period.
    Pruned,
    /// No file changed: no row and no backup folder are kept.
    Empty,
}

impl OperationState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Partial => "partial",
            Self::Restored => "restored",
            Self::Pruned => "pruned",
            Self::Empty => "empty",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Done,
            Self::Partial,
            Self::Restored,
            Self::Pruned,
            Self::Empty,
        ]
        .into_iter()
        .find(|state| state.name() == name)
    }
}

/// A file an operation replaced, removed or created (paths relative to the server root).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// Whether a file was there before: its backup is at the same path in the backup folder.
    pub existed: bool,
}

/// An operation as the history shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Operation {
    pub id: Uuid,
    pub kind: OperationKind,
    /// Unix seconds.
    pub created: i64,
    pub user: Option<Uuid>,
    pub username: Option<String>,
    pub world: String,
    pub dimension: String,
    /// The backup folder (relative to the server root).
    pub backup: String,
    /// How many files it changed.
    pub files: usize,
    pub summary: serde_json::Value,
    pub state: OperationState,
    /// Whether [`restore`] would accept it now.
    pub restorable: bool,
}

/// What the restore rule needs to know about an operation.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: Uuid,
    pub kind: OperationKind,
    pub state: OperationState,
    pub files: Vec<FileChange>,
}

/// The backup folder of an operation: `.worldmap-backups/<YYYYMMDD-HHMMSS>-<kind>-<8 hex>`
/// (UTC time, the first 8 hex digits of the id).
pub fn backup_folder(created: DateTime<Utc>, kind: OperationKind, id: Uuid) -> String {
    let hex = id.simple().to_string();
    format!(
        "{BACKUPS}/{}-{}-{}",
        created.format("%Y%m%d-%H%M%S"),
        kind.name(),
        &hex[..8]
    )
}

/// When a backup folder (its name, without the parent) was created; `None` for names this module
/// does not make.
pub fn folder_time(name: &str) -> Option<DateTime<Utc>> {
    let stamp = name.get(..15)?;
    if name.as_bytes().get(15).is_some_and(|&byte| byte != b'-') {
        return None;
    }
    NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S")
        .ok()
        .map(|time| time.and_utc())
}

/// The backup folders (names of `.worldmap-backups` entries: `(name, directory)`) created before
/// `cutoff`.
pub fn expired_backups<'a>(
    entries: impl IntoIterator<Item = (&'a str, bool)>,
    cutoff: DateTime<Utc>,
) -> Vec<String> {
    entries
        .into_iter()
        .filter(|&(name, directory)| directory && folder_time(name).is_some_and(|t| t < cutoff))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The temporary file new content of `path` is written to: `<dir>/.<name>.worldmap-tmp`.
pub fn temp_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/.{name}{TEMP_SUFFIX}"),
        None => format!(".{path}{TEMP_SUFFIX}"),
    }
}

/// Where the backup of `path` goes in the backup folder `folder`.
pub fn backup_path(folder: &str, path: &str) -> String {
    format!("{folder}/{path}")
}

/// Where a restore moves the files it replaces: `<backup folder>/replaced`.
pub fn replaced_folder(folder: &str) -> String {
    format!("{folder}/replaced")
}

/// Refuses paths a writer must not touch: absolute, empty, `.`/`..` parts, backslashes, and
/// anything inside the backup folder.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("invalid file path {path:?}"));
    }
    if path == BACKUPS || path.starts_with(&format!("{BACKUPS}/")) {
        return Err(format!("{path} is inside the backup folder"));
    }
    Ok(())
}

/// The manifest written into the backup folder.
pub fn manifest(operation: &Operation, files: &[FileChange]) -> serde_json::Value {
    serde_json::json!({
        "id": operation.id,
        "kind": operation.kind,
        "user": operation.user,
        "created": operation.created,
        "created_at": DateTime::<Utc>::from_timestamp(operation.created, 0)
            .map(|time| time.to_rfc3339()),
        "world": operation.world,
        "dimension": operation.dimension,
        "state": operation.state,
        "summary": operation.summary,
        "files": files,
    })
}

/// The restore rule: only finished or partial delete/change/import operations with files, and
/// only when no newer operation (except restores, and restored or pruned ones) touched any of
/// the same files, since putting the old file back would undo that operation too.
pub fn check_restore(target: &Record, newer: &[Record]) -> Result<(), String> {
    match target.state {
        OperationState::Restored => return Err("the operation was already restored".into()),
        OperationState::Pruned => {
            return Err("the operation's backup was deleted after the retention period".into());
        }
        OperationState::Empty => return Err("the operation changed no files".into()),
        OperationState::Done | OperationState::Partial => {}
    }
    if target.kind == OperationKind::Restore {
        return Err("a restore cannot be restored".into());
    }
    if target.files.is_empty() {
        return Err("the operation changed no files".into());
    }
    let paths: HashSet<&str> = target.files.iter().map(|file| file.path.as_str()).collect();
    for other in newer {
        if other.id == target.id
            || other.kind == OperationKind::Restore
            || !matches!(other.state, OperationState::Done | OperationState::Partial)
        {
            continue;
        }
        if let Some(file) = other
            .files
            .iter()
            .find(|file| paths.contains(file.path.as_str()))
        {
            return Err(format!(
                "a newer operation ({}) changed {} too: restore it first",
                other.id, file.path
            ));
        }
    }
    Ok(())
}

/// The dimension folder of a region-type file path (`<folder>/region|entities|poi/<file>`).
fn dimension_folder(path: &str) -> Option<&str> {
    let (dir, _) = path.rsplit_once('/')?;
    let (folder, kind) = dir.rsplit_once('/')?;
    ["region", "entities", "poi"]
        .contains(&kind)
        .then_some(folder)
}

fn files_json(files: &[FileChange]) -> String {
    serde_json::to_string(files).unwrap_or_else(|_| "[]".into())
}

fn failed(message: impl AsRef<str>) -> ApiResponse {
    ApiResponse::error(message).with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

fn conflict(message: impl AsRef<str>) -> ApiResponse {
    ApiResponse::error(message).with_status(StatusCode::CONFLICT)
}

/// Refuses to go on unless the node reports the server offline right now.
async fn ensure_offline(wings: &Wings) -> Result<(), ApiResponse> {
    if Power::from(wings.power_state_fresh().await?) == Power::Offline {
        Ok(())
    } else {
        Err(conflict(
            "the server was started: stopped before changing more files (what was done so far can be restored)",
        ))
    }
}

static LOCKS: LazyLock<Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn server_lock(server: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    // locks nobody holds or waits for are only referenced by the map
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(server).or_default().clone()
}

/// The server's write lock: hold it around a whole write job (every [`Writer`] of it) and around
/// [`restore`], so two operations on one server never interleave. Not reentrant: nothing in this
/// module takes it.
pub async fn lock(server: Uuid) -> OwnedMutexGuard<()> {
    server_lock(server).lock_owned().await
}

/// The server's write lock when nobody holds it (for answering 409 instead of waiting).
pub fn try_lock(server: Uuid) -> Option<OwnedMutexGuard<()>> {
    server_lock(server).try_lock_owned().ok()
}

/// Writes files of one operation with backups (see the module docs).
pub struct Writer<'a> {
    map: &'a Map,
    id: Uuid,
    user: Uuid,
    kind: OperationKind,
    world: String,
    dimension: String,
    created: DateTime<Utc>,
    folder: String,
    files: Vec<FileChange>,
}

impl<'a> Writer<'a> {
    /// Starts an operation: checks that the server is offline and records the operation (state
    /// `partial` until it ends). Does not take [`lock`].
    pub async fn begin(
        map: &'a Map,
        user: Uuid,
        kind: OperationKind,
        world: &str,
        dimension: &str,
    ) -> Result<Self, ApiResponse> {
        ensure_offline(&map.wings).await?;
        let id = Uuid::new_v4();
        let created = Utc::now();
        let folder = backup_folder(created, kind, id);
        db::insert_operation(
            &map.database,
            map.server,
            &db::OperationRow {
                id,
                user: Some(user),
                username: None,
                kind: kind.name().into(),
                world: world.into(),
                dimension: dimension.into(),
                backup: folder.clone(),
                files: "[]".into(),
                summary: "{}".into(),
                state: OperationState::Partial.name().into(),
                created,
                restored: None,
            },
        )
        .await?;
        Ok(Self {
            map,
            id,
            user,
            kind,
            world: world.into(),
            dimension: dimension.into(),
            created,
            folder,
            files: Vec::new(),
        })
    }

    async fn record(&mut self, change: FileChange) -> Result<(), ApiResponse> {
        self.files.push(change);
        db::set_operation_files(&self.map.database, self.id, &files_json(&self.files)).await?;
        Ok(())
    }

    fn changed(&self, path: &str) -> bool {
        self.files.iter().any(|file| file.path == path)
    }

    /// Replaces (or creates) the file at `path` (relative to the server root, no leading `/`)
    /// with `data`, the original moved into the backup folder.
    pub async fn replace(&mut self, path: &str, data: Vec<u8>) -> Result<(), ApiResponse> {
        check_path(path).map_err(failed)?;
        ensure_offline(&self.map.wings).await?;
        let map: &'a Map = self.map;
        let wings = &map.wings;
        let temp = temp_path(path);
        wings.write(&format!("/{temp}"), self.user, data).await?;

        let backup = backup_path(&self.folder, path);
        // `None`: written before by this operation, whose backup already holds the original
        let existed = if self.changed(path) {
            wings.delete("/", &[path.to_string()]).await?;
            None
        } else {
            match wings.rename(path, &backup).await {
                Ok(moved) => Some(moved),
                Err(err) => {
                    let _ = wings.delete("/", &[temp.clone()]).await;
                    return Err(err);
                }
            }
        };

        let placed = wings.rename(&temp, path).await;
        if !matches!(placed, Ok(true)) {
            let mut lost = false;
            if existed == Some(true) && !matches!(wings.rename(&backup, path).await, Ok(true)) {
                lost = true;
            }
            let _ = wings.delete("/", &[temp]).await;
            if lost {
                // the original stays in the backup folder; recorded so a restore puts it back
                self.record(FileChange {
                    path: path.into(),
                    existed: true,
                })
                .await?;
            }
            return Err(match placed {
                Err(err) => err,
                Ok(_) => failed(format!("could not move the new {path} into place")),
            });
        }
        if let Some(existed) = existed {
            self.record(FileChange {
                path: path.into(),
                existed,
            })
            .await?;
        }
        Ok(())
    }

    /// Removes the file at `path` (moved into the backup folder); nothing when it is missing.
    pub async fn remove(&mut self, path: &str) -> Result<(), ApiResponse> {
        check_path(path).map_err(failed)?;
        ensure_offline(&self.map.wings).await?;
        if self.changed(path) {
            // written by this operation: the backup folder already holds the original
            self.map.wings.delete("/", &[path.to_string()]).await?;
            return Ok(());
        }
        if self
            .map
            .wings
            .rename(path, &backup_path(&self.folder, path))
            .await?
        {
            self.record(FileChange {
                path: path.into(),
                existed: true,
            })
            .await?;
        }
        Ok(())
    }

    fn operation(&self, state: OperationState, summary: serde_json::Value) -> Operation {
        Operation {
            id: self.id,
            kind: self.kind,
            created: self.created.timestamp(),
            user: Some(self.user),
            username: None,
            world: self.world.clone(),
            dimension: self.dimension.clone(),
            backup: self.folder.clone(),
            files: self.files.len(),
            summary,
            state,
            restorable: false,
        }
    }

    async fn end(
        self,
        state: OperationState,
        summary: serde_json::Value,
    ) -> Result<Operation, ApiResponse> {
        let map = self.map;
        if self.files.is_empty() {
            db::delete_operation(&map.database, self.id).await?;
            if let Some(name) = self.folder.strip_prefix(&format!("{BACKUPS}/"))
                && map
                    .wings
                    .delete(&format!("/{BACKUPS}"), &[name.to_string()])
                    .await
                    .is_err()
            {
                tracing::debug!(server = %map.server, "world map: empty backup folder {} not deleted", self.folder);
            }
            return Ok(self.operation(OperationState::Empty, summary));
        }
        let operation = self.operation(state, summary);
        let manifest = manifest(&operation, &self.files);
        if map
            .wings
            .write(
                &format!("/{}/{MANIFEST}", self.folder),
                self.user,
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            )
            .await
            .is_err()
        {
            tracing::warn!(server = %map.server, "world map: backup manifest of {} not written", self.folder);
        }
        db::finish_operation(
            &map.database,
            self.id,
            state.name(),
            &files_json(&self.files),
            &operation.summary.to_string(),
        )
        .await?;
        Ok(get(map, self.id).await.unwrap_or(Operation {
            restorable: true,
            ..operation
        }))
    }

    /// Ends the operation: manifest written, state `done`. With no changed file the row and the
    /// backup folder are removed and the result has state `empty`.
    pub async fn finish(self, summary: serde_json::Value) -> Result<Operation, ApiResponse> {
        self.end(OperationState::Done, summary).await
    }

    /// Ends a failed or cancelled operation: what was done is kept (restorable), state `partial`,
    /// the summary holds `error`. With no changed file, as [`Writer::finish`].
    pub async fn abort(self, error: &str) -> Result<Operation, ApiResponse> {
        self.end(
            OperationState::Partial,
            serde_json::json!({ "error": error }),
        )
        .await
    }
}

fn to_record(row: &db::OperationRow) -> Option<Record> {
    Some(Record {
        id: row.id,
        kind: OperationKind::parse(&row.kind)?,
        state: OperationState::parse(&row.state)?,
        files: serde_json::from_str(&row.files).unwrap_or_default(),
    })
}

fn operation(row: db::OperationRow, record: &Record, restorable: bool) -> Operation {
    Operation {
        id: row.id,
        kind: record.kind,
        created: row.created.timestamp(),
        user: row.user,
        username: row.username,
        world: row.world,
        dimension: row.dimension,
        backup: row.backup,
        files: record.files.len(),
        summary: serde_json::from_str(&row.summary).unwrap_or(serde_json::Value::Null),
        state: record.state,
        restorable,
    }
}

/// The server's operations, newest first (at most `limit`, 1..=1000), with `restorable` per
/// [`check_restore`].
pub async fn list(map: &Map, limit: i64) -> Result<Vec<Operation>, ApiResponse> {
    let rows = db::operations(&map.database, map.server, limit.clamp(1, 1000)).await?;
    let mut newer: Vec<Record> = Vec::new();
    let mut operations = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(record) = to_record(&row) else {
            continue;
        };
        // rows come newest first: everything before this one is newer
        let restorable = check_restore(&record, &newer).is_ok();
        operations.push(operation(row, &record, restorable));
        newer.push(record);
    }
    Ok(operations)
}

async fn load(map: &Map, id: Uuid) -> Result<(db::OperationRow, Record, Vec<Record>), ApiResponse> {
    let row = db::operation(&map.database, map.server, id)
        .await?
        .ok_or_else(|| crate::map::not_found("operation not found"))?;
    let record = to_record(&row).ok_or_else(|| failed("the operation's record is unreadable"))?;
    let newer: Vec<Record> = db::operations_after(&map.database, map.server, row.created)
        .await?
        .iter()
        .filter_map(to_record)
        .collect();
    Ok((row, record, newer))
}

/// One operation of the server (404 when it is not one of its operations).
pub async fn get(map: &Map, id: Uuid) -> Result<Operation, ApiResponse> {
    let (row, record, newer) = load(map, id).await?;
    let restorable = check_restore(&record, &newer).is_ok();
    Ok(operation(row, &record, restorable))
}

/// Puts back one file of a restored operation; whether its backup was found.
async fn restore_file(
    wings: &Wings,
    backup: &str,
    replaced: &str,
    file: &FileChange,
    changes: &mut Vec<FileChange>,
) -> Result<bool, ApiResponse> {
    ensure_offline(wings).await?;
    let aside = backup_path(replaced, &file.path);
    let moved = wings.rename(&file.path, &aside).await?;
    if file.existed {
        let back = wings
            .rename(&backup_path(backup, &file.path), &file.path)
            .await;
        if !matches!(back, Ok(true)) {
            if moved && !matches!(wings.rename(&aside, &file.path).await, Ok(true)) {
                // the current file stays in `replaced/`; recorded so the history shows it
                changes.push(FileChange {
                    path: file.path.clone(),
                    existed: true,
                });
            }
            back?;
            return Ok(false);
        }
    }
    if moved || file.existed {
        changes.push(FileChange {
            path: file.path.clone(),
            existed: moved,
        });
    }
    Ok(true)
}

/// Restores operation `id` (refused per [`check_restore`]): the current files are moved into
/// `<backup>/replaced/`, the backups moved back, files the operation created moved into
/// `<backup>/replaced/`; a fresh power check before each file. Records a `restore` operation
/// (returned; its backup folder is `<backup>/replaced`) and marks the original `restored`.
/// Files whose backup is missing are kept as they are and listed in the summary's `missing`.
/// Does not take [`lock`].
pub async fn restore(map: &Map, user: Uuid, id: Uuid) -> Result<Operation, ApiResponse> {
    let (row, target, newer) = load(map, id).await?;
    check_restore(&target, &newer).map_err(conflict)?;
    ensure_offline(&map.wings).await?;

    let restore_id = Uuid::new_v4();
    let created = Utc::now();
    let replaced = replaced_folder(&row.backup);
    db::insert_operation(
        &map.database,
        map.server,
        &db::OperationRow {
            id: restore_id,
            user: Some(user),
            username: None,
            kind: OperationKind::Restore.name().into(),
            world: row.world.clone(),
            dimension: row.dimension.clone(),
            backup: replaced.clone(),
            files: "[]".into(),
            summary: serde_json::json!({ "restores": id }).to_string(),
            state: OperationState::Partial.name().into(),
            created,
            restored: None,
        },
    )
    .await?;

    let mut changes = Vec::new();
    let mut missing = Vec::new();
    let mut error = None;
    for file in target.files.iter().rev() {
        match restore_file(&map.wings, &row.backup, &replaced, file, &mut changes).await {
            Ok(true) => {}
            Ok(false) => missing.push(file.path.clone()),
            Err(err) => {
                error = Some(err);
                break;
            }
        }
        if target.state == OperationState::Partial {
            // a crashed write may have left its temporary file behind
            let _ = map.wings.delete("/", &[temp_path(&file.path)]).await;
        }
        db::set_operation_files(&map.database, restore_id, &files_json(&changes)).await?;
    }
    let folders: BTreeSet<&str> = target
        .files
        .iter()
        .filter_map(|file| dimension_folder(&file.path))
        .collect();
    for folder in folders {
        map.forget(folder);
    }

    let state = if error.is_some() {
        OperationState::Partial
    } else {
        OperationState::Done
    };
    let mut summary = serde_json::json!({
        "restores": id,
        "restored_kind": target.kind,
        "files": changes.len(),
        "missing": missing,
    });
    if error.is_some() {
        summary["error"] = "the restore stopped midway; restoring again finishes it".into();
    }
    let operation = Operation {
        id: restore_id,
        kind: OperationKind::Restore,
        created: created.timestamp(),
        user: Some(user),
        username: None,
        world: row.world.clone(),
        dimension: row.dimension.clone(),
        backup: replaced.clone(),
        files: changes.len(),
        summary,
        state,
        restorable: false,
    };
    if !changes.is_empty()
        && map
            .wings
            .write(
                &format!("/{replaced}/{MANIFEST}"),
                user,
                serde_json::to_vec_pretty(&manifest(&operation, &changes)).unwrap_or_default(),
            )
            .await
            .is_err()
    {
        tracing::warn!(server = %map.server, "world map: restore manifest of {replaced} not written");
    }
    db::finish_operation(
        &map.database,
        restore_id,
        state.name(),
        &files_json(&changes),
        &operation.summary.to_string(),
    )
    .await?;
    if let Some(err) = error {
        return Err(err);
    }
    db::set_operation_restored(&map.database, id, Utc::now()).await?;
    Ok(get(map, restore_id).await.unwrap_or(operation))
}

/// Deletes, on every server the map knows (opened maps and servers with operations), backup
/// folders older than `backup_retention_days` (rows marked `pruned`) and import staging folders
/// unchanged for [`IMPORT_RETENTION_HOURS`]. Servers whose write lock is held or whose node
/// cannot be reached are skipped until the next run. The number of folders deleted.
pub async fn prune(state: &State) -> Result<u64, anyhow::Error> {
    let settings = settings::load(state).await?;
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(i64::from(settings.backup_retention_days.max(1)));
    let import_cutoff = now - chrono::Duration::hours(IMPORT_RETENTION_HOURS);
    let mut servers: BTreeSet<Uuid> = db::tracked(&state.database)
        .await?
        .into_iter()
        .map(|(server, _)| server)
        .collect();
    servers.extend(db::operation_servers(&state.database).await?);
    let mut deleted = 0;
    for server in servers {
        match prune_server(state, server, cutoff, import_cutoff).await {
            Ok(count) => deleted += count,
            Err(err) => tracing::debug!(%server, "world map backups: {err:#}"),
        }
    }
    Ok(deleted)
}

async fn prune_server(
    state: &State,
    server: Uuid,
    cutoff: DateTime<Utc>,
    import_cutoff: DateTime<Utc>,
) -> Result<u64, anyhow::Error> {
    // deleted servers lose their rows by cascade
    let Ok(model) = Server::by_uuid(&state.database, server).await else {
        return Ok(0);
    };
    let Some(_lock) = try_lock(server) else {
        return Ok(0);
    };
    let unreachable =
        |what: &'static str| move |_: ApiResponse| anyhow::anyhow!("could not {what}");
    let wings = Wings::connect(state, &model)
        .await
        .map_err(unreachable("reach the server"))?;

    let backups = wings
        .list(&format!("/{BACKUPS}"))
        .await
        .map_err(unreachable("list the backups"))?
        .unwrap_or_default();
    let expired = expired_backups(
        backups
            .iter()
            .map(|entry| (entry.name.as_str(), entry.directory)),
        cutoff,
    );
    let mut deleted = wings
        .delete(&format!("/{BACKUPS}"), &expired)
        .await
        .map_err(unreachable("delete old backups"))?;
    db::prune_operations(&state.database, server, cutoff).await?;

    let imports: Vec<String> = wings
        .list(&format!("/{IMPORTS}"))
        .await
        .map_err(unreachable("list the import folders"))?
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.directory && entry.modified.timestamp() < import_cutoff.timestamp())
        .map(|entry| entry.name.to_string())
        .collect();
    deleted += wings
        .delete(&format!("/{IMPORTS}"), &imports)
        .await
        .map_err(unreachable("delete old import folders"))?;
    if deleted > 0 {
        tracing::info!(%server, "world map: deleted {deleted} old backup and import folders");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn change(path: &str, existed: bool) -> FileChange {
        FileChange {
            path: path.into(),
            existed,
        }
    }

    fn rec(id: u128, kind: OperationKind, state: OperationState, paths: &[&str]) -> Record {
        Record {
            id: Uuid::from_u128(id),
            kind,
            state,
            files: paths.iter().map(|path| change(path, true)).collect(),
        }
    }

    #[test]
    fn backup_folders_are_named_by_time_kind_and_id() {
        let created = Utc.with_ymd_and_hms(2026, 11, 24, 9, 5, 7).unwrap();
        let id = Uuid::parse_str("0123abcd-0000-4000-8000-000000000000").unwrap();
        let folder = backup_folder(created, OperationKind::Change, id);
        assert_eq!(folder, ".worldmap-backups/20261124-090507-change-0123abcd");
        let name = folder.strip_prefix(".worldmap-backups/").unwrap();
        assert_eq!(folder_time(name), Some(created));
        assert_eq!(folder_time("notes.txt"), None);
        assert_eq!(folder_time("20261124-090507x"), None);
        assert_eq!(
            backup_path(&folder, "world/region/r.0.0.mca"),
            ".worldmap-backups/20261124-090507-change-0123abcd/world/region/r.0.0.mca"
        );
        assert_eq!(
            replaced_folder(&folder),
            ".worldmap-backups/20261124-090507-change-0123abcd/replaced"
        );
    }

    #[test]
    fn only_old_backup_folders_expire() {
        let cutoff = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
        let entries = [
            ("20261101-000000-delete-00000000", true),
            ("20261125-000000-import-00000000", true),
            ("20261101-000000-delete-11111111", false),
            ("random", true),
        ];
        assert_eq!(
            expired_backups(entries, cutoff),
            vec!["20261101-000000-delete-00000000".to_string()]
        );
    }

    #[test]
    fn temporary_files_sit_next_to_the_original() {
        assert_eq!(
            temp_path("world/region/r.0.0.mca"),
            "world/region/.r.0.0.mca.worldmap-tmp"
        );
        assert_eq!(temp_path("r.0.0.mca"), ".r.0.0.mca.worldmap-tmp");
    }

    #[test]
    fn writers_refuse_odd_paths() {
        assert!(check_path("world/region/r.0.0.mca").is_ok());
        for bad in [
            "",
            "/world/region/r.0.0.mca",
            "world//r.0.0.mca",
            "world/../etc",
            "./world",
            "world/region/",
            "world\\region",
            ".worldmap-backups",
            ".worldmap-backups/x/world/region/r.0.0.mca",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn manifests_list_the_operation_and_its_files() {
        let operation = Operation {
            id: Uuid::from_u128(1),
            kind: OperationKind::Delete,
            created: 1_800_000_000,
            user: Some(Uuid::from_u128(2)),
            username: None,
            world: "world".into(),
            dimension: "minecraft:overworld".into(),
            backup: ".worldmap-backups/x".into(),
            files: 2,
            summary: serde_json::json!({ "chunks": 5 }),
            state: OperationState::Done,
            restorable: true,
        };
        let files = [
            change("world/region/r.0.0.mca", true),
            change("world/region/r.1.0.mca", false),
        ];
        let manifest = manifest(&operation, &files);
        assert_eq!(manifest["kind"], "delete");
        assert_eq!(manifest["state"], "done");
        assert_eq!(manifest["world"], "world");
        assert_eq!(manifest["dimension"], "minecraft:overworld");
        assert_eq!(manifest["created"], 1_800_000_000);
        assert_eq!(manifest["user"], Uuid::from_u128(2).to_string());
        assert_eq!(manifest["summary"]["chunks"], 5);
        assert_eq!(manifest["files"][1]["path"], "world/region/r.1.0.mca");
        assert_eq!(manifest["files"][1]["existed"], false);
        assert!(
            manifest["created_at"]
                .as_str()
                .unwrap()
                .starts_with("2027-01-15")
        );
    }

    #[test]
    fn kinds_and_states_round_trip_their_names() {
        for kind in [
            OperationKind::Delete,
            OperationKind::Change,
            OperationKind::Import,
            OperationKind::Restore,
        ] {
            assert_eq!(OperationKind::parse(kind.name()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::from(kind.name())
            );
        }
        for state in [
            OperationState::Done,
            OperationState::Partial,
            OperationState::Restored,
            OperationState::Pruned,
            OperationState::Empty,
        ] {
            assert_eq!(OperationState::parse(state.name()), Some(state));
        }
        assert_eq!(OperationKind::parse("other"), None);
    }

    #[test]
    fn restores_are_refused_when_a_newer_operation_touched_the_files() {
        use OperationKind::*;
        use OperationState::*;
        let target = rec(
            1,
            Delete,
            Done,
            &["w/region/r.0.0.mca", "w/region/r.1.0.mca"],
        );
        assert!(check_restore(&target, &[]).is_ok());
        // a newer change of another file does not matter
        assert!(check_restore(&target, &[rec(2, Change, Done, &["w/region/r.5.5.mca"])]).is_ok());
        // a newer change of the same file does, also when partial
        for state in [Done, Partial] {
            let err = check_restore(&target, &[rec(2, Change, state, &["w/region/r.1.0.mca"])])
                .unwrap_err();
            assert!(err.contains("r.1.0.mca"), "{err}");
        }
        // unless it was restored or pruned, or is itself a restore
        for newer in [
            rec(2, Change, Restored, &["w/region/r.1.0.mca"]),
            rec(3, Import, Pruned, &["w/region/r.1.0.mca"]),
            rec(4, Restore, Done, &["w/region/r.1.0.mca"]),
        ] {
            assert!(check_restore(&target, &[newer]).is_ok());
        }
        // only restorable operations
        assert!(check_restore(&rec(1, Delete, Restored, &["a/b"]), &[]).is_err());
        assert!(check_restore(&rec(1, Delete, Pruned, &["a/b"]), &[]).is_err());
        assert!(check_restore(&rec(1, Restore, Done, &["a/b"]), &[]).is_err());
        assert!(check_restore(&rec(1, Import, Partial, &[]), &[]).is_err());
        assert!(check_restore(&rec(1, Import, Partial, &["a/b"]), &[]).is_ok());
        // the target itself in the list is ignored
        assert!(check_restore(&target, std::slice::from_ref(&target)).is_ok());
    }

    #[test]
    fn region_paths_name_their_dimension_folder() {
        assert_eq!(dimension_folder("world/region/r.0.0.mca"), Some("world"));
        assert_eq!(
            dimension_folder("world_nether/DIM-1/entities/r.0.0.mca"),
            Some("world_nether/DIM-1")
        );
        assert_eq!(dimension_folder("world/level.dat"), None);
    }

    #[tokio::test]
    async fn the_server_lock_is_exclusive_per_server() {
        let server = Uuid::from_u128(0xfeed);
        let held = lock(server).await;
        assert!(try_lock(server).is_none());
        assert!(try_lock(Uuid::from_u128(0xbeef)).is_some());
        drop(held);
        assert!(try_lock(server).is_some());
    }
}
