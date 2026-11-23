-- Chunk tool write operations (delete, change fields, import, restore): their backup folder
-- below `.worldmap-backups/`, the files they replaced or removed and how they ended.
CREATE TABLE dev_caloptreyx_worldmap_operations (
    id uuid NOT NULL PRIMARY KEY,
    server_uuid uuid NOT NULL REFERENCES servers(uuid) ON DELETE CASCADE,
    user_uuid uuid,
    kind VARCHAR(15) NOT NULL,
    world VARCHAR(255) NOT NULL,
    dimension VARCHAR(255) NOT NULL,
    backup TEXT NOT NULL,
    -- [{"path": "world/region/r.0.0.mca", "existed": true}, ...]
    files JSONB NOT NULL DEFAULT '[]',
    summary JSONB NOT NULL DEFAULT '{}',
    -- done, partial (still running, failed or aborted midway), restored, pruned
    state VARCHAR(15) NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    restored TIMESTAMPTZ
);

CREATE INDEX dev_caloptreyx_worldmap_operations_server_idx
    ON dev_caloptreyx_worldmap_operations (server_uuid, created DESC);

-- Overlay values of one region (1024 little-endian f64, x + z * 32, NaN = no value), keyed by
-- the region's files: only served while `version` (the files' sizes and modification times and
-- the overlay version) still matches.
CREATE TABLE dev_caloptreyx_worldmap_overlays (
    server_uuid uuid NOT NULL REFERENCES servers(uuid) ON DELETE CASCADE,
    path TEXT NOT NULL,
    overlay VARCHAR(255) NOT NULL,
    version TEXT NOT NULL,
    data BYTEA NOT NULL,
    used TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_uuid, path, overlay)
);

CREATE INDEX dev_caloptreyx_worldmap_overlays_used_idx ON dev_caloptreyx_worldmap_overlays (used);
