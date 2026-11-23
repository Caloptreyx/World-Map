#!/usr/bin/env bash
# Regenerates tests/fixtures/mcaselector/ with the real MCA Selector (headless CLI) as the oracle.
#
# Needs docker and network (first run downloads the release jar, OpenJFX jars from Maven Central and
# the eclipse-temurin:21-jdk image). Never writes the source world: it is copied to $WORK first.
# Headless mode itself runs on a plain JRE, but the Structures filter loads JavaFX classes
# (StructureRegistry renders icons via javafx.embed.swing), and javafx.swing needs the JDK-only module
# jdk.unsupported.desktop, hence a JDK image plus the OpenJFX module jars and the software renderer.
#
#   scripts/mcaselector_oracle.sh [source-world-dir]
#
# Env: WORK (scratch dir, default /tmp/oracle), IMAGE (java runtime image).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC=${1:-/var/lib/calagopus-wings/volumes/a8e9078c-577b-4835-b31b-1a5e1830e11c/world}
WORK=${WORK:-/tmp/oracle}
OUT=$ROOT/tests/fixtures/mcaselector
IMAGE=${IMAGE:-eclipse-temurin:21-jdk}
VERSION=2.9
JAR=mcaselector-$VERSION.jar
JAR_SHA256=8e35eaa6a5119ce711b53530c22aaac531332e8fdffda2357d16eb6f52327267
FX=21.0.2
FX_SHA256="a8f3674bac12b07acda55191579586cfe9f842b91bf06bee08b14d06397a3ffd  fx/javafx-base-$FX-linux.jar
c6ddb7e90c0fe256c212fd331d297a03a4840c61a6b330893ffc54c2d7c6326b  fx/javafx-controls-$FX-linux.jar
a1951e6db7616f143b2a4bc31e6cae26960e7956658015839f60830a9cf5016b  fx/javafx-graphics-$FX-linux.jar
67a4369cd556ed9b8b6970116e5f5f7f55ef64e54aaddbcc3f7950f7590631b5  fx/javafx-swing-$FX-linux.jar"

mkdir -p "$WORK"
cd "$WORK"
if [ ! -f "$JAR" ]; then
	curl -fsSL -o "$JAR" "https://github.com/Querz/mcaselector/releases/download/$VERSION/$JAR"
fi
mkdir -p fx
for m in base controls graphics swing; do
	[ -f "fx/javafx-$m-$FX-linux.jar" ] || curl -fsSL -o "fx/javafx-$m-$FX-linux.jar" \
		"https://repo1.maven.org/maven2/org/openjfx/javafx-$m/$FX/javafx-$m-$FX-linux.jar"
done
printf '%s  %s\n%s\n' "$JAR_SHA256" "$JAR" "$FX_SHA256" | sha256sum -c -

# One short-lived container per MCA Selector run, removed afterwards; $WORK is mounted at /w.
# Runs as the calling user so the outputs are not root-owned.
mcas() {
	nice -n 19 docker run --rm --user "$(id -u):$(id -g)" -v "$WORK:/w" -w /w "$IMAGE" \
		java -Duser.home=/tmp -Dprism.order=sw --module-path /w/fx --add-modules javafx.controls,javafx.swing \
		-jar "/w/$JAR" --process-threads 1 --write-threads 1 "$@" </dev/null >>"$WORK/mcaselector.log" 2>&1 ||
		{ echo "MCA Selector failed: $*" >&2; tail -20 "$WORK/mcaselector.log" >&2; return 1; }
}

rm -rf full trim change-* import-* inputs out mcaselector.log
mkdir -p inputs out

# ---- input world: copy (region/, entities/), then trim with MCA Selector itself -----------------
mkdir full
cp -r "$SRC/region" "$SRC/entities" full/
cp -r full trim
TRIM_QUERY='!(xPos >= -10 AND xPos <= 1 AND zPos >= -2 AND zPos <= 7)'
mcas --mode delete --world trim --query "$TRIM_QUERY"

# ---- (a) selections ---------------------------------------------------------------------------
# name<TAB>query; every query is run on the trimmed world with --mode select.
cat >inputs/queries.tsv <<'EOF'
xpos-zpos	xPos >= 0 AND zPos < 0
inhabited-lt-1min	InhabitedTime < "1 minute"
inhabited-gt-0	InhabitedTime > 0
status-full	Status = "minecraft:full"
status-or-unnamespaced	Status = "carvers" OR Status = "biomes"
not-group	!(Status = "minecraft:structure_starts") AND xPos < -5
palette-contains	Palette contains "minecraft:water"
palette-contains-all	Palette contains "minecraft:sand,minecraft:lapis_ore"
palette-intersects	Palette intersects "minecraft:bedrock,minecraft:glow_lichen"
palette-equals	Palette = "minecraft:air"
biome-contains	Biome contains "minecraft:river"
biome-intersects	Biome intersects "minecraft:warm_ocean,minecraft:river"
entities-amount	#Entities > 0
entities-contains	Entities contains "minecraft:rabbit"
proto-entities-amount	#ProtoEntities > 0
tile-entities-amount	#TileEntities > 0
lastupdate-and-or	LastUpdate > 1000000 AND (xPos = -10 OR zPos = 7)
structures-contains	Structures contains "minecraft:shipwreck"
circle	Circle contains "-8;4;3"
dataversion	DataVersion = 4189
lightpopulated	LightPopulated = 0
timestamp	Timestamp > "2026-10-01"
compression	Compression = "zlib"
EOF
mkdir -p out/select
while IFS=$'\t' read -r name query; do
	mcas --mode select --world trim --query "$query" --output "out/select/$name.csv"
done <inputs/queries.tsv

# Whole-region line: a radius selection that covers every chunk of r.-1.0 collapses to "-1;0".
# The bounds selection (input, written here) limits the radius to regions r.-1.0 and r.0.0.
printf -- '-1;0\n0;0\n' >inputs/bounds.csv
mcas --mode select --world trim --query 'Status = "minecraft:full"' --radius 33 \
	--selection inputs/bounds.csv --output out/select/radius-whole-region.csv

# Headless select never writes an inverted selection (it always starts from a new Selection), so the
# inverted CSV is hand-written per Selection.saveToFile; MCA Selector then resolves it against the
# trimmed world (Selection.readFromFile + isChunkSelected) as an oracle.
printf 'inverted\n0;-1\n-1;0;-9;7\n-1;0;-10;1\n' >out/select/handwritten-inverted.csv
mcas --mode select --world trim --query 'xPos >= -1000000' \
	--selection out/select/handwritten-inverted.csv --output out/select/inverted-resolved.csv

# ---- (c) change fields on 2 chunks (one full chunk with entities, one proto chunk) ------------
CHUNKS_QUERY='(xPos = -9 AND zPos = 7) OR (xPos = -2 AND zPos = 0)'
mcas --mode select --world trim --query "$CHUNKS_QUERY" --output inputs/change-selection.csv
cp -r trim change-before
mcas --mode delete --world change-before --query "!($CHUNKS_QUERY)"
cat >inputs/changes.tsv <<'EOF'
inhabited-lastupdate	InhabitedTime = 1200, LastUpdate = 0
status	Status = "empty"
delete-sections	DeleteSections = "4:"
delete-entities	DeleteEntities = true
EOF
while IFS=$'\t' read -r name fields; do
	cp -r change-before "change-$name"
	mcas --mode change --world "change-$name" --selection inputs/change-selection.csv --fields "$fields"
done <inputs/changes.tsv

# ---- (d) import with offset into an empty world ----------------------------------------------
# ImportJob returns "no files" when the target world has no region file at all (it lists the target
# regions first), so each target gets a header-only placeholder r.9.9.mca far away from the imported
# chunks; it is left untouched by the import and not copied into the fixtures. entities/ must exist
# for entities to be imported.
for t in import-a import-b; do
	mkdir -p "$t/region" "$t/entities"
	head -c 8192 /dev/zero >"$t/region/r.9.9.mca"
done
mcas --mode import --world import-a --source-world change-before --x-offset 33 --z-offset -2
mcas --mode import --world import-b --source-world change-before --x-offset -40 --z-offset 5 \
	--y-offset 2 --sections "0:3"
rm import-a/region/r.9.9.mca import-b/region/r.9.9.mca

# ---- copy into the fixture folder --------------------------------------------------------------
rm -rf "$OUT"/world "$OUT"/select "$OUT"/inputs "$OUT"/change "$OUT"/import "$OUT"/queries-expected.json
mkdir -p "$OUT"
cp -r trim "$OUT/world"
cp -r out/select "$OUT/select"
cp -r inputs "$OUT/inputs"
mkdir -p "$OUT/change"
cp -r change-before "$OUT/change/before"
while IFS=$'\t' read -r name _; do cp -r "change-$name" "$OUT/change/$name"; done <inputs/changes.tsv
mkdir -p "$OUT/import"
cp -r import-a "$OUT/import/offset-33_-2"
cp -r import-b "$OUT/import/offset--40_5-y2-sections-0_3"
find "$OUT" -type d -empty -delete

# queries-expected.json: query -> sorted [[x, z], ...] read back from MCA Selector's CSVs.
python3 - "$OUT" <<'EOF'
import json, os, sys

out = sys.argv[1]
expected = {}
with open(os.path.join(out, "inputs", "queries.tsv")) as f:
    for line in f:
        name, query = line.rstrip("\n").split("\t")
        chunks = []
        with open(os.path.join(out, "select", name + ".csv")) as csv:
            for row in csv.read().splitlines():
                p = [int(v) for v in row.split(";")]
                if len(p) == 2:
                    chunks += [[p[0] * 32 + x, p[1] * 32 + z] for x in range(32) for z in range(32)]
                else:
                    chunks.append([p[2], p[3]])
        expected[query] = sorted(chunks)
# One query per line, so diffs stay readable.
with open(os.path.join(out, "queries-expected.json"), "w") as f:
    f.write("{\n" + ",\n".join(json.dumps(q) + ": " + json.dumps(c, separators=(",", ":")) for q, c in expected.items()) + "\n}\n")
EOF

grep -iE 'error|exception' "$WORK/mcaselector.log" && { echo "MCA Selector reported errors, see $WORK/mcaselector.log" >&2; exit 1; }
du -ab "$OUT" | sort -k2
