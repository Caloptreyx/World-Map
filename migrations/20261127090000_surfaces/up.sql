-- 3D surfaces of regions (zlib-compressed heights, colours and water per block column), keyed
-- by the region file they were read from like the tiles: only served while the file's size and
-- modification time still match. Pruned together with the tiles within one cache budget.
CREATE TABLE dev_caloptreyx_worldmap_surfaces (
    server_uuid uuid NOT NULL REFERENCES servers(uuid) ON DELETE CASCADE,
    path TEXT NOT NULL,
    modified BIGINT NOT NULL,
    size BIGINT NOT NULL,
    -- surface format version and options (for example the Nether ceiling) it was made with
    variant VARCHAR(63) NOT NULL,
    data BYTEA NOT NULL,
    used TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_uuid, path)
);

CREATE INDEX dev_caloptreyx_worldmap_surfaces_used_idx ON dev_caloptreyx_worldmap_surfaces (used);
