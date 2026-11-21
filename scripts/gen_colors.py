#!/usr/bin/env python3
"""Generates the block and biome colour tables in data/ from a vanilla Minecraft client jar.

Block colours are the alpha-weighted average of the texture a block shows from above: the
blockstate's first variant (or first multipart part) gives the model, the model's "up" face (of
its largest element) gives the texture; models without an up face (flowers, cross plants) use
their first face, then their particle texture. Animated textures use their first frame.
Grayscale textures the game tints by biome are marked `grass`, `foliage`, `dry_foliage` or
`water`; a few tinted blocks with a constant tint get it baked in.

Biome colours follow the game: water from the biome's `water_color`; grass and foliage from the
biome's own colour when it has one, otherwise from the grass/foliage colormaps at the biome's
temperature and downfall; the swamp and dark forest grass modifiers as the client applies them
(swamp uses its cold-noise colour, the more common one).

Only the colours derived from the jar are written; no textures are copied.

Usage: python3 scripts/gen_colors.py [version]   (default: the latest release)
Needs network access to piston-meta.mojang.com and only the Python standard library.
"""

import io
import json
import struct
import sys
import urllib.request
import zipfile
import zlib
from pathlib import Path

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
OUT = Path(__file__).resolve().parent.parent / "src" / "data"

GRASS = {
    "grass_block",
    "short_grass",
    "grass",
    "tall_grass",
    "fern",
    "large_fern",
    "potted_fern",
    "sugar_cane",
    "bush",
}
FOLIAGE_EXTRA = {"vine", "mangrove_leaves"}
WATER = {"water", "bubble_column"}
# Tinted blocks whose tint does not depend on the biome (the client's fixed colours).
FIXED_TINT = {
    "birch_leaves": 0x80A755,
    "spruce_leaves": 0x619961,
    "lily_pad": 0x208030,
    "attached_melon_stem": 0xE0C71C,
    "attached_pumpkin_stem": 0xE0C71C,
    "melon_stem": 0x7AB33A,
    "pumpkin_stem": 0x7AB33A,
    "redstone_wire": 0x9F0000,
}
# Leaves the game does not tint.
UNTINTED_LEAVES = {"cherry_leaves", "azalea_leaves", "flowering_azalea_leaves", "pale_oak_leaves"}


def fetch(url):
    with urllib.request.urlopen(url) as response:
        return response.read()


def client_jar(version):
    """The client jar of `version` (latest release by default), or a local jar file."""
    if version and version.endswith(".jar"):
        jar = zipfile.ZipFile(version)
        return json.loads(jar.read("version.json"))["id"], jar
    manifest = json.loads(fetch(MANIFEST))
    version = version or manifest["latest"]["release"]
    entry = next(v for v in manifest["versions"] if v["id"] == version)
    meta = json.loads(fetch(entry["url"]))
    print(f"downloading the {version} client jar", file=sys.stderr)
    return version, zipfile.ZipFile(io.BytesIO(fetch(meta["downloads"]["client"]["url"])))


def decode_png(data):
    """RGBA rows of a non-interlaced PNG (all colour types, bit depths up to 8)."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos, idat, palette, trns = 8, b"", None, None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert interlace == 0 and depth <= 8
        elif kind == b"PLTE":
            palette = [tuple(body[i : i + 3]) for i in range(0, len(body), 3)]
        elif kind == b"tRNS":
            trns = body
        elif kind == b"IDAT":
            idat += body
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
    bits = depth * channels
    stride = (width * bits + 7) // 8
    bpp = max(1, bits // 8)
    raw = zlib.decompress(idat)
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        kind, line = raw[i], bytearray(raw[i + 1 : i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if kind == 1:
                line[x] = (line[x] + a) & 255
            elif kind == 2:
                line[x] = (line[x] + b) & 255
            elif kind == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        prev = line
        samples = []
        if depth == 8:
            samples = list(line)
        else:
            per, mask = 8 // depth, (1 << depth) - 1
            for byte in line:
                for k in range(per):
                    samples.append((byte >> (8 - depth * (k + 1))) & mask)
        row = []
        for x in range(width):
            if color == 3:
                index = samples[x]
                r, g, b = palette[index]
                alpha = trns[index] if trns and index < len(trns) else 255
            elif color == 0:
                v = samples[x] * 255 // ((1 << depth) - 1)
                r = g = b = v
                alpha = 255
            elif color == 4:
                r = g = b = samples[2 * x]
                alpha = samples[2 * x + 1]
            elif color == 2:
                r, g, b = samples[3 * x : 3 * x + 3]
                alpha = 255
            else:
                r, g, b, alpha = samples[4 * x : 4 * x + 4]
            row.append((r, g, b, alpha))
        rows.append(row)
    return rows


def average(rows):
    """Alpha-weighted average colour of the first square frame; None when fully transparent."""
    width = len(rows[0])
    total = [0, 0, 0]
    weight = 0
    for row in rows[:width]:
        for r, g, b, a in row:
            total[0] += r * a
            total[1] += g * a
            total[2] += b * a
            weight += a
    if weight == 0:
        return None
    return tuple(round(c / weight) for c in total)


class Assets:
    def __init__(self, jar):
        self.jar = jar
        self.models = {}

    def json(self, path):
        return json.loads(self.jar.read(path))

    def model(self, name):
        name = name.removeprefix("minecraft:")
        if name not in self.models:
            try:
                data = self.json(f"assets/minecraft/models/{name}.json")
            except KeyError:
                data = {}
            textures, elements = {}, None
            if "parent" in data and not data["parent"].startswith("builtin/"):
                parent = self.model(data["parent"])
                textures.update(parent["textures"])
                elements = parent["elements"]
            textures.update(data.get("textures", {}))
            self.models[name] = {"textures": textures, "elements": data.get("elements", elements)}
        return self.models[name]

    def texture(self, path):
        path = path.removeprefix("minecraft:")
        try:
            return average(decode_png(self.jar.read(f"assets/minecraft/textures/{path}.png")))
        except KeyError:
            return None


def resolve(textures, ref, depth=0):
    """A texture path from a `#key` reference; newer models may give `{"sprite": path}`."""
    while depth < 16:
        if isinstance(ref, dict):
            ref = ref.get("sprite")
        if not ref or not ref.startswith("#"):
            return ref
        ref = textures.get(ref[1:])
        depth += 1
    return None


def top_face(model):
    """The texture reference and tint flag of what is seen from above."""
    elements = model["elements"] or []
    best = None
    for element in elements:
        faces = element.get("faces", {})
        if "up" in faces:
            frm, to = element.get("from", [0, 0, 0]), element.get("to", [16, 16, 16])
            area = (to[0] - frm[0]) * (to[2] - frm[2])
            if best is None or area > best[0]:
                best = (area, faces["up"])
    if best is None:
        for element in elements:
            for face in element.get("faces", {}).values():
                best = (0, face)
                break
            if best:
                break
    if best is None:
        return model["textures"].get("particle"), False
    face = best[1]
    return face.get("texture"), "tintindex" in face


def first_model(blockstate):
    if "variants" in blockstate:
        variant = next(iter(blockstate["variants"].values()))
    else:
        parts = blockstate.get("multipart", [])
        part = next((p for p in parts if "when" not in p), parts[0] if parts else {"apply": {}})
        variant = part["apply"]
    if isinstance(variant, list):
        variant = variant[0]
    return variant.get("model")


def tint_kind(name, tinted):
    if name in WATER:
        return "water"
    if name in FIXED_TINT:
        return "fixed"
    if not tinted:
        return "-"
    if name in GRASS:
        return "grass"
    if name == "leaf_litter":
        return "dry_foliage"
    if (name.endswith("_leaves") and name not in UNTINTED_LEAVES) or name in FOLIAGE_EXTRA:
        return "foliage"
    return "foliage"


def blocks(assets):
    lines = []
    prefix = "assets/minecraft/blockstates/"
    for path in sorted(n for n in assets.jar.namelist() if n.startswith(prefix) and n.endswith(".json")):
        name = path[len(prefix) : -5]
        model_name = first_model(assets.json(path))
        if not model_name:
            continue
        model = assets.model(model_name)
        ref, tinted = top_face(model)
        texture = resolve(model["textures"], ref)
        colour = assets.texture(texture) if texture else None
        if colour is None and model["textures"].get("particle"):
            colour = assets.texture(resolve(model["textures"], model["textures"]["particle"]))
        if colour is None:
            continue
        kind = tint_kind(name, tinted or name in WATER)
        if kind == "fixed":
            tint = FIXED_TINT[name]
            colour = tuple(c * ((tint >> s) & 255) // 255 for c, s in zip(colour, (16, 8, 0)))
            kind = "-"
        lines.append(f"{name}\t{colour[0]:02x}{colour[1]:02x}{colour[2]:02x}\t{kind}")
    return lines


def colormap(rows, temperature, downfall):
    temperature = min(max(temperature, 0.0), 1.0)
    downfall = min(max(downfall, 0.0), 1.0) * temperature
    x = int((1.0 - temperature) * 255)
    y = int((1.0 - downfall) * 255)
    r, g, b, _ = rows[y][x]
    return (r << 16) | (g << 8) | b


def biomes(assets):
    grass = decode_png(assets.jar.read("assets/minecraft/textures/colormap/grass.png"))
    foliage = decode_png(assets.jar.read("assets/minecraft/textures/colormap/foliage.png"))
    lines = []
    prefix = "data/minecraft/worldgen/biome/"
    for path in sorted(n for n in assets.jar.namelist() if n.startswith(prefix) and n.endswith(".json")):
        name = path[len(prefix) : -5]
        data = assets.json(path)
        effects = data.get("effects", {})
        temperature, downfall = data.get("temperature", 0.5), data.get("downfall", 0.5)
        hex_colour = lambda key: int(effects[key].lstrip("#"), 16) if isinstance(effects.get(key), str) else effects.get(key)
        grass_colour = hex_colour("grass_color") or colormap(grass, temperature, downfall)
        modifier = effects.get("grass_color_modifier")
        if modifier == "swamp":
            grass_colour = 0x4C763C
        elif modifier == "dark_forest":
            grass_colour = ((grass_colour & 0xFEFEFE) + 0x28340A) >> 1
        foliage_colour = hex_colour("foliage_color") or colormap(foliage, temperature, downfall)
        dry = hex_colour("dry_foliage_color") or 0x7B5334
        water = hex_colour("water_color") or 0x3F76E4
        lines.append(f"{name}\t{grass_colour:06x}\t{foliage_colour:06x}\t{dry:06x}\t{water:06x}")
    return lines


def main():
    version, jar = client_jar(sys.argv[1] if len(sys.argv) > 1 else None)
    assets = Assets(jar)
    OUT.mkdir(exist_ok=True)
    header = f"# Generated by scripts/gen_colors.py from the Minecraft {version} client jar.\n"
    block_lines = blocks(assets)
    (OUT / "blocks.tsv").write_text(
        header + "# block\taverage top colour\tbiome tint (grass, foliage, dry_foliage, water or -)\n" + "\n".join(block_lines) + "\n"
    )
    biome_lines = biomes(assets)
    (OUT / "biomes.tsv").write_text(
        header + "# biome\tgrass\tfoliage\tdry foliage\twater\n" + "\n".join(biome_lines) + "\n"
    )
    print(f"{len(block_lines)} blocks, {len(biome_lines)} biomes", file=sys.stderr)


if __name__ == "__main__":
    main()
