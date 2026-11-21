-- World folders added by hand per server (paths below the server root), for layouts the map
-- does not find by itself.
ALTER TABLE dev_caloptreyx_worldmap_servers
    ADD COLUMN extra_worlds TEXT[] NOT NULL DEFAULT '{}';
