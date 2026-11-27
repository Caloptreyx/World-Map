//! The extension's tables: rendered tiles and 3D surfaces, servers whose map was opened (with
//! their per-server settings), heatmap samples, chunk tool operations and cached overlay values.
use crate::players::Visibility;
use chrono::{DateTime, NaiveDate, Utc};
use shared::database::Database;
use uuid::Uuid;

pub struct Tile {
    pub modified: i64,
    pub size: i64,
    pub variant: String,
    pub image: Vec<u8>,
}

pub async fn tile(db: &Database, server: Uuid, path: &str) -> Result<Option<Tile>, sqlx::Error> {
    let row: Option<(i64, i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT modified, size, variant, image FROM dev_caloptreyx_worldmap_tiles
         WHERE server_uuid = $1 AND path = $2",
    )
    .bind(server)
    .bind(path)
    .fetch_optional(db.read())
    .await?;
    Ok(row.map(|(modified, size, variant, image)| Tile {
        modified,
        size,
        variant,
        image,
    }))
}

/// Marks a tile as viewed (at most once an hour), for the cache's least-recently-used pruning.
pub async fn touch_tile(db: &Database, server: Uuid, path: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_tiles SET used = now()
         WHERE server_uuid = $1 AND path = $2 AND used < now() - INTERVAL '1 hour'",
    )
    .bind(server)
    .bind(path)
    .execute(db.write())
    .await?;
    Ok(())
}

pub async fn store_tile(
    db: &Database,
    server: Uuid,
    path: &str,
    tile: &Tile,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_tiles (server_uuid, path, modified, size, variant, image)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (server_uuid, path) DO UPDATE SET
             modified = EXCLUDED.modified, size = EXCLUDED.size, variant = EXCLUDED.variant,
             image = EXCLUDED.image, used = now()",
    )
    .bind(server)
    .bind(path)
    .bind(tile.modified)
    .bind(tile.size)
    .bind(&tile.variant)
    .bind(&tile.image)
    .execute(db.write())
    .await?;
    Ok(())
}

/// A cached 3D surface of a region file (zlib-compressed, see `surface`).
pub struct Surface {
    pub modified: i64,
    pub size: i64,
    pub variant: String,
    pub data: Vec<u8>,
}

pub async fn surface(db: &Database, server: Uuid, path: &str) -> Result<Option<Surface>, sqlx::Error> {
    let row: Option<(i64, i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT modified, size, variant, data FROM dev_caloptreyx_worldmap_surfaces
         WHERE server_uuid = $1 AND path = $2",
    )
    .bind(server)
    .bind(path)
    .fetch_optional(db.read())
    .await?;
    Ok(row.map(|(modified, size, variant, data)| Surface {
        modified,
        size,
        variant,
        data,
    }))
}

/// Marks a surface as viewed (at most once an hour), for the cache's least-recently-used pruning.
pub async fn touch_surface(db: &Database, server: Uuid, path: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_surfaces SET used = now()
         WHERE server_uuid = $1 AND path = $2 AND used < now() - INTERVAL '1 hour'",
    )
    .bind(server)
    .bind(path)
    .execute(db.write())
    .await?;
    Ok(())
}

pub async fn store_surface(
    db: &Database,
    server: Uuid,
    path: &str,
    surface: &Surface,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_surfaces (server_uuid, path, modified, size, variant, data)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (server_uuid, path) DO UPDATE SET
             modified = EXCLUDED.modified, size = EXCLUDED.size, variant = EXCLUDED.variant,
             data = EXCLUDED.data, used = now()",
    )
    .bind(server)
    .bind(path)
    .bind(surface.modified)
    .bind(surface.size)
    .bind(&surface.variant)
    .bind(&surface.data)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Deletes the least recently viewed tiles and 3D surfaces beyond `budget` bytes of both
/// together (one least-recently-used order over the two tables); returns how many tiles and
/// how many surfaces went.
pub async fn prune_cache(db: &Database, budget: i64) -> Result<(u64, u64), sqlx::Error> {
    let (tiles, surfaces): (i64, i64) = sqlx::query_as(
        "WITH ranked AS (
             SELECT kind, server_uuid, path,
                 SUM(bytes) OVER (ORDER BY used DESC, kind, server_uuid, path) AS total
             FROM (
                 SELECT 'tile' AS kind, server_uuid, path, used, octet_length(image) AS bytes
                 FROM dev_caloptreyx_worldmap_tiles
                 UNION ALL
                 SELECT 'surface', server_uuid, path, used, octet_length(data)
                 FROM dev_caloptreyx_worldmap_surfaces
             ) AS cached
         ), old AS (
             SELECT kind, server_uuid, path FROM ranked WHERE total > $1
         ), deleted_tiles AS (
             DELETE FROM dev_caloptreyx_worldmap_tiles AS tiles USING old
             WHERE old.kind = 'tile' AND tiles.server_uuid = old.server_uuid AND tiles.path = old.path
             RETURNING 1
         ), deleted_surfaces AS (
             DELETE FROM dev_caloptreyx_worldmap_surfaces AS surfaces USING old
             WHERE old.kind = 'surface' AND surfaces.server_uuid = old.server_uuid
                 AND surfaces.path = old.path
             RETURNING 1
         )
         SELECT (SELECT COUNT(*) FROM deleted_tiles), (SELECT COUNT(*) FROM deleted_surfaces)",
    )
    .bind(budget)
    .fetch_one(db.write())
    .await?;
    Ok((tiles as u64, surfaces as u64))
}

/// Remembers that the server's map was opened, so its heatmap is sampled.
pub async fn track(db: &Database, server: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_servers (server_uuid) VALUES ($1)
         ON CONFLICT (server_uuid) DO UPDATE SET opened = now()",
    )
    .bind(server)
    .execute(db.write())
    .await?;
    Ok(())
}

pub async fn tracked(db: &Database) -> Result<Vec<(Uuid, Option<DateTime<Utc>>)>, sqlx::Error> {
    sqlx::query_as("SELECT server_uuid, sampled FROM dev_caloptreyx_worldmap_servers")
        .fetch_all(db.read())
        .await
}

pub async fn set_sampled(db: &Database, server: Uuid, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dev_caloptreyx_worldmap_servers SET sampled = $2 WHERE server_uuid = $1")
        .bind(server)
        .bind(at)
        .execute(db.write())
        .await?;
    Ok(())
}

/// The world folders added by hand for the server.
pub async fn extra_worlds(db: &Database, server: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let row: Option<(Vec<String>,)> = sqlx::query_as(
        "SELECT extra_worlds FROM dev_caloptreyx_worldmap_servers WHERE server_uuid = $1",
    )
    .bind(server)
    .fetch_optional(db.read())
    .await?;
    Ok(row.map(|(paths,)| paths).unwrap_or_default())
}

pub async fn set_extra_worlds(db: &Database, server: Uuid, paths: &[String]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_servers (server_uuid, extra_worlds) VALUES ($1, $2)
         ON CONFLICT (server_uuid) DO UPDATE SET extra_worlds = EXCLUDED.extra_worlds",
    )
    .bind(server)
    .bind(paths)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Which players the map shows on the server; both until someone changes it.
pub async fn player_visibility(db: &Database, server: Uuid) -> Result<Visibility, sqlx::Error> {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT show_online_players, show_offline_players FROM dev_caloptreyx_worldmap_servers
         WHERE server_uuid = $1",
    )
    .bind(server)
    .fetch_optional(db.read())
    .await?;
    Ok(row
        .map(|(online, offline)| Visibility { online, offline })
        .unwrap_or_default())
}

pub async fn set_player_visibility(
    db: &Database,
    server: Uuid,
    visibility: Visibility,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_servers
             (server_uuid, show_online_players, show_offline_players) VALUES ($1, $2, $3)
         ON CONFLICT (server_uuid) DO UPDATE SET
             show_online_players = EXCLUDED.show_online_players,
             show_offline_players = EXCLUDED.show_offline_players",
    )
    .bind(server)
    .bind(visibility.online)
    .bind(visibility.offline)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Adds one sample per entry of `chunks` (`(x, z)`, repeats count twice).
pub async fn add_heat(
    db: &Database,
    server: Uuid,
    world: &str,
    dimension: &str,
    day: NaiveDate,
    chunks: &[(i32, i32)],
) -> Result<(), sqlx::Error> {
    if chunks.is_empty() {
        return Ok(());
    }
    let xs: Vec<i32> = chunks.iter().map(|&(x, _)| x).collect();
    let zs: Vec<i32> = chunks.iter().map(|&(_, z)| z).collect();
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_heat
             (server_uuid, world, dimension, day, chunk_x, chunk_z, samples)
         SELECT $1, $2, $3, $4, x, z, COUNT(*) FROM UNNEST($5::INTEGER[], $6::INTEGER[]) AS c(x, z)
         GROUP BY x, z
         ON CONFLICT (server_uuid, world, dimension, day, chunk_x, chunk_z) DO UPDATE
             SET samples = dev_caloptreyx_worldmap_heat.samples + EXCLUDED.samples",
    )
    .bind(server)
    .bind(world)
    .bind(dimension)
    .bind(day)
    .bind(xs)
    .bind(zs)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Samples per chunk since `since`, the busiest `limit` chunks.
pub async fn heat(
    db: &Database,
    server: Uuid,
    world: &str,
    dimension: &str,
    since: NaiveDate,
    limit: i64,
) -> Result<Vec<(i32, i32, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT chunk_x, chunk_z, SUM(samples)::BIGINT AS total FROM dev_caloptreyx_worldmap_heat
         WHERE server_uuid = $1 AND world = $2 AND dimension = $3 AND day >= $4
         GROUP BY chunk_x, chunk_z ORDER BY total DESC LIMIT $5",
    )
    .bind(server)
    .bind(world)
    .bind(dimension)
    .bind(since)
    .bind(limit)
    .fetch_all(db.read())
    .await
}

pub async fn reset_heat(db: &Database, server: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM dev_caloptreyx_worldmap_heat WHERE server_uuid = $1")
        .bind(server)
        .execute(db.write())
        .await?;
    Ok(result.rows_affected())
}

pub async fn prune_heat(db: &Database, before: NaiveDate) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM dev_caloptreyx_worldmap_heat WHERE day < $1")
        .bind(before)
        .execute(db.write())
        .await?;
    Ok(result.rows_affected())
}

/// A chunk tool write operation (see `ops`). `files` and `summary` are JSON text; the table
/// stores them as JSONB.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationRow {
    pub id: Uuid,
    pub user: Option<Uuid>,
    /// The user's name, filled in by the queries that read rows.
    pub username: Option<String>,
    pub kind: String,
    pub world: String,
    pub dimension: String,
    pub backup: String,
    pub files: String,
    pub summary: String,
    pub state: String,
    pub created: DateTime<Utc>,
    pub restored: Option<DateTime<Utc>>,
}

type OperationTuple = (
    Uuid,
    Option<Uuid>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

const OPERATION_COLUMNS: &str = "o.id, o.user_uuid, u.username, o.kind, o.world, o.dimension, o.backup,
     o.files::text, o.summary::text, o.state, o.created, o.restored
     FROM dev_caloptreyx_worldmap_operations AS o LEFT JOIN users AS u ON u.uuid = o.user_uuid";

fn operation_row(row: OperationTuple) -> OperationRow {
    let (id, user, username, kind, world, dimension, backup, files, summary, state, created, restored) =
        row;
    OperationRow {
        id,
        user,
        username,
        kind,
        world,
        dimension,
        backup,
        files,
        summary,
        state,
        created,
        restored,
    }
}

pub async fn insert_operation(
    db: &Database,
    server: Uuid,
    row: &OperationRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_operations
             (id, server_uuid, user_uuid, kind, world, dimension, backup, files, summary, state,
              created, restored)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9::jsonb, $10, $11, $12)",
    )
    .bind(row.id)
    .bind(server)
    .bind(row.user)
    .bind(&row.kind)
    .bind(&row.world)
    .bind(&row.dimension)
    .bind(&row.backup)
    .bind(&row.files)
    .bind(&row.summary)
    .bind(&row.state)
    .bind(row.created)
    .bind(row.restored)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Records the files an operation changed so far (JSON text).
pub async fn set_operation_files(db: &Database, id: Uuid, files: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dev_caloptreyx_worldmap_operations SET files = $2::jsonb WHERE id = $1")
        .bind(id)
        .bind(files)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Sets how an operation ended: its state, files and summary (JSON text).
pub async fn finish_operation(
    db: &Database,
    id: Uuid,
    state: &str,
    files: &str,
    summary: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_operations
         SET state = $2, files = $3::jsonb, summary = $4::jsonb WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(files)
    .bind(summary)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Marks an operation restored.
pub async fn set_operation_restored(
    db: &Database,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_operations SET state = 'restored', restored = $2
         WHERE id = $1",
    )
    .bind(id)
    .bind(at)
    .execute(db.write())
    .await?;
    Ok(())
}

pub async fn delete_operation(db: &Database, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM dev_caloptreyx_worldmap_operations WHERE id = $1")
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// The server's newest operations first.
pub async fn operations(
    db: &Database,
    server: Uuid,
    limit: i64,
) -> Result<Vec<OperationRow>, sqlx::Error> {
    let rows: Vec<OperationTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {OPERATION_COLUMNS} WHERE o.server_uuid = $1 ORDER BY o.created DESC, o.id LIMIT $2"
    )))
    .bind(server)
    .bind(limit)
    .fetch_all(db.read())
    .await?;
    Ok(rows.into_iter().map(operation_row).collect())
}

pub async fn operation(
    db: &Database,
    server: Uuid,
    id: Uuid,
) -> Result<Option<OperationRow>, sqlx::Error> {
    let row: Option<OperationTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {OPERATION_COLUMNS} WHERE o.server_uuid = $1 AND o.id = $2"
    )))
    .bind(server)
    .bind(id)
    .fetch_optional(db.read())
    .await?;
    Ok(row.map(operation_row))
}

/// The server's operations created after `after`, newest first.
pub async fn operations_after(
    db: &Database,
    server: Uuid,
    after: DateTime<Utc>,
) -> Result<Vec<OperationRow>, sqlx::Error> {
    let rows: Vec<OperationTuple> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {OPERATION_COLUMNS} WHERE o.server_uuid = $1 AND o.created > $2
         ORDER BY o.created DESC, o.id"
    )))
    .bind(server)
    .bind(after)
    .fetch_all(db.read())
    .await?;
    Ok(rows.into_iter().map(operation_row).collect())
}

/// Servers with operations whose backups are not pruned yet.
pub async fn operation_servers(db: &Database) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT server_uuid FROM dev_caloptreyx_worldmap_operations WHERE state <> 'pruned'",
    )
    .fetch_all(db.read())
    .await?;
    Ok(rows.into_iter().map(|(server,)| server).collect())
}

/// Marks the server's operations created before `before` as pruned (their backups are gone).
pub async fn prune_operations(
    db: &Database,
    server: Uuid,
    before: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_operations SET state = 'pruned'
         WHERE server_uuid = $1 AND created < $2 AND state <> 'pruned'",
    )
    .bind(server)
    .bind(before)
    .execute(db.write())
    .await?;
    Ok(result.rows_affected())
}

/// Values per chunk of an overlay in a region (`x + z * 32`; `None` = no value).
pub const OVERLAY_VALUES: usize = 1024;

/// Encodes overlay values as stored: 1024 little-endian f64, NaN for `None` (a NaN value reads
/// back as `None`). Missing trailing values are stored as `None`.
pub fn encode_overlay(values: &[Option<f64>]) -> Vec<u8> {
    let mut data = Vec::with_capacity(OVERLAY_VALUES * 8);
    for i in 0..OVERLAY_VALUES {
        let value = values.get(i).copied().flatten().unwrap_or(f64::NAN);
        data.extend_from_slice(&value.to_le_bytes());
    }
    data
}

/// Decodes stored overlay values; `None` when the data has the wrong length.
pub fn decode_overlay(data: &[u8]) -> Option<Vec<Option<f64>>> {
    if data.len() != OVERLAY_VALUES * 8 {
        return None;
    }
    Some(
        data.chunks_exact(8)
            .map(|raw| {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(raw);
                let value = f64::from_le_bytes(bytes);
                (!value.is_nan()).then_some(value)
            })
            .collect(),
    )
}

/// The cached overlay `key` of the region at `path`: its version (valid while it matches the
/// region's files) and values. Rows with undecodable data count as missing.
pub async fn overlay(
    db: &Database,
    server: Uuid,
    path: &str,
    key: &str,
) -> Result<Option<(String, Vec<Option<f64>>)>, sqlx::Error> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT version, data FROM dev_caloptreyx_worldmap_overlays
         WHERE server_uuid = $1 AND path = $2 AND overlay = $3",
    )
    .bind(server)
    .bind(path)
    .bind(key)
    .fetch_optional(db.read())
    .await?;
    Ok(row.and_then(|(version, data)| decode_overlay(&data).map(|values| (version, values))))
}

/// Caches the overlay `key` of the region at `path` (see [`encode_overlay`]).
pub async fn store_overlay(
    db: &Database,
    server: Uuid,
    path: &str,
    key: &str,
    version: &str,
    values: &[Option<f64>],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dev_caloptreyx_worldmap_overlays (server_uuid, path, overlay, version, data)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (server_uuid, path, overlay) DO UPDATE SET
             version = EXCLUDED.version, data = EXCLUDED.data, used = now()",
    )
    .bind(server)
    .bind(path)
    .bind(key)
    .bind(version)
    .bind(encode_overlay(values))
    .execute(db.write())
    .await?;
    Ok(())
}

/// Marks a cached overlay as viewed (at most once an hour), for least-recently-used pruning.
pub async fn touch_overlay(
    db: &Database,
    server: Uuid,
    path: &str,
    overlay: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dev_caloptreyx_worldmap_overlays SET used = now()
         WHERE server_uuid = $1 AND path = $2 AND overlay = $3 AND used < now() - INTERVAL '1 hour'",
    )
    .bind(server)
    .bind(path)
    .bind(overlay)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Bytes of cached overlay values kept panel-wide (8 KiB per region and overlay: 32768 of them).
pub const OVERLAY_CACHE_BYTES: i64 = 256 << 20;

/// Deletes the least recently viewed cached overlays beyond `budget` bytes of data.
pub async fn prune_overlays(db: &Database, budget: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM dev_caloptreyx_worldmap_overlays AS overlays USING (
             SELECT server_uuid, path, overlay FROM (
                 SELECT server_uuid, path, overlay,
                     SUM(octet_length(data)) OVER (ORDER BY used DESC, server_uuid, path, overlay) AS total
                 FROM dev_caloptreyx_worldmap_overlays
             ) AS ranked WHERE total > $1
         ) AS old
         WHERE overlays.server_uuid = old.server_uuid AND overlays.path = old.path
             AND overlays.overlay = old.overlay",
    )
    .bind(budget)
    .execute(db.write())
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_values_round_trip_with_nan_as_none() {
        let mut values = vec![None; OVERLAY_VALUES];
        values[0] = Some(1.5);
        values[33] = Some(-7.0);
        values[1023] = Some(0.0);
        let data = encode_overlay(&values);
        assert_eq!(data.len(), 8192);
        assert_eq!(&data[0..8], &1.5f64.to_le_bytes());
        assert_eq!(decode_overlay(&data).unwrap(), values);
        // short input is padded with None, a NaN value reads as None
        let short = encode_overlay(&[Some(f64::NAN), Some(2.0)]);
        let decoded = decode_overlay(&short).unwrap();
        assert_eq!(decoded[0], None);
        assert_eq!(decoded[1], Some(2.0));
        assert_eq!(decoded[2], None);
        assert!(decode_overlay(&data[..100]).is_none());
    }
}
