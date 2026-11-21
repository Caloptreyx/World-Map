# Changelog

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
