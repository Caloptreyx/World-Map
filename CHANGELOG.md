# Changelog

## 0.1.3

- Chunk tools after [MCA Selector](https://github.com/Querz/mcaselector) (same names, query syntax
  and selection files), in a new "Chunk tools" card (bottom sheet on phones):
  - **Selection**: whole regions as well as chunks (region mode, automatic when zoomed far out),
    Alt-drag or the Remove switch to unselect, select all, invert (within the regions that exist),
    clear; save, open, copy and paste selections as MCA Selector CSV files (replace, merge or
    subtract).
  - **Filter chunks**: a builder with AND/OR groups and negation, synced with a query box in MCA
    Selector's syntax (`InhabitedTime < "5 minutes" AND Status != "full"`): InhabitedTime,
    LastUpdate, DataVersion, Status, Biome, Palette, Entities, #Entities, #ProtoEntities,
    #TileEntities, Structures, xPos/yPos/zPos, Circle, Border, Timestamp, Compression,
    LightPopulated, plus `Size` (compressed chunk size). Runs as a background job with progress and
    cancel, then shows the count: select the matches, add them to the selection or delete them.
  - **Overlays** (layers menu): InhabitedTime, LastUpdate, Timestamp, DataVersion, #Entities,
    #TileEntities, AverageHeight, #Blocks and Size, with min/max, gradient and legend; computed per
    region and cached like the tiles.
  - **Change fields** on the selected chunks: Status, FixStatus, InhabitedTime, LastUpdate,
    Timestamp, DataVersion, Compression, Biome, ReplaceBlocks, DeleteEntities, DeleteSections,
    FixHeightmaps, DeleteStructureReference, PreventRetrogen, ForceBlend, RemoveLightingInfo,
    LightPopulated, with MCA Selector's "force".
  - **Copy and import**: copy the selected chunks to another world or dimension of the server, or
    import from one, with an x/z (chunk) and y (section) offset, overwrite or skip, section ranges
    and "only into the selection"; import uploaded region files or a zip; download the selection as
    a zip of `region/`, `entities/` and `poi/` files. Chunk, entity, block entity, tick, structure
    and point-of-interest coordinates move with the chunks.
  - **History**: every write is an operation with a backup; restore the latest one in one click.
- Safe writes for every change (chunk deletion included): only while the server is stopped
  (checked again, uncached, before every file), the original file is moved into
  `.worldmap-backups/<time>-<kind>-<id>/` first, the new file is written next to it and renamed
  into place, so a file is never half-written. `region/`, `entities/` and `poi/` files are changed
  together; unchanged chunks are kept byte for byte with their compression and timestamps;
  oversized chunks (`.mcc` files) are read and written as the game does. Backups are deleted after
  a retention period (admin setting, default 7 days).
- New `world-map.edit-chunks` and `world-map.import-chunks` permissions. Writes now also need the
  core `files.create` and `files.update` permissions (deleting chunks `files.delete` too), zip
  uploads `files.archive`.
- Admin setting "Days to keep chunk backups"; "most chunks per deletion" now limits every chunk
  operation.

## 0.1.2

- Online and offline players: each player is marked online or offline from the server itself
  (the query port when `enable-query=true`, else the Server List Ping, both without console
  commands), for every viewer and not only for teleports. Online markers have a green dot,
  offline markers are greyed out. A stopped server, an unknown state or no answer counts everyone
  as offline, and the Player Selection says so; a ping that names only some online players says
  how many it missed.
- "Online players" and "Offline players" toolbar toggles replace the single Players layer (both on
  by default; a saved Players choice carries over). They filter the markers, the player select
  and the counts, and are remembered per server with the rest of the view.
- Per-server setting (gear in the Player Selection, new `world-map.manage-players` permission)
  to stop showing online players and/or offline players on that server's map. The backend leaves
  them out of the player list; existing servers keep showing both.
- The player list refreshes every 30 seconds and when the server starts or stops.

## 0.1.1

- Worlds with any name: the main world follows `level-name` from `server.properties` (Java
  properties escapes, spaces, paths like `worlds/survival`), with its `<level-name>_nether`,
  `<level-name>_the_end` and Paper datapack dimension folders wherever it is.
- Bukkit's world container (`settings.world-container` in `bukkit.yml`) is searched too, and other
  worlds (Multiverse and similar) are found up to two folders deep instead of only at the root.
- World folders the map misses can be added per server from the World Overview (gear button,
  new `world-map.manage-worlds` permission); paths must stay inside the server's files.
- Datapack and mod dimensions with nested paths (`namespace:caves/deep`) are shown under their
  id; dimensions whose datapack type has a ceiling are drawn below the roof like the Nether.
- Spawn, version and the "Go to spawn" button follow the selected world; switching worlds jumps to
  that world's spawn.
- Teleporting to a map position lands in the selected world: Paper/Spigot worlds use their
  `minecraft:<world>` key, and worlds the server does not load cannot be teleported into.
- Players in Paper worlds without a `uid.dat` are placed by their dimension key.
- The World select always shows, the main world labelled "(main)"; clearer messages when no world
  was generated yet and when `level-name` points to a folder that does not exist.

## 0.1.0

- First version: a World Map page for Minecraft Java servers, drawn from the server files with no
  plugin. Overworld, Nether, End and datapack dimensions; vanilla, Paper/Spigot and 26.1 world
  layouts; worlds saved by 1.13 and newer.
- Layers for players (last saved position, skin heads), the spawn marker, mobs and other entities.
- Player heatmap from position samples while the server runs, with a retention setting.
- Chunk selection with copy and delete (server stopped, with confirmation).
- Teleport a player to another player or to a spot on the map.
- Works on phones and tablets: bottom sheets for the panels, a layers menu, pinch zoom, tap for
  the position readout and touch gestures for chunk selection.
- `world-map.view`, `world-map.delete-chunks`, `world-map.teleport` and `world-map.reset-heatmap`
  server permissions; admin settings for eggs, heatmap and rendering limits.
