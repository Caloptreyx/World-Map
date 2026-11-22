-- Which players the map shows per server: online ones, offline ones. Both by default, so
-- servers opened before keep showing everyone.
ALTER TABLE dev_caloptreyx_worldmap_servers
    ADD COLUMN show_online_players BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN show_offline_players BOOLEAN NOT NULL DEFAULT TRUE;
