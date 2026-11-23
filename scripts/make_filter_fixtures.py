#!/usr/bin/env python3
"""Builds the region files the filter and overlay tests read (tests/fixtures/filter_*.mca).

The existing fixtures (scripts/make_fixtures.py) hold plain chunks; the filters also need
structure references, block entities, proto-entities, entities, inhabited time and the
per-version places of all of these, which the recorded world does not have. So:

- filter_modern.mca: three chunks of a real Paper 1.21.4 world (DataVersion 4189) at their
  original slots of r.-1.0.mca: 89 (a chest, copied byte for byte), 121 (a spawner and a chest,
  given InhabitedTime 1200 = 1 minute, a mineshaft structure start and references and an empty
  village reference) and 18 (a proto-chunk in status carvers, given two proto-entities and stored
  gzip-compressed). Every chunk gets its own header timestamp.
- filter_entities.mca: the entities/ chunk for slot 121 (a cow and two chickens), synthesized.
- filter_legacy.mca: synthesized older chunks: slot 0 a 1.16 chunk (Level.*: Entities,
  TileEntities, Structures with old-style names, int biomes, isLightOn), slot 1 a 1.12 chunk
  (DataVersion 1343: byte biomes incl. a signed id 129, LightPopulated, numeric block ids).

Usage: python3 scripts/make_filter_fixtures.py <world folder holding region/>
"""

import gzip
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from make_fixtures import legacy_chunk  # noqa: E402
from nbt import (  # noqa: E402
    BYTE,
    BYTE_ARRAY,
    COMPOUND,
    DOUBLE,
    INT,
    INT_ARRAY,
    LIST,
    LONG,
    LONG_ARRAY,
    STRING,
    Tagged,
    build_region,
    chunk_nbt,
    region_chunks,
    write,
)

OUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"


def with_timestamps(region, timestamps):
    """The region file with the header timestamp of each slot in `timestamps` replaced."""
    data = bytearray(region)
    for index, timestamp in timestamps.items():
        struct.pack_into(">I", data, 4096 + index * 4, timestamp)
    return bytes(data)


def entity(entity_id, x, y, z):
    return Tagged(
        COMPOUND,
        {
            "id": Tagged(STRING, entity_id),
            "Pos": Tagged(LIST, [Tagged(DOUBLE, x), Tagged(DOUBLE, y), Tagged(DOUBLE, z)], DOUBLE),
        },
    )


def chunk_ref(x, z):
    """A structure reference: the chunk position of the structure start, packed as the game does."""
    value = (x & 0xFFFFFFFF) | ((z & 0xFFFFFFFF) << 32)
    return value - (1 << 64) if value >= 1 << 63 else value


def modern(world):
    source = (world / "region/r.-1.0.mca").read_bytes()
    chunks = {index: (index, c, p) for index, c, p in region_chunks(source)}

    spawner = chunk_nbt(chunks[121][1], chunks[121][2])
    root = spawner.value
    root["InhabitedTime"] = Tagged(LONG, 1200)
    structures = root["structures"].value
    structures["References"] = Tagged(
        COMPOUND,
        {
            "minecraft:mineshaft": Tagged(LONG_ARRAY, [chunk_ref(-7, 3)]),
            "minecraft:village_plains": Tagged(LONG_ARRAY, []),
        },
    )
    structures["starts"] = Tagged(
        COMPOUND,
        {
            "minecraft:mineshaft": Tagged(
                COMPOUND,
                {
                    "id": Tagged(STRING, "minecraft:mineshaft"),
                    "ChunkX": Tagged(INT, -7),
                    "ChunkZ": Tagged(INT, 3),
                    "references": Tagged(INT, 0),
                    "Children": Tagged(LIST, [], COMPOUND),
                },
            )
        },
    )

    proto = chunk_nbt(chunks[18][1], chunks[18][2])
    proto.value["entities"] = Tagged(
        LIST, [entity("minecraft:sheep", -222.5, 70.0, 5.5), entity("minecraft:pig", -220.5, 70.0, 6.5)], COMPOUND
    )

    region = build_region(
        [
            chunks[89],
            (121, 2, zlib.compress(write(spawner))),
            (18, 1, gzip.compress(write(proto), mtime=0)),
        ]
    )
    (OUT / "filter_modern.mca").write_bytes(
        with_timestamps(region, {18: 1_600_000_000, 89: 1_700_000_000, 121: 1_710_000_000})
    )


def entities():
    root = {
        "DataVersion": Tagged(INT, 4189),
        "Position": Tagged(INT_ARRAY, [-7, 3]),
        "Entities": Tagged(
            LIST,
            [
                entity("minecraft:cow", -105.5, 64.0, 50.5),
                entity("minecraft:chicken", -106.5, 64.0, 51.5),
                entity("minecraft:chicken", -107.5, 64.0, 52.5),
            ],
            COMPOUND,
        ),
    }
    region = build_region([(121, 2, zlib.compress(write(Tagged(COMPOUND, root))))])
    (OUT / "filter_entities.mca").write_bytes(region)


def legacy():
    v116 = legacy_chunk(0, 0, 2586)
    level = v116.value["Level"].value
    level["InhabitedTime"] = Tagged(LONG, 72000)
    level["LastUpdate"] = Tagged(LONG, 1000)
    level["isLightOn"] = Tagged(BYTE, 1)
    level["Biomes"] = Tagged(INT_ARRAY, [1] * 512 + [2] * 512)
    level["Entities"] = Tagged(LIST, [entity("minecraft:cow", 2.5, 61.0, 2.5), entity("minecraft:pig", 3.5, 61.0, 3.5)], COMPOUND)
    level["TileEntities"] = Tagged(
        LIST,
        [Tagged(COMPOUND, {"id": Tagged(STRING, "minecraft:chest"), "x": Tagged(INT, 1), "y": Tagged(INT, 61), "z": Tagged(INT, 1)})],
        COMPOUND,
    )
    level["Structures"] = Tagged(
        COMPOUND,
        {
            "References": Tagged(COMPOUND, {"Village": Tagged(LONG_ARRAY, [chunk_ref(0, 0)]), "Mineshaft": Tagged(LONG_ARRAY, [])}),
            "Starts": Tagged(COMPOUND, {"Village": Tagged(COMPOUND, {"id": Tagged(STRING, "Village"), "ChunkX": Tagged(INT, 0), "ChunkZ": Tagged(INT, 0)})}),
        },
    )

    v112 = {
        "DataVersion": Tagged(INT, 1343),
        "Level": Tagged(
            COMPOUND,
            {
                "xPos": Tagged(INT, 1),
                "zPos": Tagged(INT, 0),
                "LastUpdate": Tagged(LONG, 500),
                "InhabitedTime": Tagged(LONG, 0),
                "LightPopulated": Tagged(BYTE, 1),
                "TerrainPopulated": Tagged(BYTE, 1),
                "Biomes": Tagged(BYTE_ARRAY, [1] * 255 + [-127]),
                "Entities": Tagged(LIST, [entity("minecraft:zombie", 20.5, 5.0, 2.5)], COMPOUND),
                "TileEntities": Tagged(LIST, [], COMPOUND),
                "Sections": Tagged(
                    LIST,
                    [Tagged(COMPOUND, {"Y": Tagged(BYTE, 0), "Blocks": Tagged(BYTE_ARRAY, [1] * 4096), "Data": Tagged(BYTE_ARRAY, [0] * 2048)})],
                    COMPOUND,
                ),
            },
        ),
    }
    region = build_region([(0, 2, zlib.compress(write(v116))), (1, 2, zlib.compress(write(Tagged(COMPOUND, v112))))])
    (OUT / "filter_legacy.mca").write_bytes(region)


def main():
    world = Path(sys.argv[1])
    modern(world)
    entities()
    legacy()
    for path in sorted(OUT.glob("filter_*.mca")):
        print(f"{path.name}: {path.stat().st_size} bytes", file=sys.stderr)


if __name__ == "__main__":
    main()
