"""Builds the tests/fixtures/relocate_*.mca fixtures of src/relocate.rs (chunk import: offsets and merges).

relocate_region.mca
  slot 0: a real 1.21.4 chunk (paper world, region r.-1.0, slot 218) with a block tick, structure
          references and starts, an end gateway, a spawner and a beehive added
  slot 1: a real 1.21.4 chunk (same region, slot 250): merge destination
  slot 2: a synthetic 1.16.5 chunk (Level format) with every relocated field
  slot 3: a synthetic 1.16.5 chunk: merge destination for slot 2
relocate_entities.mca
  slot 0: a real 1.21.4 entities chunk (three rabbits with Paper.Origin) plus synthetic entities covering
          passengers, leashes, block positions, villager memories and items
  slot 1: a synthetic entities chunk: merge destination
relocate_poi.mca
  slot 0: a synthetic 1.21.4 poi chunk; slot 1: a merge destination

Usage: python3 scripts/make_relocate_fixtures.py <world folder>
"""

import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from nbt import (  # noqa: E402
    BYTE,
    COMPOUND,
    DOUBLE,
    END,
    INT,
    INT_ARRAY,
    LIST,
    LONG,
    LONG_ARRAY,
    SHORT,
    STRING,
    Tagged,
    build_region,
    chunk_nbt,
    region_chunks,
    write,
)

OUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"


def c(**entries):
    return Tagged(COMPOUND, dict(entries))


def cd(entries):
    return Tagged(COMPOUND, dict(entries))


def i(v):
    return Tagged(INT, v)


def b(v):
    return Tagged(BYTE, v)


def s(v):
    return Tagged(STRING, v)


def ia(*v):
    return Tagged(INT_ARRAY, list(v))


def lst(items, kind=COMPOUND):
    return Tagged(LIST, items, kind if items else END)


def dpos(x, y, z):
    return lst([Tagged(DOUBLE, x), Tagged(DOUBLE, y), Tagged(DOUBLE, z)], DOUBLE)


def chunk_long(x, z):
    """A structure reference: chunk x in the low 32 bits, z in the high ones (signed long)."""
    value = ((z & 0xFFFFFFFF) << 32) | (x & 0xFFFFFFFF)
    return value - (1 << 64) if value >= 1 << 63 else value


def real_chunk(path, index):
    for slot, compression, payload in region_chunks(path.read_bytes()):
        if slot == index:
            return chunk_nbt(compression, payload)
    raise SystemExit(f"{path} has no chunk {index}")


def entry(index, root):
    return (index, 2, zlib.compress(write(root)))


def region_v1214(world):
    root = real_chunk(world / "region" / "r.-1.0.mca", 218)
    v = root.value
    x, z = v["xPos"].value, v["zPos"].value
    bx, bz = x * 16, z * 16
    v["block_ticks"] = lst([c(i=s("minecraft:redstone_wire"), x=i(bx + 3), y=i(-60), z=i(bz + 4), t=i(2), p=i(0))])
    v["structures"] = c(
        References=c(**{"minecraft:mineshaft": Tagged(LONG_ARRAY, [chunk_long(x - 1, z + 2), chunk_long(-5, -7)])}),
        starts=c(
            **{
                "minecraft:village_plains": c(
                    id=s("minecraft:village_plains"),
                    ChunkX=i(x),
                    ChunkZ=i(z),
                    references=i(0),
                    Children=lst(
                        [
                            c(
                                id=s("minecraft:jigsaw"),
                                BB=ia(bx, 63, bz, bx + 9, 70, bz + 9),
                                PosX=i(bx),
                                PosY=i(63),
                                PosZ=i(bz),
                                junctions=lst([c(source_x=i(bx + 4), source_y=i(64), source_z=i(bz), dest_proj=s("rigid"), delta_y=i(0))]),
                            ),
                            c(
                                id=s("minecraft:mscorridor"),
                                BB=ia(bx + 2, -40, bz + 2, bx + 6, -36, bz + 14),
                                TPX=i(bx + 2),
                                TPY=i(-40),
                                TPZ=i(bz + 2),
                                Entrances=lst([ia(bx + 2, -40, bz + 2, bx + 4, -38, bz + 2)], INT_ARRAY),
                            ),
                        ]
                    ),
                ),
                "minecraft:stronghold": c(id=s("INVALID")),
            }
        ),
    )
    v["block_entities"].value.extend(
        [
            c(id=s("minecraft:end_gateway"), x=i(bx + 1), y=i(80), z=i(bz + 1), keepPackedEntities=b(0), Age=Tagged(LONG, 5), exit_portal=ia(100, 75, -200), ExitPortal=c(X=i(1), Y=i(2), Z=i(3))),
            c(
                id=s("minecraft:spawner"),
                x=i(bx + 8),
                y=i(-50),
                z=i(bz + 8),
                SpawnPotentials=lst([c(weight=i(1), data=c(entity=c(id=s("minecraft:zombie"), UUID=ia(1, 2, 3, 4))))]),
                SpawnData=c(entity=c(id=s("minecraft:zombie"))),
            ),
            c(
                id=s("minecraft:beehive"),
                x=i(bx + 5),
                y=i(70),
                z=i(bz + 5),
                flower_pos=ia(bx + 7, 69, bz + 9),
                bees=lst([c(min_ticks_in_hive=i(600), ticks_in_hive=i(10), entity_data=c(id=s("minecraft:bee"), hive_pos=ia(bx + 5, 70, bz + 5), UUID=ia(5, 6, 7, 8)))]),
            ),
        ]
    )
    return root


def level_chunk(x, z, marker, biome_base):
    """A 1.16.5 chunk with every field MCA Selector relocates in the Level format."""
    bx, bz = x * 16, z * 16
    sections = [cd({"Y": b(y), "Palette": lst([c(Name=s(marker))]), "BlockStates": Tagged(LONG_ARRAY, [y] * 4)}) for y in range(0, 5)]
    # biomes: 4x4x4 cells per section, value = section * 100 + cell row (so rows are distinguishable)
    biomes = [biome_base + (row // 4) * 100 + row % 4 for row in range(64) for _ in range(16)]
    shorts = lambda n: lst([Tagged(SHORT, k) for k in range(n)], SHORT)  # noqa: E731
    index_lists = lambda: lst([shorts(y % 3) for y in range(16)], LIST)  # noqa: E731
    pig = c(
        id=s("minecraft:pig"),
        Pos=dpos(bx + 2.5, 65.0, bz + 3.5),
        UUIDMost=Tagged(LONG, 11),
        UUIDLeast=Tagged(LONG, 12),
        Leash=c(X=i(bx + 1), Y=i(64), Z=i(bz + 1)),
        Passengers=lst([c(id=s("minecraft:zombie"), Pos=dpos(bx + 2.5, 66.0, bz + 3.5), UUID=ia(9, 9, 9, 9))]),
    )
    turtle = c(
        id=s("minecraft:turtle"),
        Pos=dpos(bx + 8.0, 30.0, bz + 8.0),
        UUID=ia(1, 1, 1, 1),
        HomePosX=i(bx + 9),
        HomePosY=i(29),
        HomePosZ=i(bz + 9),
        TravelPosX=i(bx + 10),
        TravelPosY=i(28),
        TravelPosZ=i(bz + 10),
    )
    level = {
        "xPos": i(x),
        "zPos": i(z),
        "Status": s("full"),
        "Sections": lst(sections),
        "Biomes": Tagged(INT_ARRAY, biomes),
        "Heightmaps": c(MOTION_BLOCKING=Tagged(LONG_ARRAY, [7] * 37)),
        "Entities": lst([pig, turtle]),
        "TileEntities": lst(
            [
                c(id=s("minecraft:chest"), x=i(bx + 4), y=i(20), z=i(bz + 5)),
                c(id=s("minecraft:end_gateway"), x=i(bx + 6), y=i(70), z=i(bz + 6), ExitPortal=c(X=i(500), Y=i(60), Z=i(-500))),
                c(id=s("minecraft:mob_spawner"), x=i(bx + 7), y=i(12), z=i(bz + 7), SpawnPotentials=lst([c(Weight=i(1), Entity=c(id=s("minecraft:skeleton"), UUID=ia(2, 2, 2, 2)))])),
            ]
        ),
        "TileTicks": lst([c(i=s("minecraft:stone"), x=i(bx), y=i(40), z=i(bz), t=i(1), p=i(0))]),
        "LiquidTicks": lst([c(i=s("minecraft:water"), x=i(bx + 15), y=i(75), z=i(bz + 15), t=i(5), p=i(0))]),
        "Lights": index_lists(),
        "LiquidsToBeTicked": index_lists(),
        "ToBeTicked": index_lists(),
        "PostProcessing": index_lists(),
        "Structures": c(
            References=c(Mineshaft=Tagged(LONG_ARRAY, [chunk_long(x, z), chunk_long(x + 1, z - 1)])),
            Starts=c(
                Mineshaft=c(
                    id=s("Mineshaft"),
                    ChunkX=i(x),
                    ChunkZ=i(z),
                    BB=ia(bx, 10, bz, bx + 40, 50, bz + 40),
                    Processed=lst([c(X=i(x), Z=i(z))]),
                    Children=lst(
                        [
                            c(id=s("MSRoom"), BB=ia(bx, 10, bz, bx + 10, 15, bz + 10), Entrances=lst([ia(bx, 10, bz, bx + 2, 12, bz)], INT_ARRAY)),
                            c(id=s("MSCorridor"), BB=ia(bx + 10, 40, bz, bx + 20, 45, bz + 3), TPX=i(bx + 10), TPY=i(40), TPZ=i(bz)),
                        ]
                    ),
                ),
                Village=c(id=s("INVALID")),
            ),
        ),
    }
    return c(DataVersion=i(2586), Level=cd(level))


def entities_v1214(world):
    root = real_chunk(world / "entities" / "r.-1.0.mca", 247)
    px, pz = root.value["Position"].value
    bx, bz = px * 16, pz * 16
    compass = c(
        id=s("minecraft:compass"),
        count=i(1),
        components=cd({"minecraft:lodestone_tracker": c(target=c(pos=ia(bx + 1, 64, bz + 1), dimension=s("minecraft:overworld")))}),
    )
    shulker_box = c(
        id=s("minecraft:shulker_box"),
        count=i(1),
        components=cd({"minecraft:container": lst([c(slot=i(0), item=c(id=s("minecraft:compass"), count=i(1), components=cd({"minecraft:lodestone_tracker": c(target=c(pos=ia(7, 8, 9)))})))])}),
    )
    extra = [
        c(
            id=s("minecraft:horse"),
            Pos=dpos(bx + 4.5, 70.0, bz + 4.5),
            UUID=ia(10, 20, 30, 40),
            leash=ia(bx + 2, 69, bz + 2),
            home_pos=ia(bx + 3, 68, bz + 3),
            Passengers=lst([c(id=s("minecraft:zombie"), Pos=dpos(bx + 4.5, 71.0, bz + 4.5), UUID=ia(50, 60, 70, 80), sleeping_pos=ia(1, 2, 3))]),
        ),
        c(id=s("minecraft:wolf"), Pos=dpos(bx + 1.0, 64.0, bz + 1.0), UUID=ia(3, 3, 3, 3), Leash=c(X=i(bx + 5), Y=i(64), Z=i(bz + 6))),
        c(
            id=s("minecraft:villager"),
            Pos=dpos(bx + 6.0, 64.0, bz + 6.0),
            UUID=ia(4, 4, 4, 4),
            Brain=c(memories=cd({"minecraft:home": c(value=c(pos=ia(bx + 6, 64, bz + 7), dimension=s("minecraft:overworld"))), "minecraft:job_site": c(pos=ia(bx + 8, 64, bz + 8))})),
            equipment=c(mainhand=compass),
        ),
        c(id=s("minecraft:end_crystal"), Pos=dpos(bx + 0.5, 100.0, bz + 0.5), UUID=ia(5, 5, 5, 5), beam_target=ia(0, 128, 0)),
        c(id=s("minecraft:item_frame"), Pos=dpos(bx + 9.5, 65.5, bz + 0.03), UUID=ia(6, 6, 6, 6), block_pos=ia(bx + 9, 65, bz), TileX=i(bx + 9), TileY=i(65), TileZ=i(bz)),
        c(id=s("minecraft:dolphin"), Pos=dpos(bx + 12.0, 40.0, bz + 12.0), UUID=ia(7, 7, 7, 7), TreasurePosX=i(300), TreasurePosY=i(30), TreasurePosZ=i(-300)),
        c(id=s("minecraft:chest_minecart"), Pos=dpos(bx + 13.0, 50.0, bz + 2.0), UUID=ia(8, 8, 8, 8), Items=lst([shulker_box])),
    ]
    root.value["Entities"].value.extend(extra)
    return root


def entities_destination(px, pz):
    bx, bz = px * 16, pz * 16
    return c(
        DataVersion=i(4189),
        Position=ia(px, pz),
        Entities=lst(
            [
                c(id=s("minecraft:cow"), Pos=dpos(bx + 1.0, -30.0, bz + 1.0), UUID=ia(1, 0, 0, 1)),
                c(id=s("minecraft:sheep"), Pos=dpos(bx + 2.0, 70.0, bz + 2.0), UUID=ia(2, 0, 0, 2)),
                c(id=s("minecraft:bat"), Pos=dpos(bx + 3.0, 200.0, bz + 3.0), UUID=ia(3, 0, 0, 3)),
            ]
        ),
    )


def poi(px, pz, kinds):
    bx, bz = px * 16, pz * 16
    sections = {}
    for y, kind in kinds.items():
        sections[str(y)] = c(Valid=b(1), Records=lst([c(pos=ia(bx + 3, y * 16 + 5, bz + 4), type=s(kind), free_tickets=i(0))]))
    return c(DataVersion=i(4189), Sections=cd(sections))


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    world = Path(sys.argv[1])
    region = [
        entry(0, region_v1214(world)),
        entry(1, real_chunk(world / "region" / "r.-1.0.mca", 250)),
        entry(2, level_chunk(10, -3, "minecraft:stone", 0)),
        entry(3, level_chunk(10, -3, "minecraft:dirt", 5000)),
    ]
    (OUT / "relocate_region.mca").write_bytes(build_region(region))
    entities = entities_v1214(world)
    px, pz = entities.value["Position"].value
    (OUT / "relocate_entities.mca").write_bytes(build_region([entry(0, entities), entry(1, entities_destination(px, pz))]))
    source = poi(4, -2, {-4: "minecraft:home", 3: "minecraft:meeting", 19: "minecraft:bee_nest"})
    destination = poi(4, -2, {-4: "minecraft:lodestone", 2: "minecraft:armorer", 5: "minecraft:fletcher"})
    (OUT / "relocate_poi.mca").write_bytes(build_region([entry(0, source), entry(1, destination)]))


if __name__ == "__main__":
    main()
