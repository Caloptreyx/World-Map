-- Rendered region tiles (PNG), keyed by the region file they were drawn from. A tile is only
-- served while the file's size and modification time still match.
CREATE TABLE dev_caloptreyx_worldmap_tiles (
    server_uuid uuid NOT NULL REFERENCES servers(uuid) ON DELETE CASCADE,
    path TEXT NOT NULL,
    modified BIGINT NOT NULL,
    size BIGINT NOT NULL,
    -- renderer version and options (for example the Nether ceiling) the tile was drawn with
    variant VARCHAR(63) NOT NULL,
    image BYTEA NOT NULL,
    used TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_uuid, path)
);

CREATE INDEX dev_caloptreyx_worldmap_tiles_used_idx ON dev_caloptreyx_worldmap_tiles (used);

-- Servers whose map was opened: the heatmap samples their player files while they run.
CREATE TABLE dev_caloptreyx_worldmap_servers (
    server_uuid uuid NOT NULL PRIMARY KEY REFERENCES servers(uuid) ON DELETE CASCADE,
    opened TIMESTAMPTZ NOT NULL DEFAULT now(),
    sampled TIMESTAMPTZ
);

-- Player position samples, counted per chunk and day.
CREATE TABLE dev_caloptreyx_worldmap_heat (
    server_uuid uuid NOT NULL REFERENCES servers(uuid) ON DELETE CASCADE,
    world VARCHAR(255) NOT NULL,
    dimension VARCHAR(255) NOT NULL,
    day DATE NOT NULL,
    chunk_x INTEGER NOT NULL,
    chunk_z INTEGER NOT NULL,
    samples INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (server_uuid, world, dimension, day, chunk_x, chunk_z)
);

CREATE INDEX dev_caloptreyx_worldmap_heat_day_idx ON dev_caloptreyx_worldmap_heat (day);
