#!/usr/bin/env python3
"""Builds the small region files in tests/fixtures/ and their expected surfaces.

Recorded fixtures are cut from a real Paper 1.21.4 world (DataVersion 4189): a few chunks are
copied byte for byte (same compression, same chunk index) into a new, compact region file.
Older chunk formats and the rarer compressions are synthesized here, because no such world is
at hand. `expected.json` holds, for every fixture chunk, the topmost block of every column as
an independent reference implementation (this file) sees it; the Rust tests compare against it.

Usage: python3 scripts/make_fixtures.py <world root with world/, world_nether/>
"""

import gzip
import json
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from nbt import (  # noqa: E402
    BYTE_ARRAY,
    COMPOUND,
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
INVISIBLE = {"minecraft:air", "minecraft:cave_air", "minecraft:void_air", "minecraft:barrier", "minecraft:light", "minecraft:structure_void"}
NETHER_CAP = 100


def unpack(longs, bits, count, spanning):
    """`count` indices of `bits` bits from packed longs (unsigned)."""
    out = []
    mask = (1 << bits) - 1
    if spanning:
        for i in range(count):
            bit = i * bits
            word, offset = divmod(bit, 64)
            value = (longs[word] & 0xFFFFFFFFFFFFFFFF) >> offset
            if offset + bits > 64:
                value |= (longs[word + 1] & 0xFFFFFFFFFFFFFFFF) << (64 - offset)
            out.append(value & mask)
    else:
        per = 64 // bits
        for i in range(count):
            word, slot = divmod(i, per)
            out.append(((longs[word] & 0xFFFFFFFFFFFFFFFF) >> (slot * bits)) & mask)
    return out


def bits_for(n, minimum):
    bits = max(minimum, (n - 1).bit_length())
    return bits


def sections(root):
    """(y, 4096 block names) of every section with blocks, highest first."""
    data_version = root.get("DataVersion", Tagged(INT, 0)).value
    out = []
    if "sections" in root:
        for section in root["sections"].value:
            s = section.value
            states = s.get("block_states")
            if not states:
                continue
            palette = [p.value["Name"].value for p in states.value["palette"].value]
            if len(palette) == 1:
                names = [palette[0]] * 4096
            else:
                indices = unpack(states.value["data"].value, bits_for(len(palette), 4), 4096, False)
                names = [palette[i] for i in indices]
            out.append((s["Y"].value, names))
    else:
        level = root["Level"].value
        for section in level["Sections"].value:
            s = section.value
            if "Palette" not in s:
                continue
            palette = [p.value["Name"].value for p in s["Palette"].value]
            indices = unpack(s["BlockStates"].value, bits_for(len(palette), 4), 4096, data_version < 2529)
            out.append((s["Y"].value, [palette[i] for i in indices]))
    return sorted(out, reverse=True)


def surface(root, cap=None):
    """[z][x] -> [name, y] of the topmost visible block (None for empty columns)."""
    secs = sections(root)
    grid = []
    for z in range(16):
        row = []
        for x in range(16):
            found = None
            seen_air = cap is None
            for sy, names in secs:
                for ly in range(15, -1, -1):
                    y = sy * 16 + ly
                    if cap is not None and y > cap:
                        continue
                    name = names[ly * 256 + z * 16 + x]
                    if name in INVISIBLE:
                        seen_air = True
                        continue
                    if seen_air:
                        found = [name, y]
                        break
                if found:
                    break
            row.append(found)
        grid.append(row)
    return grid


def status_done(root):
    status = root.get("Status") or root.get("Level", Tagged(COMPOUND, {})).value.get("Status")
    return status is not None and status.value.removeprefix("minecraft:") in ("full", "postprocessed", "fullchunk")


def lz4_literals(data):
    """An LZ4 block holding `data` as literals only (valid, uncompressed)."""
    n = len(data)
    out = bytearray([min(n, 15) << 4])
    if n >= 15:
        rest = n - 15
        while rest >= 255:
            out.append(255)
            rest -= 255
        out.append(rest)
    return bytes(out) + data


def lz4_java_block(data):
    """lz4-java's LZ4BlockOutputStream framing, as Minecraft writes compression type 4."""
    block = lz4_literals(data)
    header = b"LZ4Block" + bytes([0x20 | 6]) + struct.pack("<iii", len(block), len(data), 0)
    end = b"LZ4Block" + bytes([0x10 | 6]) + struct.pack("<iii", 0, 0, 0)
    return header + block + end


def pack(values, bits, spanning):
    longs = []
    if spanning:
        total = len(values) * bits
        acc = 0
        for i, v in enumerate(values):
            acc |= v << (i * bits)
        for w in range((total + 63) // 64):
            word = (acc >> (64 * w)) & 0xFFFFFFFFFFFFFFFF
            longs.append(word - (1 << 64) if word >= 1 << 63 else word)
    else:
        per = 64 // bits
        for w in range((len(values) + per - 1) // per):
            word = 0
            for slot, v in enumerate(values[w * per : (w + 1) * per]):
                word |= v << (slot * bits)
            longs.append(word - (1 << 64) if word >= 1 << 63 else word)
    return longs


def legacy_chunk(x, z, data_version):
    """A 1.13-1.17 style chunk: stone up to y=60, grass on top, a water pool, 5 palette entries."""
    palette = ["minecraft:air", "minecraft:stone", "minecraft:grass_block", "minecraft:water", "minecraft:sand"]
    sections_out = []
    for sy in range(0, 4):
        values = []
        for ly in range(16):
            y = sy * 16 + ly
            for zz in range(16):
                for xx in range(16):
                    pool = 4 <= xx < 8 and 4 <= zz < 8
                    if y < 60:
                        values.append(4 if pool and y == 59 else 1)
                    elif y == 60:
                        values.append(3 if pool else 2)
                    else:
                        values.append(0)
        compound = {
            "Y": Tagged(1, sy),
            "Palette": Tagged(LIST, [Tagged(COMPOUND, {"Name": Tagged(STRING, n)}) for n in palette], COMPOUND),
            "BlockStates": Tagged(LONG_ARRAY, pack(values, 4, data_version < 2529)),
        }
        sections_out.append(Tagged(COMPOUND, compound))
    level = {
        "xPos": Tagged(INT, x),
        "zPos": Tagged(INT, z),
        "Status": Tagged(STRING, "full"),
        "Sections": Tagged(LIST, sections_out, COMPOUND),
        "Biomes": Tagged(INT_ARRAY, [1] * 1024),
    }
    return Tagged(COMPOUND, {"DataVersion": Tagged(INT, data_version), "Level": Tagged(COMPOUND, level)})


def ancient_chunk(x, z):
    """A pre-1.13 chunk (numeric block ids, no palette): unsupported."""
    level = {
        "xPos": Tagged(INT, x),
        "zPos": Tagged(INT, z),
        "TerrainPopulated": Tagged(1, 1),
        "Sections": Tagged(
            LIST,
            [Tagged(COMPOUND, {"Y": Tagged(1, 0), "Blocks": Tagged(BYTE_ARRAY, [1] * 4096)})],
            COMPOUND,
        ),
    }
    return Tagged(COMPOUND, {"Level": Tagged(COMPOUND, level)})


def main():
    world = Path(sys.argv[1])
    OUT.mkdir(parents=True, exist_ok=True)
    expected = {}

    # Overworld: four full chunks and one unfinished one from r.-1.0.mca.
    source = (world / "world/region/r.-1.0.mca").read_bytes()
    chunks = {index: (index, c, p) for index, c, p in region_chunks(source)}
    picked = [52, 53, 84, 85]
    unfinished = next(i for i, c, p in chunks.values() if not status_done(chunk_nbt(c, p).value))
    (OUT / "overworld.mca").write_bytes(build_region([chunks[i] for i in picked + [unfinished]]))
    expected["overworld"] = {"unfinished": unfinished, "chunks": {}}
    for i in picked:
        root = chunk_nbt(chunks[i][1], chunks[i][2]).value
        expected["overworld"]["chunks"][str(i)] = surface(root)

    # Nether: two small full chunks from r.0.0.mca, rendered below the roof.
    source = (world / "world_nether/DIM-1/region/r.0.0.mca").read_bytes()
    full = sorted(
        ((len(p), i, c, p) for i, c, p in region_chunks(source) if status_done(chunk_nbt(c, p).value)),
    )[:2]
    (OUT / "nether.mca").write_bytes(build_region([(i, c, p) for _, i, c, p in full]))
    expected["nether"] = {"cap": NETHER_CAP, "chunks": {}}
    for _, i, c, p in full:
        expected["nether"]["chunks"][str(i)] = surface(chunk_nbt(c, p).value, NETHER_CAP)

    # Entities: the overworld entity chunk.
    source = (world / "world/entities/r.-1.0.mca").read_bytes()
    entity_chunks = region_chunks(source)
    (OUT / "entities.mca").write_bytes(build_region(entity_chunks))
    expected["entities"] = []
    for i, c, p in entity_chunks:
        root = chunk_nbt(c, p).value
        for entity in root["Entities"].value:
            e = entity.value
            expected["entities"].append([e["id"].value] + [v.value for v in e["Pos"].value])

    # Compressions: one recorded chunk stored as gzip (1), zlib (2), none (3) and LZ4 (4).
    index, compression, payload = chunks[52]
    raw = zlib.decompress(payload)
    variants = [(0, 1, gzip.compress(raw)), (1, 2, zlib.compress(raw)), (2, 3, raw), (3, 4, lz4_java_block(raw))]
    (OUT / "compressions.mca").write_bytes(build_region(variants))
    expected["compressions"] = surface(chunk_nbt(compression, payload).value)

    # Older formats, synthesized: 1.16 (no spanning), 1.13 (spanning), pre-1.13 (unsupported).
    legacy = [
        (0, 2, zlib.compress(write(legacy_chunk(0, 0, 2586)))),
        (1, 2, zlib.compress(write(legacy_chunk(1, 0, 1631)))),
        (2, 2, zlib.compress(write(ancient_chunk(2, 0)))),
    ]
    (OUT / "legacy.mca").write_bytes(build_region(legacy))
    expected["legacy"] = {
        "1.16": surface(chunk_nbt(2, legacy[0][2]).value),
        "1.13": surface(chunk_nbt(2, legacy[1][2]).value),
    }

    (OUT / "expected.json").write_text(json.dumps(expected, separators=(",", ":")) + "\n")
    for path in sorted(OUT.iterdir()):
        print(f"{path.name}: {path.stat().st_size} bytes", file=sys.stderr)


if __name__ == "__main__":
    main()
