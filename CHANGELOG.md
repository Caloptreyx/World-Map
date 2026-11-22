# Changelog

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
