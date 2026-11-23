# MCA Selector oracle fixtures

Everything in this folder except `select/handwritten-inverted.csv` and `inputs/bounds.csv` was written
by the real MCA Selector, run headless. Regenerate with `scripts/mcaselector_oracle.sh` (needs docker; it
overwrites this folder except this README).

## Producer

- MCA Selector **2.9**, the release jar `mcaselector-2.9.jar` from
  https://github.com/Querz/mcaselector/releases/tag/2.9 (tag `2.9` = commit
  `88884a9b41dc834189eb90a872388fb226d77985`), sha256
  `8e35eaa6a5119ce711b53530c22aaac531332e8fdffda2357d16eb6f52327267`.
- Runtime: `eclipse-temurin:21-jdk` (`sha256:8ad118b146aa92da815b576441acd6b42b38236797ad0b6edc8c200b85bb0734`,
  Temurin 21.0.13.1+2) plus OpenJFX 21.0.2 linux module jars from Maven Central (base, graphics,
  controls, swing). Headless mode runs on a plain JRE, but the `Structures` filter loads JavaFX
  (`StructureRegistry` renders icons through `javafx.embed.swing.SwingFXUtils`), and `javafx.swing`
  needs the JDK-only module `jdk.unsupported.desktop`.
- Every run: `java -Duser.home=/tmp -Dprism.order=sw --module-path fx --add-modules
  javafx.controls,javafx.swing -jar mcaselector-2.9.jar --process-threads 1 --write-threads 1 <args>`,
  one `docker run --rm` container per run.

## Input world: `world/`

Copied from the Paper 1.21.x world (DataVersion 4189, all chunks zlib) at
`/var/lib/calagopus-wings/volumes/a8e9078c-577b-4835-b31b-1a5e1830e11c/world` (`region/`,
`entities/`; the world has no `poi/`), then trimmed by MCA Selector itself:

    --mode delete --world trim --query '!(xPos >= -10 AND xPos <= 1 AND zPos >= -2 AND zPos <= 7)'

Result: 120 chunks, x -10..1, z -2..7, over r.-1.-1, r.-1.0, r.0.-1, r.0.0 (MCA Selector rewrote the
files compactly; chunk timestamps kept). Statuses: 35 `minecraft:full` (x -10..-6, z 1..7),
13 `initialize_light`, 15 `carvers`, 17 `biomes`, 40 `structure_starts`. `entities/r.-1.0.mca` holds one
chunk, (-9, 7), with 3 rabbits. InhabitedTime is 0 everywhere. Chunk (1, 0) has a `minecraft:shipwreck`
structure start but no chunk has structure references. The 85 proto chunks have an empty `entities` list.

| file | bytes |
|---|---|
| world/region/r.-1.-1.mca | 90112 |
| world/region/r.-1.0.mca | 483328 |
| world/region/r.0.-1.mca | 24576 |
| world/region/r.0.0.mca | 73728 |
| world/entities/r.-1.0.mca | 12288 |

## Selections: `select/*.csv`, `queries-expected.json`

`inputs/queries.tsv` lists `name<TAB>query`; each one was run as

    --mode select --world trim --query '<query>' --output select/<name>.csv

`queries-expected.json` maps each query to the sorted `[[x, z], ...]` chunks of its CSV (whole-region
lines expanded to their 1024 chunks), for filter parity on `world/`.

| name | query | chunks | what it shows |
|---|---|---|---|
| xpos-zpos | `xPos >= 0 AND zPos < 0` | 4 | xPos/zPos come from chunk data: absent chunks never match |
| inhabited-lt-1min | `InhabitedTime < "1 minute"` | 4096 | **absent chunks count as InhabitedTime 0**, so every slot of the 4 regions matches and MCA Selector writes 4 whole-region lines |
| inhabited-gt-0 | `InhabitedTime > 0` | 0 | empty selection = empty file (0 bytes) |
| status-full | `Status = "minecraft:full"` | 35 | |
| status-or-unnamespaced | `Status = "carvers" OR Status = "biomes"` | 32 | names without `minecraft:` are accepted; OR |
| not-group | `!(Status = "minecraft:structure_starts") AND xPos < -5` | 50 | negated group |
| palette-contains | `Palette contains "minecraft:water"` | 59 | |
| palette-contains-all | `Palette contains "minecraft:sand,minecraft:lapis_ore"` | 46 | contains = all listed blocks |
| palette-intersects | `Palette intersects "minecraft:bedrock,minecraft:glow_lichen"` | 63 | intersects = any listed block |
| palette-equals | `Palette = "minecraft:air"` | 57 | |
| biome-contains | `Biome contains "minecraft:river"` | 22 | |
| biome-intersects | `Biome intersects "minecraft:warm_ocean,minecraft:river"` | 48 | |
| entities-amount | `#Entities > 0` | 1 | reads `entities/` |
| entities-contains | `Entities contains "minecraft:rabbit"` | 1 | |
| proto-entities-amount | `#ProtoEntities > 0` | 0 | |
| tile-entities-amount | `#TileEntities > 0` | 4 | |
| lastupdate-and-or | `LastUpdate > 1000000 AND (xPos = -10 OR zPos = 7)` | 11 | nested group |
| structures-contains | `Structures contains "minecraft:shipwreck"` | 0 | **Structures checks `structures.References` only**, not `starts` (chunk (1, 0) has the start) |
| circle | `Circle contains "-8;4;3"` | 37 | **Circle is location-based: it matches absent chunks too** (x = -11 is outside the world) |
| dataversion | `DataVersion = 4189` | 120 | |
| lightpopulated | `LightPopulated = 0` | 4096 | absent chunks match (like InhabitedTime) |
| timestamp | `Timestamp > "2026-10-01"` | 35 | region-header timestamps; date string accepted |
| compression | `Compression = "zlib"` | 120 | |

CSV layout (Selection.saveToFile): one line per chunk `regionX;regionZ;chunkX;chunkZ` (absolute chunk
coordinates), or `regionX;regionZ` for a whole region, `\n` line ends, no header unless inverted.
Regions come in fastutil `Long2ObjectOpenHashMap` order (e.g. `0;0`, `-1;0`, `0;-1`, `-1;-1`), chunks
inside a region by index `(x & 31) + (z & 31) * 32`, so compare CSVs as sets, not as text.

Extra selection files:

- `radius-whole-region.csv` (MCA-made, 813 lines): whole-region line `-1;0` plus 812 chunk lines in
  r.0.0, from
  `--mode select --world trim --query 'Status = "minecraft:full"' --radius 33 --selection inputs/bounds.csv`.
  The radius grows every match into a disc (`(r + 0.3)²` test) clipped to the `--selection` bounds
  (`inputs/bounds.csv`, hand-written: whole regions `-1;0` and `0;0`), and adds chunks whether they
  exist or not; a region whose 1024 chunks all end up selected is written as a whole-region line.
- `handwritten-inverted.csv`: **hand-written**, following Selection.saveToFile, because headless select
  always starts from a new, non-inverted Selection and can never write `inverted`. The first line
  `inverted` marks the rest as excluded: whole region `0;-1` and chunks (-9, 7), (-10, 1).
- `inverted-resolved.csv` (MCA-made): MCA Selector reading `handwritten-inverted.csv` as
  `--selection` with `--query 'xPos >= -1000000'`: 114 chunks = the 120 existing minus the 4 of r.0.-1
  and the 2 listed chunks.

## Change fields: `change/`

`inputs/change-selection.csv` (MCA-made) selects chunk (-2, 0) (`structure_starts`, all-air sections)
and chunk (-9, 7) (`full`, with the rabbits in `entities/`):

    --mode select --world trim --query '(xPos = -9 AND zPos = 7) OR (xPos = -2 AND zPos = 0)' --output inputs/change-selection.csv

`change/before/` is the trimmed world with every other chunk deleted
(`--mode delete --query '!(<same query>)'`). Each other folder is a copy of `before/` after

    --mode change --world <copy> --selection inputs/change-selection.csv --fields '<fields>'

(fields from `inputs/changes.tsv`, no `--force`). Each folder has `region/r.-1.0.mca` (16384 bytes)
and `entities/r.-1.0.mca` (12288 bytes); MCA Selector rewrites both files (re-compressed, chunk
timestamps kept) even when only one changes.

| folder | fields | observed change (NBT diff against `before/`) |
|---|---|---|
| inhabited-lastupdate | `InhabitedTime = 1200, LastUpdate = 0` | both chunks: InhabitedTime 0 -> 1200, LastUpdate -> 0 |
| status | `Status = "empty"` | both chunks: Status -> `minecraft:empty` (namespace added) |
| delete-sections | `DeleteSections = "4:"` | (-9, 7) only: Status `minecraft:full` -> `minecraft:features`; section Y=4 loses `block_states.data` and its palette becomes `[{Name: "minecraft:air"}]` (biomes kept; sections 5+ were already air); Heightmaps unchanged. (-2, 0): no change (Status stays `structure_starts`, all its sections are air) |
| delete-entities | `DeleteEntities = true` | **nothing**: in 2.9 `ChunkFilter_21w43a.Entities.deleteEntities` looks up `Level.Entities` / `Level.entities`, which do not exist in 1.17+ chunks, so the rabbits stay (MCA Selector bug; the same lookup makes DeleteSections leave entities alone) |

## Import with offset: `import/`

Source: `change/before/`. Target: an empty world (`region/`, `entities/`). ImportJob returns
"no files" when the target has no region file at all, so a header-only placeholder `region/r.9.9.mca`
was put in each target and deleted afterwards (it is never touched by the import).

- `offset-33_-2/`: `--mode import --world <target> --source-world change-before --x-offset 33 --z-offset -2`
  - `region/r.0.0.mca` (12288): (-9, 7) -> (24, 5). Only xPos/zPos change, except that the empty
    section Y=-5 (no `block_states`) is dropped. Status stays `minecraft:full`, timestamp kept.
  - `region/r.0.-1.mca` (12288): (-2, 0) -> (31, -2); only xPos/zPos change.
  - `entities/r.0.0.mca` (12288): Position [-9, 7] -> [24, 5]; each rabbit's Pos moves by
    (+528, 0, -32) blocks and gets a **new random UUID**; `Paper.Origin` is not moved.
- `offset--40_5-y2-sections-0_3/`: `... --x-offset -40 --z-offset 5 --y-offset 2 --sections "0:3"`
  - `region/r.-2.0.mca` (16384): (-9, 7) -> (-49, 12) and (-2, 0) -> (-42, 5). With a section range
    MCA Selector builds new chunks holding only the imported sections: keys `DataVersion`, `Status`
    (`minecraft:full` for both, also the former structure_starts chunk), `block_entities`,
    `block_ticks`, `fluid_ticks`, `sections`, `xPos`, `yPos`, `zPos` (no Heightmaps, structures,
    LastUpdate, InhabitedTime), sections Y 0..3 moved to Y 2..5, header timestamp 0.
  - `entities/r.-2.0.mca` (12288): chunk (-49, 12) with Position [-49, 12] and no entities (the
    rabbits at y 64, section 4, are outside the range).
