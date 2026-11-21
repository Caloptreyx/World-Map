//! The extension's tables: rendered tiles, servers whose map was opened, heatmap samples.
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

/// Deletes the least recently viewed tiles beyond `budget` bytes of images.
pub async fn prune_tiles(db: &Database, budget: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM dev_caloptreyx_worldmap_tiles AS tiles USING (
             SELECT server_uuid, path FROM (
                 SELECT server_uuid, path,
                     SUM(octet_length(image)) OVER (ORDER BY used DESC, server_uuid, path) AS total
                 FROM dev_caloptreyx_worldmap_tiles
             ) AS ranked WHERE total > $1
         ) AS old
         WHERE tiles.server_uuid = old.server_uuid AND tiles.path = old.path",
    )
    .bind(budget)
    .execute(db.write())
    .await?;
    Ok(result.rows_affected())
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
