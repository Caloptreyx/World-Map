//! MCA Selector's chunk import transforms (`ImportJob`, `ChunkFilter.Relocate`/`Merge` of every
//! version): moving chunks by an offset (x/z in chunks, y in sections) and merging the sections of
//! an imported chunk into an existing one ("sections" ranges).
//!
//! Formats: region chunks of 1.13-1.17 (`Level.*`, DataVersion 1451..2843) and 1.18+ (root,
//! 2844+), 1.17+ `entities/` chunks and 1.14+ `poi/` chunks. Older chunks are refused.
//!
//! Where MCA Selector has an evident bug this module does what the code intends instead (each
//! case is noted where it happens): the y offset of poi sections, biome shifts by negative
//! offsets, merge ranges applied with the wrong sign, merged structure starts lost when the
//! destination lacks the parent compound. Additions over MCA Selector: the 1.20.5+/1.21.5+ block
//! position fields of entities and block entities (int arrays such as `leash`, `sleeping_pos`,
//! `block_pos`), bees and beehives, villager memories in their `value` wrapper, the `equipment`
//! items, proto-chunk entity lists (moved, not merged), and `Heightmaps` dropped when blocks move
//! vertically or sections are merged (the game rebuilds missing heightmaps when it loads a chunk).
//! Like MCA Selector 2.9 (checked against its output in `tests/fixtures/mcaselector/import/`),
//! moved entities get new random UUIDs, Paper's `Paper.Origin` stays, and sections below `yPos`
//! (the light-only section under the world) are dropped by a move.
use crate::nbt::{Compound, Tag};

/// The import offset: `x`, `z` in chunks, `y` in sections (16 blocks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Offset {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// MCA Selector section ranges ("-4:3,10", ":0", "5:"; empty = all).
///
/// `is_all` is MCA Selector's "no sections given": the import replaces whole chunks and does not
/// merge. Any other value (including ":" or "true", which contain every section) merges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranges {
    /// Inclusive (from, to) pairs; `None` = all sections, no merge.
    ranges: Option<Vec<(i32, i32)>>,
}

impl Ranges {
    pub fn all() -> Self {
        Self { ranges: None }
    }

    pub fn contains(&self, section_y: i32) -> bool {
        self.contains_wide(i64::from(section_y))
    }

    pub fn is_all(&self) -> bool {
        self.ranges.is_none()
    }

    fn contains_wide(&self, y: i64) -> bool {
        match &self.ranges {
            None => true,
            Some(ranges) => ranges
                .iter()
                .any(|&(from, to)| i64::from(from) <= y && y <= i64::from(to)),
        }
    }

    /// Whether the source section a destination y came from (`y - y_offset`) is selected.
    fn selects(&self, y: i32, y_offset: i32) -> bool {
        self.contains_wide(i64::from(y) - i64::from(y_offset))
    }

    /// Whether one range holds both ys (after removing the y offset).
    fn selects_both(&self, a: i32, b: i32, y_offset: i32) -> bool {
        let (a, b) = (
            i64::from(a) - i64::from(y_offset),
            i64::from(b) - i64::from(y_offset),
        );
        match &self.ranges {
            None => true,
            Some(ranges) => ranges.iter().any(|&(from, to)| {
                let (from, to) = (i64::from(from), i64::from(to));
                from <= a && a <= to && from <= b && b <= to
            }),
        }
    }
}

/// Parses MCA Selector's section ranges (`RangeParser.parseRanges(text, ",")`): comma separated
/// `n`, `from:to`, `:to`, `from:` or `:` (inclusive; open ends are unbounded), `true` = every
/// section. Blank text = [`Ranges::all`]. As in Java's `String.split`, trailing empty items are
/// ignored and any other empty item is an error.
pub fn parse_ranges(text: &str) -> Result<Ranges, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Ranges::all());
    }
    if text == "true" {
        return Ok(Ranges {
            ranges: Some(vec![(i32::MIN, i32::MAX)]),
        });
    }
    let mut parts: Vec<&str> = text.split(',').collect();
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    let ranges = parts
        .into_iter()
        .map(|part| {
            parse_range(part).ok_or_else(|| format!("invalid section range \"{}\"", part.trim()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Ranges {
        ranges: Some(ranges),
    })
}

/// One range, as `RangeParser.parseRange` (`^(-?\d*)(:?)(-?\d*)$`): without a colon only the
/// first number counts ("5-3" is 5, as in MCA Selector).
fn parse_range(part: &str) -> Option<(i32, i32)> {
    let part = part.trim();
    if part.is_empty() {
        return None;
    }
    let (from, rest) = signed_digits(part);
    let (divider, rest) = match rest.strip_prefix(':') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let (to, rest) = signed_digits(rest);
    if !rest.is_empty() {
        return None;
    }
    let from = if from.is_empty() {
        i32::MIN
    } else {
        from.parse().ok()?
    };
    let to = if to.is_empty() {
        i32::MAX
    } else {
        to.parse().ok()?
    };
    Some(if divider { (from, to) } else { (from, from) })
}

/// Splits a leading `-?[0-9]*` off.
fn signed_digits(text: &str) -> (&str, &str) {
    let sign = usize::from(text.starts_with('-'));
    let digits = text[sign..].bytes().take_while(u8::is_ascii_digit).count();
    text.split_at(sign + digits)
}

/// The offset in the units each field needs.
struct Shift {
    /// chunks
    cx: i32,
    cz: i32,
    /// blocks
    bx: i32,
    by: i32,
    bz: i32,
    /// sections
    sy: i32,
}

impl Shift {
    fn new(offset: Offset) -> Self {
        Self {
            cx: offset.x,
            cz: offset.z,
            bx: offset.x.wrapping_mul(16),
            by: offset.y.wrapping_mul(16),
            bz: offset.z.wrapping_mul(16),
            sy: offset.y,
        }
    }
}

/// First 1.13 snapshot (17w47a, the flattening).
const V1_13: i32 = 1451;
/// 19w02a: "full" replaces "postprocessed" as the status of new chunks.
const V19W02A: i32 = 1921;
/// 19w36a: 3D biomes (1024 ints).
const V19W36A: i32 = 2203;
/// 20w45a: entities move to `entities/` files.
const V20W45A: i32 = 2681;
/// 21w06a..21w14a: snapshots with an extended world height in the Level format.
const V21W06A: i32 = 2694;
const V21W15A: i32 = 2709;
/// 21w43a: the root chunk format (1.18).
const V21W43A: i32 = 2844;
/// 1.20 pre-release 5: namespaced status of new chunks.
const V1_20_PRE5: i32 = 3458;

/// `Helper.getDataVersion`: the root DataVersion, else the Level's, else 100.
fn data_version(root: &Compound) -> i32 {
    root.get("DataVersion")
        .or_else(|| {
            root.compound("Level")
                .and_then(|level| level.get("DataVersion"))
        })
        .and_then(java_int)
        .unwrap_or(100)
}

fn supported_version(root: &Compound, what: &str) -> Result<i32, String> {
    let version = data_version(root);
    if version < V1_13 {
        return Err(format!(
            "{what} chunk with DataVersion {version} is from before Minecraft 1.13; only 1.13+ chunks can be imported"
        ));
    }
    Ok(version)
}

/// Moves a `region/` chunk by `offset` (MCA Selector `ChunkFilter.Relocate`).
///
/// Updated in every format: `xPos`/`zPos` (+x/z chunks), block entity `x`/`y`/`z` (end gateway
/// `ExitPortal`/`exit_portal`, spawner and beehive entities), tick `x`/`y`/`z`, structure
/// `References` (packed chunk positions) and starts (`ChunkX`/`ChunkZ`, `BB`, `Processed[].X/Z`,
/// children `BB`, `TPX..`, `PosX..`, `Entrances`, `junctions[].source_x..`; `INVALID` starts
/// untouched), the per-section short lists (shifted by y), sections `Y` (sections leaving the
/// chunk's section range are dropped) and entity lists. 1.13-1.17: `Level.TileEntities`,
/// `TileTicks`, `LiquidTicks`, `Structures.Starts`, `Lights`, `LiquidsToBeTicked`, `ToBeTicked`,
/// `PostProcessing`, `Entities` and the 3D `Biomes` (1.15+, rows shifted by 4 per section, new rows
/// 0). 1.18+: `block_entities`, `block_ticks`, `fluid_ticks`, `structures.starts`,
/// `PostProcessing`, `sections`, proto-chunk `entities`. `yPos` stays (it is the world's bottom).
pub fn relocate_region(root: &mut Compound, offset: Offset) -> Result<(), String> {
    let version = supported_version(root, "region")?;
    let shift = Shift::new(offset);
    if version >= V21W43A {
        relocate_root(root, &shift);
        return Ok(());
    }
    let level = root
        .compound_mut("Level")
        .ok_or("region chunk without a Level compound")?;
    relocate_level(level, version, &shift);
    Ok(())
}

fn relocate_level(level: &mut Compound, version: i32, shift: &Shift) {
    add_chunk_pos(level, shift);
    // MCA Selector stops relocating Level.Entities at 20w45a; proto chunks still keep them there
    for_compounds(level, "Entities", |entity| relocate_entity(entity, shift));
    for_compounds(level, "TileEntities", |tile| {
        relocate_block_entity(tile, shift)
    });
    for key in ["TileTicks", "LiquidTicks"] {
        for_compounds(level, key, |tick| add_xyz(tick, "x", "y", "z", shift));
    }
    if let Some(structures) = level.compound_mut("Structures") {
        relocate_structures(structures, "Starts", shift);
    }
    for key in [
        "Lights",
        "LiquidsToBeTicked",
        "ToBeTicked",
        "PostProcessing",
    ] {
        shift_index_lists(level, key, shift.sy);
    }
    let range = if (V21W06A..V21W15A).contains(&version) {
        section_range(level, level.list("Sections").unwrap_or_default())
    } else {
        (0, 15)
    };
    if let Some(sections) = level.list_mut("Sections") {
        shift_sections(sections, shift.sy, range);
    }
    if version >= V19W36A && shift.sy != 0 {
        let sections = if (V21W06A..V21W15A).contains(&version) {
            (i64::from(range.1) - i64::from(range.0) + 1).max(0) as usize
        } else {
            16
        };
        shift_biomes(level, shift.sy, sections);
    }
    if shift.sy != 0 {
        level.remove("Heightmaps");
    }
}

fn relocate_root(root: &mut Compound, shift: &Shift) {
    add_chunk_pos(root, shift);
    for_compounds(root, "block_entities", |tile| {
        relocate_block_entity(tile, shift)
    });
    for key in ["block_ticks", "fluid_ticks"] {
        for_compounds(root, key, |tick| add_xyz(tick, "x", "y", "z", shift));
    }
    for_compounds(root, "entities", |entity| relocate_entity(entity, shift));
    if let Some(structures) = root.compound_mut("structures") {
        relocate_structures(structures, "starts", shift);
    }
    shift_index_lists(root, "PostProcessing", shift.sy);
    let range = section_range(root, root.list("sections").unwrap_or_default());
    if let Some(sections) = root.list_mut("sections") {
        shift_sections(sections, shift.sy, range);
    }
    if shift.sy != 0 {
        root.remove("Heightmaps");
    }
}

/// `xPos`/`zPos` (+ chunks); missing ones count as 0, as MCA Selector does.
fn add_chunk_pos(chunk: &mut Compound, shift: &Shift) {
    for (key, delta) in [("xPos", shift.cx), ("zPos", shift.cz)] {
        let value = chunk.get(key).and_then(java_int).unwrap_or(0);
        chunk.insert(key, Tag::Int(value.wrapping_add(delta)));
    }
}

/// The section range of a chunk (`Helper.findSectionRange`): lowest to highest `Y` of the
/// sections holding more than their `Y`, the lowest replaced by `yPos` when present, limited to
/// -127..126.
fn section_range(chunk: &Compound, sections: &[Tag]) -> (i32, i32) {
    let mut min = i32::MAX;
    let mut max = i32::MIN;
    for section in sections.iter().filter_map(Tag::as_compound) {
        if section.len() == 1 {
            continue;
        }
        let Some(y) = section.get("Y").and_then(java_int) else {
            continue;
        };
        min = min.min(y);
        max = max.max(y);
    }
    if let Some(y_pos) = chunk.get("yPos").and_then(java_int) {
        min = y_pos;
    }
    if min > max {
        (-127, 126)
    } else {
        (min.max(-127), max.min(126))
    }
}

/// `applyOffsetToSection` on every section: sections outside `range` before or after the move are
/// dropped, `Y` is rewritten as a byte; sections without `Y` stay, non-compounds go.
fn shift_sections(sections: &mut Vec<Tag>, dy: i32, (low, high): (i32, i32)) {
    sections.retain_mut(|tag| {
        let Some(section) = tag.as_compound_mut() else {
            return false;
        };
        let Some(y) = section.get("Y").and_then(java_int) else {
            return true;
        };
        let moved = i64::from(y) + i64::from(dy);
        if !(low..=high).contains(&y) || !(i64::from(low)..=i64::from(high)).contains(&moved) {
            return false;
        }
        section.insert("Y", Tag::Byte(moved as i8));
        true
    });
}

/// `Helper.applyOffsetToListOfShortTagLists`: a list indexed by section (from the bottom) moves
/// by `dy` entries; entries moved past either end are dropped, vacated ones become empty lists.
fn shift_index_lists(chunk: &mut Compound, key: &str, dy: i32) {
    if dy == 0 {
        return;
    }
    let Some(list) = chunk.list_mut(key) else {
        return;
    };
    let old = std::mem::take(list);
    let count = old.len() as i64;
    let mut moved: Vec<Tag> = (0..old.len()).map(|_| Tag::List(Vec::new())).collect();
    for (index, tag) in old.into_iter().enumerate() {
        let target = index as i64 + i64::from(dy);
        if (0..count).contains(&target) {
            moved[target as usize] = tag;
        }
    }
    *list = moved;
}

/// 3D biomes (4x4x4 cells, index `y * 16 + z * 4 + x`): rows move by 4 per section, vacated rows
/// become 0. (MCA Selector stops at the first row moved below the chunk, which empties every
/// column for negative offsets; this keeps the rows that stay inside.)
fn shift_biomes(level: &mut Compound, dy: i32, sections: usize) {
    let Some(biomes) = level.get_mut("Biomes").and_then(Tag::as_ints_mut) else {
        return;
    };
    let rows = sections * 4;
    if biomes.len() != rows * 16 {
        return;
    }
    let mut moved = vec![0; biomes.len()];
    for row in 0..rows {
        let target = row as i64 + i64::from(dy) * 4;
        if !(0..rows as i64).contains(&target) {
            continue;
        }
        let target = target as usize;
        moved[target * 16..target * 16 + 16].copy_from_slice(&biomes[row * 16..row * 16 + 16]);
    }
    *biomes = moved;
}

/// Structure references and starts (`applyOffsetToStructures`).
fn relocate_structures(structures: &mut Compound, starts_key: &str, shift: &Shift) {
    if let Some(references) = structures.compound_mut("References") {
        for (_, tag) in references.iter_mut() {
            if let Some(longs) = tag.as_longs_mut() {
                for packed in longs.iter_mut() {
                    *packed = shift_chunk_long(*packed, shift);
                }
            }
        }
    }
    let Some(starts) = structures.compound_mut(starts_key) else {
        return;
    };
    for (_, tag) in starts.iter_mut() {
        let Some(start) = tag.as_compound_mut() else {
            continue;
        };
        if start.str("id") == Some("INVALID") {
            continue;
        }
        add_int(start, "ChunkX", shift.cx);
        add_int(start, "ChunkZ", shift.cz);
        shift_bb(start.get_mut("BB"), shift);
        for_compounds(start, "Processed", |chunk| {
            add_int(chunk, "X", shift.cx);
            add_int(chunk, "Z", shift.cz);
        });
        for_compounds(start, "Children", |child| {
            add_xyz(child, "TPX", "TPY", "TPZ", shift);
            add_xyz(child, "PosX", "PosY", "PosZ", shift);
            shift_bb(child.get_mut("BB"), shift);
            if let Some(entrances) = child.list_mut("Entrances") {
                for entrance in entrances.iter_mut() {
                    shift_bb(Some(entrance), shift);
                }
            }
            for_compounds(child, "junctions", |junction| {
                add_xyz(junction, "source_x", "source_y", "source_z", shift);
            });
        });
    }
}

/// A chunk position packed as `x & 0xFFFFFFFF | z << 32`.
fn shift_chunk_long(packed: i64, shift: &Shift) -> i64 {
    let x = (packed as i32).wrapping_add(shift.cx);
    let z = ((packed >> 32) as i32).wrapping_add(shift.cz);
    (i64::from(z as u32) << 32) | i64::from(x as u32)
}

/// A bounding box `[I; x1, y1, z1, x2, y2, z2]`.
fn shift_bb(tag: Option<&mut Tag>, shift: &Shift) {
    if let Some(bb) = tag.and_then(Tag::as_ints_mut).filter(|bb| bb.len() == 6) {
        for (index, delta) in [shift.bx, shift.by, shift.bz, shift.bx, shift.by, shift.bz]
            .into_iter()
            .enumerate()
        {
            bb[index] = bb[index].wrapping_add(delta);
        }
    }
}

/// Block entity positions (`applyOffsetToTileEntity`). MCA Selector also moves the structure
/// block's `posX/posY/posZ`; those are relative to the block, so they stay here.
fn relocate_block_entity(tile: &mut Compound, shift: &Shift) {
    add_xyz(tile, "x", "y", "z", shift);
    let id = tile.str("id").unwrap_or_default().to_owned();
    match id.as_str() {
        "minecraft:end_gateway" => {
            add_xyz_in(tile, "ExitPortal", shift);
            add_pos(tile, "exit_portal", shift);
        }
        "minecraft:mob_spawner" | "minecraft:spawner" => {
            for_compounds(tile, "SpawnPotentials", |potential| {
                if let Some(entity) = potential.compound_mut("Entity") {
                    relocate_entity(entity, shift);
                }
                if let Some(entity) = potential
                    .compound_mut("data")
                    .and_then(|data| data.compound_mut("entity"))
                {
                    relocate_entity(entity, shift);
                }
            });
        }
        "minecraft:beehive" | "minecraft:bee_nest" => {
            add_xyz_in(tile, "FlowerPos", shift);
            add_pos(tile, "flower_pos", shift);
            for_compounds(tile, "Bees", |bee| {
                if let Some(entity) = bee.compound_mut("EntityData") {
                    relocate_entity(entity, shift);
                }
            });
            for_compounds(tile, "bees", |bee| {
                if let Some(entity) = bee.compound_mut("entity_data") {
                    relocate_entity(entity, shift);
                }
            });
        }
        _ => {}
    }
}

/// Block positions of the 1.20.5+ entity formats (int arrays), moved on any entity that has them.
const POS_ARRAYS: [&str; 10] = [
    "leash",
    "sleeping_pos",
    "block_pos",
    "anchor_pos",
    "home_pos",
    "bound_pos",
    "wander_target",
    "patrol_target",
    "beam_target",
    "hive_pos",
];

/// Entity positions (`applyOffsetToEntity` of the latest MCA Selector versions, plus the
/// additions listed in the module docs), recursing into passengers; the entity gets new random
/// UUIDs as MCA Selector does (`Helper.fixEntityUUID`), so copies do not collide.
fn relocate_entity(entity: &mut Compound, shift: &Shift) {
    add_double_pos(entity, "Pos", shift);
    add_xyz_in(entity, "Leash", shift);
    add_xyz(entity, "xTile", "yTile", "zTile", shift);
    add_xyz(entity, "SleepingX", "SleepingY", "SleepingZ", shift);
    for key in POS_ARRAYS {
        add_pos(entity, key, shift);
    }
    add_pos(entity, "flower_pos", shift);
    let id = entity.str("id").unwrap_or_default().to_owned();
    match id.as_str() {
        "minecraft:dolphin" => add_xyz(
            entity,
            "TreasurePosX",
            "TreasurePosY",
            "TreasurePosZ",
            shift,
        ),
        "minecraft:phantom" => add_xyz(entity, "AX", "AY", "AZ", shift),
        "minecraft:shulker" => add_xyz(entity, "APX", "APY", "APZ", shift),
        "minecraft:turtle" => {
            add_xyz(entity, "HomePosX", "HomePosY", "HomePosZ", shift);
            add_xyz(entity, "TravelPosX", "TravelPosY", "TravelPosZ", shift);
        }
        "minecraft:vex" => add_xyz(entity, "BoundX", "BoundY", "BoundZ", shift),
        "minecraft:wandering_trader" => add_xyz_in(entity, "WanderTarget", shift),
        "minecraft:shulker_bullet" => {
            add_xyz_in(entity, "Owner", shift);
            add_xyz_in(entity, "Target", shift);
        }
        "minecraft:end_crystal" => add_xyz_in(entity, "BeamTarget", shift),
        "minecraft:item_frame"
        | "minecraft:glow_item_frame"
        | "minecraft:painting"
        | "minecraft:leash_knot" => {
            add_xyz(entity, "TileX", "TileY", "TileZ", shift);
        }
        "minecraft:villager" => {
            if let Some(memories) = memories(entity) {
                for key in [
                    "minecraft:meeting_point",
                    "minecraft:home",
                    "minecraft:job_site",
                    "minecraft:potential_job_site",
                ] {
                    if let Some(memory) = memories.compound_mut(key) {
                        shift_memory(memory, shift);
                    }
                }
            }
        }
        "minecraft:pillager"
        | "minecraft:witch"
        | "minecraft:vindicator"
        | "minecraft:ravager"
        | "minecraft:illusioner"
        | "minecraft:evoker" => add_xyz_in(entity, "PatrolTarget", shift),
        "minecraft:falling_block" => {
            if let Some(tile) = entity.compound_mut("TileEntityData") {
                relocate_block_entity(tile, shift);
            }
        }
        "minecraft:sniffer" => {
            if let Some(explored) = memories(entity)
                .and_then(|memories| memories.compound_mut("minecraft:sniffer_explored_positions"))
            {
                for_compounds(explored, "value", |position| {
                    add_pos(position, "pos", shift)
                });
            }
        }
        "minecraft:bee" => {
            add_xyz_in(entity, "HivePos", shift);
            add_xyz_in(entity, "FlowerPos", shift);
        }
        _ => {}
    }
    for_compounds(entity, "Passengers", |passenger| {
        relocate_entity(passenger, shift)
    });
    if let Some(item) = entity.compound_mut("Item") {
        relocate_item(item, shift);
    }
    for key in ["Items", "HandItems", "ArmorItems"] {
        for_compounds(entity, key, |item| relocate_item(item, shift));
    }
    if let Some(equipment) = entity.compound_mut("equipment") {
        for (_, item) in equipment.iter_mut() {
            if let Some(item) = item.as_compound_mut() {
                relocate_item(item, shift);
            }
        }
    }
    randomize_uuid(entity);
}

fn memories(entity: &mut Compound) -> Option<&mut Compound> {
    entity.compound_mut("Brain")?.compound_mut("memories")
}

/// A villager memory: `pos` (int array or int list) directly or in its `value` wrapper.
fn shift_memory(memory: &mut Compound, shift: &Shift) {
    add_pos(memory, "pos", shift);
    if let Some(value) = memory.compound_mut("value") {
        add_pos(value, "pos", shift);
    }
}

/// Lodestone compasses and the items inside containers, pre-1.20.5 (`tag`) and with components.
fn relocate_item(item: &mut Compound, shift: &Shift) {
    let compass = item.str("id") == Some("minecraft:compass");
    if let Some(tag) = item.compound_mut("tag") {
        if compass {
            add_xyz_in(tag, "LodestonePos", shift);
        }
        if let Some(block_entity) = tag.compound_mut("BlockEntityTag") {
            for_compounds(block_entity, "Items", |inner| relocate_item(inner, shift));
        }
    }
    if let Some(components) = item.compound_mut("components") {
        if compass {
            if let Some(target) = components.compound_mut("minecraft:lodestone_target") {
                add_pos(target, "pos", shift);
            }
            if let Some(target) = components
                .compound_mut("minecraft:lodestone_tracker")
                .and_then(|tracker| tracker.compound_mut("target"))
            {
                add_pos(target, "pos", shift);
            }
        }
        for_compounds(components, "minecraft:container", |slot| {
            if let Some(inner) = slot.compound_mut("item") {
                relocate_item(inner, shift);
            }
        });
    }
}

/// `Helper.fixEntityUUID`: new random `UUIDMost`/`UUIDLeast` and `UUID` (4 ints) where present.
fn randomize_uuid(entity: &mut Compound) {
    let random = uuid::Uuid::new_v4().as_u128();
    for (key, bits) in [
        ("UUIDMost", (random >> 64) as i64),
        ("UUIDLeast", random as i64),
    ] {
        if entity.contains(key) {
            entity.insert(key, Tag::Long(bits));
        }
    }
    if let Some(uuid) = entity
        .get_mut("UUID")
        .and_then(Tag::as_ints_mut)
        .filter(|uuid| uuid.len() == 4)
    {
        for (index, value) in uuid.iter_mut().enumerate() {
            *value = (random >> (96 - 32 * index)) as i32;
        }
    }
}

/// Moves an `entities/` chunk (`RelocateEntities`): `Position [I; x, z]` (+ chunks) and every
/// entity of `Entities` (see [`relocate_region`] for the entity fields).
pub fn relocate_entities(root: &mut Compound, offset: Offset) -> Result<(), String> {
    supported_version(root, "entities")?;
    let shift = Shift::new(offset);
    let position = root
        .get_mut("Position")
        .and_then(Tag::as_ints_mut)
        .filter(|position| position.len() == 2)
        .ok_or("entities chunk without a Position")?;
    position[0] = position[0].wrapping_add(shift.cx);
    position[1] = position[1].wrapping_add(shift.cz);
    for_compounds(root, "Entities", |entity| relocate_entity(entity, &shift));
    Ok(())
}

/// Moves a `poi/` chunk (`RelocatePOI`): `Sections.<y>.Records[].pos` (+ blocks) and the section
/// keys (+ y sections). Sections leaving 0..15 are dropped before 1.18 (the world height then);
/// 1.18+ poi chunks do not say how high their world is, so their sections all stay. (MCA Selector
/// adds the y offset in blocks to the section keys and drops every negative key, losing 1.18+ poi
/// below y=0.) Entries that are not sections are dropped, as MCA Selector does.
pub fn relocate_poi(root: &mut Compound, offset: Offset) -> Result<(), String> {
    let version = supported_version(root, "poi")?;
    let shift = Shift::new(offset);
    let sections = root
        .compound_mut("Sections")
        .ok_or("poi chunk without Sections")?;
    let mut old = std::mem::take(sections);
    for (key, tag) in old.iter_mut() {
        let Some(y) = section_key(key) else {
            continue;
        };
        let Tag::Compound(mut section) = std::mem::replace(tag, Tag::Byte(0)) else {
            continue;
        };
        for_compounds(&mut section, "Records", |record| {
            add_pos(record, "pos", &shift)
        });
        let moved = i64::from(y) + i64::from(shift.sy);
        if version < V21W43A && !(0..=15).contains(&moved) {
            continue;
        }
        sections.insert(moved.to_string(), Tag::Compound(section));
    }
    Ok(())
}

/// A section key as MCA Selector accepts them (`^-?[0-9]{1,2}$`).
fn section_key(key: &str) -> Option<i32> {
    let digits = key.strip_prefix('-').unwrap_or(key);
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse().ok()
}

/// Imports `source` into `destination` (both already relocated), only sections in `ranges`, the
/// rest of destination kept (MCA Selector merge semantics for region/ chunks).
///
/// `ranges` select source sections (before the move), `y_offset` is the import's y offset in
/// sections, as MCA Selector's `mergeChunks(source, destination, ranges, yOffset)`: a destination
/// element at section y is replaced when `y - y_offset` is selected, a source element at y is taken
/// when `y - y_offset` is selected. Lists merged by the y of each element: sections (`Y`), block
/// entities and ticks (`y >> 4`), entities (`Pos[1] >> 4`); per-section lists (`Lights`,
/// `LiquidsToBeTicked`, `ToBeTicked`, `PostProcessing`) entry by entry; 1.15-1.16 biomes section by
/// section; structure starts: destination children (and childless starts, which become
/// `INVALID`) whose bounding box lies in one selected range go, source children reaching a
/// selected section come (with their start). References stay the destination's, as in MCA
/// Selector. 1.13-1.16 entities get new UUIDs. `Heightmaps` are dropped so the game rebuilds them.
pub fn merge_region(
    source: &Compound,
    destination: &mut Compound,
    ranges: &Ranges,
    y_offset: i32,
) -> Result<(), String> {
    let version = supported_version(source, "region")?;
    if version >= V21W43A {
        merge_root(source, destination, ranges, y_offset);
    } else {
        merge_level(source, destination, ranges, y_offset, version);
    }
    Ok(())
}

fn merge_level(
    source: &Compound,
    destination: &mut Compound,
    ranges: &Ranges,
    y_offset: i32,
    version: i32,
) {
    let empty = Compound::new();
    let source = source.compound("Level").unwrap_or(&empty);
    let level = child_compound(destination, "Level");
    merge_list(
        source.list("Sections"),
        level,
        "Sections",
        ranges,
        y_offset,
        section_y,
    );
    merge_list(
        source.list("Entities"),
        level,
        "Entities",
        ranges,
        y_offset,
        entity_y,
    );
    for key in ["TileEntities", "TileTicks", "LiquidTicks"] {
        merge_list(source.list(key), level, key, ranges, y_offset, block_y);
    }
    for key in [
        "Lights",
        "LiquidsToBeTicked",
        "ToBeTicked",
        "PostProcessing",
    ] {
        merge_index_lists(source.list(key), level, key, ranges, y_offset, 0);
    }
    merge_structures(
        source.compound("Structures"),
        level,
        "Structures",
        "Starts",
        ranges,
        y_offset,
    );
    if version < V20W45A {
        for_compounds(level, "Entities", randomize_uuids);
    }
    if (V19W36A..V20W45A).contains(&version) {
        merge_biomes(source, level, ranges, y_offset);
    }
    level.remove("Heightmaps");
}

fn merge_root(source: &Compound, destination: &mut Compound, ranges: &Ranges, y_offset: i32) {
    let had_sections = destination
        .list("sections")
        .is_some_and(|sections| !sections.is_empty());
    merge_list(
        source.list("sections"),
        destination,
        "sections",
        ranges,
        y_offset,
        section_y,
    );
    for key in ["block_entities", "block_ticks", "fluid_ticks"] {
        merge_list(
            source.list(key),
            destination,
            key,
            ranges,
            y_offset,
            block_y,
        );
    }
    // proto-chunk `entities` are not merged (MCA Selector 2.9 leaves them out too)
    // a new destination (empty_region) assumes yPos -4: take the source's world bottom instead
    if !had_sections {
        if let Some(y_pos) = source.get("yPos") {
            destination.insert("yPos", y_pos.clone());
        }
    }
    let bottom = destination.get("yPos").and_then(java_int).unwrap_or(-4);
    merge_index_lists(
        source.list("PostProcessing"),
        destination,
        "PostProcessing",
        ranges,
        y_offset,
        bottom,
    );
    merge_structures(
        source.compound("structures"),
        destination,
        "structures",
        "starts",
        ranges,
        y_offset,
    );
    destination.remove("Heightmaps");
}

fn section_y(tag: &Tag) -> i32 {
    tag.as_compound()
        .and_then(|section| section.get("Y"))
        .and_then(java_int)
        .unwrap_or(0)
}

fn block_y(tag: &Tag) -> i32 {
    tag.as_compound()
        .and_then(|element| element.get("y"))
        .and_then(java_int)
        .unwrap_or(0)
        >> 4
}

fn entity_y(tag: &Tag) -> i32 {
    tag.as_compound()
        .and_then(|entity| entity.list("Pos"))
        .and_then(|pos| pos.get(1))
        .and_then(java_int)
        .unwrap_or(0)
        >> 4
}

/// `mergeLists`: destination elements outside the selection, then source elements inside it.
/// (MCA Selector adds a destination element once per range not holding it; here it is kept once.)
/// Nothing is written when neither chunk has the list.
fn merge_list(
    source: Option<&[Tag]>,
    destination: &mut Compound,
    key: &str,
    ranges: &Ranges,
    y_offset: i32,
    y_of: fn(&Tag) -> i32,
) {
    if source.is_none() && !destination.contains(key) {
        return;
    }
    let existing = destination
        .list_mut(key)
        .map(std::mem::take)
        .unwrap_or_default();
    let mut merged: Vec<Tag> = existing
        .into_iter()
        .filter(|tag| !ranges.selects(y_of(tag), y_offset))
        .collect();
    merged.extend(
        source
            .unwrap_or_default()
            .iter()
            .filter(|tag| ranges.selects(y_of(tag), y_offset))
            .cloned(),
    );
    destination.insert(key, Tag::List(merged));
}

/// `mergeListTagLists`: entry `i` (section `bottom + i`) taken from the source when selected;
/// lists of different lengths, or a destination without the list, are left alone (MCA Selector
/// 2.9 never merges the 1.18+ `PostProcessing` and builds new chunks without it).
fn merge_index_lists(
    source: Option<&[Tag]>,
    destination: &mut Compound,
    key: &str,
    ranges: &Ranges,
    y_offset: i32,
    bottom: i32,
) {
    let Some(source) = source else {
        return;
    };
    let Some(list) = destination.list_mut(key) else {
        return;
    };
    if list.len() != source.len() {
        return;
    }
    for (index, (entry, from)) in list.iter_mut().zip(source).enumerate() {
        let y = (i64::from(bottom) + index as i64).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
            as i32;
        if ranges.selects(y, y_offset) {
            *entry = from.clone();
        }
    }
}

/// `mergeStructures` (see [`merge_region`]). MCA Selector's 1.18+ version shifts the bounding box
/// by `4 - yOffset` bits (operator precedence) and loses everything when the destination has no
/// starts compound; this compares `(y >> 4) - y_offset` and creates the compound.
fn merge_structures(
    source: Option<&Compound>,
    destination: &mut Compound,
    container_key: &str,
    starts_key: &str,
    ranges: &Ranges,
    y_offset: i32,
) {
    let in_one_range = |tag: Option<&Tag>| {
        tag.and_then(Tag::as_ints)
            .filter(|bb| bb.len() == 6)
            .is_some_and(|bb| ranges.selects_both(bb[1] >> 4, bb[4] >> 4, y_offset))
    };
    if let Some(starts) = destination
        .compound_mut(container_key)
        .and_then(|container| container.compound_mut(starts_key))
    {
        for (_, tag) in starts.iter_mut() {
            let invalidate = {
                let Some(start) = tag.as_compound_mut() else {
                    continue;
                };
                let childless = match start.list_mut("Children") {
                    Some(children) => {
                        children.retain(|child| {
                            !in_one_range(child.as_compound().and_then(|child| child.get("BB")))
                        });
                        children.is_empty()
                    }
                    None => true,
                };
                childless && in_one_range(start.get("BB"))
            };
            if invalidate {
                *tag = Tag::Compound(compound([("id", Tag::String("INVALID".into()))]));
            }
        }
    }
    let Some(source_starts) = source.and_then(|container| container.compound(starts_key)) else {
        return;
    };
    for (key, tag) in source_starts.iter() {
        let Some(children) = tag.as_compound().and_then(|start| start.list("Children")) else {
            continue;
        };
        for child in children {
            let Some(bb) = child
                .as_compound()
                .and_then(|child| child.get("BB"))
                .and_then(Tag::as_ints)
                .filter(|bb| bb.len() == 6)
            else {
                continue;
            };
            if !ranges.selects(bb[1] >> 4, y_offset) && !ranges.selects(bb[4] >> 4, y_offset) {
                continue;
            }
            let starts = child_compound(child_compound(destination, container_key), starts_key);
            let usable = starts
                .compound(key)
                .is_some_and(|start| start.str("id") != Some("INVALID"));
            if !usable {
                let mut copy = tag.as_compound().cloned().unwrap_or_default();
                if let Some(copied) = copy.list_mut("Children") {
                    copied.clear();
                }
                starts.insert(key, Tag::Compound(copy));
            }
            let start = child_compound(starts, key);
            if start.list_mut("Children").is_none() {
                start.insert("Children", Tag::List(Vec::new()));
            }
            if let Some(list) = start.list_mut("Children") {
                list.push(child.clone());
            }
        }
    }
}

/// 1.15-1.16 biomes (1024 ints, 64 per section): selected destination sections take the source's
/// biomes, or -1 ("let the game decide") when the source has none. A destination without biomes
/// gets an array of -1 (MCA Selector builds it but never stores it).
fn merge_biomes(source: &Compound, level: &mut Compound, ranges: &Ranges, y_offset: i32) {
    let source = source
        .get("Biomes")
        .and_then(Tag::as_ints)
        .filter(|biomes| biomes.len() == 1024);
    if !level.contains("Biomes") {
        level.insert("Biomes", Tag::IntArray(vec![-1; 1024]));
    }
    let Some(biomes) = level
        .get_mut("Biomes")
        .and_then(Tag::as_ints_mut)
        .filter(|biomes| biomes.len() == 1024)
    else {
        return;
    };
    for section in 0..16 {
        if !ranges.selects(section as i32, y_offset) {
            continue;
        }
        let cells = section * 64..section * 64 + 64;
        match source {
            Some(source) => biomes[cells.clone()].copy_from_slice(&source[cells]),
            None => biomes[cells].fill(-1),
        }
    }
}

/// `fixEntityUUIDs`: new UUIDs for an entity and its passengers.
fn randomize_uuids(entity: &mut Compound) {
    randomize_uuid(entity);
    for_compounds(entity, "Passengers", randomize_uuids);
}

/// Merges `entities/` chunks (`MergeEntities`): `Entities` by `Pos[1] >> 4` (see [`merge_region`]).
pub fn merge_entities(
    source: &Compound,
    destination: &mut Compound,
    ranges: &Ranges,
    y_offset: i32,
) -> Result<(), String> {
    supported_version(source, "entities")?;
    merge_list(
        source.list("Entities"),
        destination,
        "Entities",
        ranges,
        y_offset,
        entity_y,
    );
    Ok(())
}

/// Merges `poi/` chunks (`MergePOI`): selected source sections replace the destination's. Selected
/// destination sections the source does not have are removed too (MCA Selector keeps them, leaving
/// points of interest for blocks that were replaced). A destination without `Sections` gets them.
pub fn merge_poi(
    source: &Compound,
    destination: &mut Compound,
    ranges: &Ranges,
    y_offset: i32,
) -> Result<(), String> {
    supported_version(source, "poi")?;
    let empty = Compound::new();
    let source = source.compound("Sections").unwrap_or(&empty);
    let sections = child_compound(destination, "Sections");
    let stale: Vec<String> = sections
        .iter()
        .filter(|(key, _)| {
            section_key(key).is_some_and(|y| ranges.selects(y, y_offset)) && !source.contains(key)
        })
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in stale {
        sections.remove(&key);
    }
    for (key, tag) in source.iter() {
        if section_key(key).is_some_and(|y| ranges.selects(y, y_offset)) {
            sections.insert(key, tag.clone());
        }
    }
    Ok(())
}

/// A new region chunk for a merge into a missing chunk (MCA Selector `newEmptyChunk`).
pub fn empty_region(chunk_x: i32, chunk_z: i32, data_version: i32) -> Compound {
    if data_version >= V21W43A {
        let status = if data_version >= V1_20_PRE5 {
            "minecraft:full"
        } else {
            "full"
        };
        return compound([
            ("xPos", Tag::Int(chunk_x)),
            ("yPos", Tag::Int(-4)),
            ("zPos", Tag::Int(chunk_z)),
            ("Status", Tag::String(status.into())),
            ("DataVersion", Tag::Int(data_version)),
        ]);
    }
    let status = if data_version >= V19W02A {
        "full"
    } else {
        "postprocessed"
    };
    let level = compound([
        ("xPos", Tag::Int(chunk_x)),
        ("zPos", Tag::Int(chunk_z)),
        ("Status", Tag::String(status.into())),
    ]);
    compound([
        ("Level", Tag::Compound(level)),
        ("DataVersion", Tag::Int(data_version)),
    ])
}

/// A new `entities/` chunk for a merge into a missing chunk.
pub fn empty_entities(chunk_x: i32, chunk_z: i32, data_version: i32) -> Compound {
    compound([
        ("Position", Tag::IntArray(vec![chunk_x, chunk_z])),
        ("DataVersion", Tag::Int(data_version)),
        ("Entities", Tag::List(Vec::new())),
    ])
}

/// A new `poi/` chunk for a merge into a missing chunk (poi chunks hold no position).
pub fn empty_poi(_chunk_x: i32, _chunk_z: i32, data_version: i32) -> Compound {
    compound([
        ("Sections", Tag::Compound(Compound::new())),
        ("DataVersion", Tag::Int(data_version)),
    ])
}

/// A compound of `entries` in order.
fn compound<const N: usize>(entries: [(&str, Tag); N]) -> Compound {
    entries
        .into_iter()
        .map(|(key, tag)| (key.to_owned(), tag))
        .collect()
}

/// The compound at `key`, created (or replacing a non-compound) when missing.
fn child_compound<'a>(parent: &'a mut Compound, key: &str) -> &'a mut Compound {
    if parent.compound_mut(key).is_none() {
        parent.insert(key, Tag::Compound(Compound::new()));
    }
    parent
        .compound_mut(key)
        .expect("the compound was just inserted")
}

/// Calls `f` on every compound of the list at `key`.
fn for_compounds(parent: &mut Compound, key: &str, mut f: impl FnMut(&mut Compound)) {
    if let Some(list) = parent.list_mut(key) {
        for compound in list.iter_mut().filter_map(Tag::as_compound_mut) {
            f(compound);
        }
    }
}

/// Java's `NumberTag.asInt()`: integers truncated, floating point saturated.
fn java_int(tag: &Tag) -> Option<i32> {
    match *tag {
        Tag::Byte(value) => Some(value.into()),
        Tag::Short(value) => Some(value.into()),
        Tag::Int(value) => Some(value),
        Tag::Long(value) => Some(value as i32),
        Tag::Float(value) => Some(value as i32),
        Tag::Double(value) => Some(value as i32),
        _ => None,
    }
}

/// `Helper.applyIntIfPresent`: a number becomes the int `value + delta`.
fn add_int(compound: &mut Compound, key: &str, delta: i32) {
    if let Some(value) = compound.get(key).and_then(java_int) {
        compound.insert(key, Tag::Int(value.wrapping_add(delta)));
    }
}

/// `Helper.applyIntOffsetIfRootPresent` (+ blocks).
fn add_xyz(compound: &mut Compound, x: &str, y: &str, z: &str, shift: &Shift) {
    add_int(compound, x, shift.bx);
    add_int(compound, y, shift.by);
    add_int(compound, z, shift.bz);
}

/// `X`/`Y`/`Z` of the compound at `key`.
fn add_xyz_in(compound: &mut Compound, key: &str, shift: &Shift) {
    if let Some(inner) = compound.compound_mut(key) {
        add_xyz(inner, "X", "Y", "Z", shift);
    }
}

/// A block position `[I; x, y, z]` (or a list of 3 numbers, written back as ints) at `key`.
fn add_pos(compound: &mut Compound, key: &str, shift: &Shift) {
    let deltas = [shift.bx, shift.by, shift.bz];
    match compound.get_mut(key) {
        Some(Tag::IntArray(pos)) if pos.len() == 3 => {
            for (value, delta) in pos.iter_mut().zip(deltas) {
                *value = value.wrapping_add(delta);
            }
        }
        Some(Tag::List(pos)) if pos.len() == 3 && pos.iter().all(|tag| java_int(tag).is_some()) => {
            for (value, delta) in pos.iter_mut().zip(deltas) {
                *value = Tag::Int(java_int(value).unwrap_or(0).wrapping_add(delta));
            }
        }
        _ => {}
    }
}

/// An entity position: a list of 3 numbers, written back as doubles.
fn add_double_pos(compound: &mut Compound, key: &str, shift: &Shift) {
    let deltas = [shift.bx, shift.by, shift.bz];
    if let Some(pos) = compound.list_mut(key).filter(|pos| pos.len() == 3) {
        for (value, delta) in pos.iter_mut().zip(deltas) {
            if let Some(number) = value.as_f64() {
                *value = Tag::Double(number + f64::from(delta));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil;

    const REGION: &[u8] = include_bytes!("../tests/fixtures/relocate_region.mca");
    const ENTITIES: &[u8] = include_bytes!("../tests/fixtures/relocate_entities.mca");
    const POI: &[u8] = include_bytes!("../tests/fixtures/relocate_poi.mca");
    // real MCA Selector 2.9 imports (tests/fixtures/mcaselector/README.md)
    const MCA_SOURCE_REGION: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/change/before/region/r.-1.0.mca");
    const MCA_SOURCE_ENTITIES: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/change/before/entities/r.-1.0.mca");
    const MCA_OFFSET_R00: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/import/offset-33_-2/region/r.0.0.mca");
    const MCA_OFFSET_R0M1: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/import/offset-33_-2/region/r.0.-1.mca");
    const MCA_OFFSET_E00: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/import/offset-33_-2/entities/r.0.0.mca");
    const MCA_SECTIONS_REGION: &[u8] = include_bytes!(
        "../tests/fixtures/mcaselector/import/offset--40_5-y2-sections-0_3/region/r.-2.0.mca"
    );
    const MCA_SECTIONS_ENTITIES: &[u8] = include_bytes!(
        "../tests/fixtures/mcaselector/import/offset--40_5-y2-sections-0_3/entities/r.-2.0.mca"
    );

    const XZ: Offset = Offset { x: 33, y: 0, z: -2 };
    const XZ_BACK: Offset = Offset { x: -33, y: 0, z: 2 };
    const UP: Offset = Offset { x: 0, y: 1, z: 0 };
    const DOWN: Offset = Offset { x: 0, y: -1, z: 0 };

    fn chunk(data: &[u8], index: usize) -> Compound {
        let slot = anvil::slots(data)
            .unwrap()
            .into_iter()
            .find(|slot| slot.index == index)
            .unwrap_or_else(|| panic!("no chunk in slot {index}"));
        anvil::chunk(data, slot).unwrap().unwrap()
    }

    /// The tag at a `/` separated path of compound keys and list indices.
    fn at<'a>(root: &'a Compound, path: &str) -> &'a Tag {
        let mut segments = path.split('/');
        let first = segments.next().unwrap();
        let mut tag = root
            .get(first)
            .unwrap_or_else(|| panic!("{path}: no {first}"));
        for segment in segments {
            tag = match tag {
                Tag::Compound(compound) => compound.get(segment),
                Tag::List(items) => segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| items.get(index)),
                _ => None,
            }
            .unwrap_or_else(|| panic!("{path}: no {segment}"));
        }
        tag
    }

    fn int(root: &Compound, path: &str) -> i64 {
        at(root, path)
            .as_i64()
            .unwrap_or_else(|| panic!("{path} is not an integer"))
    }

    fn ints(root: &Compound, path: &str) -> Vec<i32> {
        at(root, path)
            .as_ints()
            .unwrap_or_else(|| panic!("{path} is not an int array"))
            .to_vec()
    }

    fn doubles(root: &Compound, path: &str) -> Vec<f64> {
        at(root, path)
            .as_list()
            .unwrap()
            .iter()
            .map(|tag| tag.as_f64().unwrap())
            .collect()
    }

    fn list<'a>(root: &'a Compound, path: &str) -> &'a [Tag] {
        at(root, path)
            .as_list()
            .unwrap_or_else(|| panic!("{path} is not a list"))
    }

    fn string<'a>(root: &'a Compound, path: &str) -> &'a str {
        at(root, path)
            .as_str()
            .unwrap_or_else(|| panic!("{path} is not a string"))
    }

    fn ids(root: &Compound, path: &str) -> Vec<String> {
        list(root, path)
            .iter()
            .map(|tag| tag.as_compound().unwrap().str("id").unwrap().to_owned())
            .collect()
    }

    fn ys(sections: &[Tag]) -> Vec<i32> {
        sections.iter().map(section_y).collect()
    }

    fn pack(x: i32, z: i32) -> i64 {
        (i64::from(z as u32) << 32) | i64::from(x as u32)
    }

    /// Compounds sorted by key and entity UUIDs left out, for comparisons that ignore key order
    /// (MCA Selector writes hash order) and the random UUIDs.
    fn canonical(tag: &Tag) -> Tag {
        match tag {
            Tag::Compound(compound) => {
                let mut entries: Vec<(String, Tag)> = compound
                    .iter()
                    .filter(|(key, _)| !matches!(*key, "UUID" | "UUIDMost" | "UUIDLeast"))
                    .map(|(key, value)| (key.to_owned(), canonical(value)))
                    .collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Tag::Compound(entries.into_iter().collect())
            }
            Tag::List(items) => Tag::List(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }

    fn same(a: &Compound, b: &Compound) -> bool {
        canonical(&Tag::Compound(a.clone())) == canonical(&Tag::Compound(b.clone()))
    }

    fn without_sections(root: &Compound, key: &str, drop: &[i32]) -> Compound {
        let mut root = root.clone();
        root.list_mut(key)
            .unwrap()
            .retain(|section| !drop.contains(&section_y(section)));
        root
    }

    #[test]
    fn parses_ranges() {
        assert!(parse_ranges("").unwrap().is_all());
        assert!(parse_ranges("   ").unwrap().is_all());
        let ranges = parse_ranges("-4:3,10").unwrap();
        assert!(!ranges.is_all());
        assert!(!ranges.contains(-5));
        assert!(ranges.contains(-4) && ranges.contains(0) && ranges.contains(3));
        assert!(!ranges.contains(4) && !ranges.contains(9) && !ranges.contains(11));
        assert!(ranges.contains(10));
        let below = parse_ranges(":0").unwrap();
        assert!(below.contains(i32::MIN) && below.contains(0) && !below.contains(1));
        let above = parse_ranges("5:").unwrap();
        assert!(!above.contains(4) && above.contains(5) && above.contains(i32::MAX));
        let every = parse_ranges(":").unwrap();
        assert!(!every.is_all() && every.contains(i32::MIN) && every.contains(i32::MAX));
        assert!(parse_ranges("true").unwrap().contains(-100));
        assert_eq!(
            parse_ranges(" 1 , 2:3 ,").unwrap(),
            parse_ranges("1,2:3").unwrap()
        );
        // without a colon only the first number counts, as in MCA Selector's regex
        let odd = parse_ranges("5-3").unwrap();
        assert!(odd.contains(5) && !odd.contains(4) && !odd.contains(-3));
        let empty = parse_ranges("5:3").unwrap();
        assert!(!empty.contains(4) && !empty.contains(5));
        for bad in [
            "a",
            "1,,2",
            ",1",
            "-",
            "1:2:3",
            "1;2",
            "99999999999",
            "--1",
            "1:-",
        ] {
            assert!(parse_ranges(bad).is_err(), "{bad} should not parse");
        }
        assert_eq!(
            parse_ranges("1,x").unwrap_err(),
            "invalid section range \"x\""
        );
    }

    #[test]
    fn relocates_1_21_chunk_horizontally() {
        let original = chunk(REGION, 0);
        let mut moved = original.clone();
        relocate_region(&mut moved, XZ).unwrap();
        assert_eq!((int(&original, "xPos"), int(&original, "zPos")), (-6, 6));
        assert_eq!((int(&moved, "xPos"), int(&moved, "zPos")), (27, 4));
        assert_eq!(int(&moved, "yPos"), -4);
        for (path, before, after) in [
            ("block_ticks/0", [-93, -60, 100], [435, -60, 68]),
            ("fluid_ticks/0", [-81, 3, 107], [447, 3, 75]),
            ("block_entities/0", [-88, 6, 111], [440, 6, 79]),
            ("block_entities/3", [-88, -50, 104], [440, -50, 72]),
        ] {
            for (index, axis) in ["x", "y", "z"].into_iter().enumerate() {
                assert_eq!(
                    int(&original, &format!("{path}/{axis}")),
                    i64::from(before[index])
                );
                assert_eq!(
                    int(&moved, &format!("{path}/{axis}")),
                    i64::from(after[index])
                );
            }
        }
        assert_eq!(
            ints(&original, "block_entities/2/exit_portal"),
            [100, 75, -200]
        );
        assert_eq!(
            ints(&moved, "block_entities/2/exit_portal"),
            [628, 75, -232]
        );
        assert_eq!(int(&moved, "block_entities/2/ExitPortal/X"), 529);
        assert_eq!(int(&moved, "block_entities/2/ExitPortal/Y"), 2);
        assert_eq!(int(&moved, "block_entities/2/ExitPortal/Z"), -29);
        assert_ne!(
            ints(
                &moved,
                "block_entities/3/SpawnPotentials/0/data/entity/UUID"
            ),
            [1, 2, 3, 4]
        );
        assert_eq!(ints(&moved, "block_entities/4/flower_pos"), [439, 69, 73]);
        assert_eq!(
            ints(&original, "block_entities/4/bees/0/entity_data/hive_pos"),
            [-91, 70, 101]
        );
        assert_eq!(
            ints(&moved, "block_entities/4/bees/0/entity_data/hive_pos"),
            [437, 70, 69]
        );
        assert_ne!(
            ints(&moved, "block_entities/4/bees/0/entity_data/UUID"),
            [5, 6, 7, 8]
        );

        let references = at(&moved, "structures/References/minecraft:mineshaft")
            .as_longs()
            .unwrap();
        assert_eq!(
            at(&original, "structures/References/minecraft:mineshaft")
                .as_longs()
                .unwrap(),
            [pack(-7, 8), pack(-5, -7)]
        );
        assert_eq!(references, [pack(26, 6), pack(28, -9)]);
        let village = "structures/starts/minecraft:village_plains";
        assert_eq!(int(&moved, &format!("{village}/ChunkX")), 27);
        assert_eq!(int(&moved, &format!("{village}/ChunkZ")), 4);
        assert_eq!(
            ints(&original, &format!("{village}/Children/0/BB")),
            [-96, 63, 96, -87, 70, 105]
        );
        assert_eq!(
            ints(&moved, &format!("{village}/Children/0/BB")),
            [432, 63, 64, 441, 70, 73]
        );
        assert_eq!(int(&moved, &format!("{village}/Children/0/PosX")), 432);
        assert_eq!(int(&moved, &format!("{village}/Children/0/PosY")), 63);
        assert_eq!(int(&moved, &format!("{village}/Children/0/PosZ")), 64);
        assert_eq!(
            int(
                &moved,
                &format!("{village}/Children/0/junctions/0/source_x")
            ),
            436
        );
        assert_eq!(
            int(
                &moved,
                &format!("{village}/Children/0/junctions/0/source_y")
            ),
            64
        );
        assert_eq!(
            int(
                &moved,
                &format!("{village}/Children/0/junctions/0/source_z")
            ),
            64
        );
        assert_eq!(int(&moved, &format!("{village}/Children/1/TPX")), 434);
        assert_eq!(int(&moved, &format!("{village}/Children/1/TPY")), -40);
        assert_eq!(int(&moved, &format!("{village}/Children/1/TPZ")), 66);
        assert_eq!(
            ints(&moved, &format!("{village}/Children/1/Entrances/0")),
            [434, -40, 66, 436, -38, 66]
        );
        assert_eq!(
            at(&moved, "structures/starts/minecraft:stronghold"),
            at(&original, "structures/starts/minecraft:stronghold")
        );

        // sections keep their data; the light-only section below yPos goes, as in MCA Selector
        assert_eq!(ys(list(&original, "sections"))[0], -5);
        assert_eq!(
            list(&moved, "sections"),
            list(&without_sections(&original, "sections", &[-5]), "sections")
        );
        assert_eq!(at(&moved, "Heightmaps"), at(&original, "Heightmaps"));
        assert_eq!(
            at(&moved, "PostProcessing"),
            at(&original, "PostProcessing")
        );

        // the inverse offset restores everything else (entity UUIDs are new)
        let mut back = moved.clone();
        relocate_region(&mut back, XZ_BACK).unwrap();
        assert!(same(&back, &without_sections(&original, "sections", &[-5])));
        assert_eq!(
            at(&back, "structures/References/minecraft:mineshaft"),
            at(&original, "structures/References/minecraft:mineshaft")
        );
    }

    #[test]
    fn relocates_1_21_chunk_vertically() {
        let original = chunk(REGION, 0);
        let mut up = original.clone();
        relocate_region(&mut up, UP).unwrap();
        assert_eq!(
            (int(&up, "xPos"), int(&up, "zPos"), int(&up, "yPos")),
            (-6, 6, -4)
        );
        let sections = list(&up, "sections");
        assert_eq!(ys(sections), (-3..=19).collect::<Vec<_>>());
        for section in sections {
            let y = section_y(section);
            let mut expected = list(&original, "sections")
                .iter()
                .find(|old| section_y(old) == y - 1)
                .unwrap()
                .as_compound()
                .unwrap()
                .clone();
            expected.insert("Y", Tag::Byte(y as i8));
            assert_eq!(section.as_compound().unwrap(), &expected);
        }
        let old_post = list(&original, "PostProcessing");
        let new_post = list(&up, "PostProcessing");
        assert_eq!(new_post.len(), 24);
        assert_eq!(new_post[0], Tag::List(Vec::new()));
        assert_eq!(&new_post[1..], &old_post[..23]);
        assert_eq!(int(&up, "block_entities/0/y"), 22);
        assert_eq!(int(&up, "block_entities/0/x"), -88);
        assert_eq!(int(&up, "block_ticks/0/y"), -44);
        assert_eq!(int(&up, "fluid_ticks/0/y"), 19);
        assert_eq!(ints(&up, "block_entities/2/exit_portal"), [100, 91, -200]);
        let village = "structures/starts/minecraft:village_plains";
        assert_eq!(
            ints(&up, &format!("{village}/Children/0/BB")),
            [-96, 79, 96, -87, 86, 105]
        );
        assert_eq!(
            int(&up, &format!("{village}/Children/0/junctions/0/source_y")),
            80
        );
        assert_eq!(
            at(&up, "structures/References"),
            at(&original, "structures/References")
        );
        assert!(up.get("Heightmaps").is_none());

        // back down: what left the chunk (the top section) stays lost, the rest is restored
        let mut back = up.clone();
        relocate_region(&mut back, DOWN).unwrap();
        let mut expected = without_sections(&original, "sections", &[-5, 19]);
        expected.remove("Heightmaps");
        expected.list_mut("PostProcessing").unwrap()[23] = Tag::List(Vec::new());
        assert!(same(&back, &expected));

        let mut down = original.clone();
        relocate_region(&mut down, DOWN).unwrap();
        assert_eq!(ys(list(&down, "sections")), (-4..=18).collect::<Vec<_>>());
        assert_eq!(
            at(&down, "sections/0/block_states"),
            at(
                &without_sections(&original, "sections", &[-5, -4]),
                "sections/0/block_states"
            )
        );
        assert_eq!(int(&down, "block_ticks/0/y"), -76);
    }

    #[test]
    fn relocates_level_chunk() {
        let original = chunk(REGION, 2);
        let mut moved = original.clone();
        relocate_region(&mut moved, XZ).unwrap();
        assert_eq!(
            (int(&moved, "Level/xPos"), int(&moved, "Level/zPos")),
            (43, -5)
        );
        assert_eq!(
            doubles(&original, "Level/Entities/0/Pos"),
            [162.5, 65.0, -44.5]
        );
        assert_eq!(
            doubles(&moved, "Level/Entities/0/Pos"),
            [690.5, 65.0, -76.5]
        );
        assert_eq!(int(&moved, "Level/Entities/0/Leash/X"), 689);
        assert_eq!(int(&moved, "Level/Entities/0/Leash/Y"), 64);
        assert_eq!(int(&moved, "Level/Entities/0/Leash/Z"), -79);
        assert!(
            (
                int(&moved, "Level/Entities/0/UUIDMost"),
                int(&moved, "Level/Entities/0/UUIDLeast")
            ) != (11, 12)
        );
        assert_eq!(
            doubles(&moved, "Level/Entities/0/Passengers/0/Pos"),
            [690.5, 66.0, -76.5]
        );
        assert_ne!(
            ints(&moved, "Level/Entities/0/Passengers/0/UUID"),
            [9, 9, 9, 9]
        );
        assert_eq!(int(&moved, "Level/Entities/1/HomePosX"), 697);
        assert_eq!(int(&moved, "Level/Entities/1/HomePosY"), 29);
        assert_eq!(int(&moved, "Level/Entities/1/HomePosZ"), -71);
        assert_eq!(int(&moved, "Level/Entities/1/TravelPosX"), 698);
        assert_eq!(int(&moved, "Level/Entities/1/TravelPosZ"), -70);
        assert_eq!(int(&moved, "Level/TileEntities/0/x"), 692);
        assert_eq!(int(&moved, "Level/TileEntities/0/y"), 20);
        assert_eq!(int(&moved, "Level/TileEntities/0/z"), -75);
        assert_eq!(int(&moved, "Level/TileEntities/1/ExitPortal/X"), 1028);
        assert_eq!(int(&moved, "Level/TileEntities/1/ExitPortal/Z"), -532);
        assert_ne!(
            ints(&moved, "Level/TileEntities/2/SpawnPotentials/0/Entity/UUID"),
            [2, 2, 2, 2]
        );
        assert_eq!(
            (
                int(&moved, "Level/TileTicks/0/x"),
                int(&moved, "Level/TileTicks/0/z")
            ),
            (688, -80)
        );
        assert_eq!(
            (
                int(&moved, "Level/LiquidTicks/0/x"),
                int(&moved, "Level/LiquidTicks/0/z")
            ),
            (703, -65)
        );
        assert_eq!(
            at(&moved, "Level/Structures/References/Mineshaft")
                .as_longs()
                .unwrap(),
            [pack(43, -5), pack(44, -6)]
        );
        let start = "Level/Structures/Starts/Mineshaft";
        assert_eq!(
            (
                int(&moved, &format!("{start}/ChunkX")),
                int(&moved, &format!("{start}/ChunkZ"))
            ),
            (43, -5)
        );
        assert_eq!(
            ints(&moved, &format!("{start}/BB")),
            [688, 10, -80, 728, 50, -40]
        );
        assert_eq!(int(&moved, &format!("{start}/Processed/0/X")), 43);
        assert_eq!(int(&moved, &format!("{start}/Processed/0/Z")), -5);
        assert_eq!(
            ints(&moved, &format!("{start}/Children/0/Entrances/0")),
            [688, 10, -80, 690, 12, -80]
        );
        assert_eq!(int(&moved, &format!("{start}/Children/1/TPX")), 698);
        assert_eq!(int(&moved, &format!("{start}/Children/1/TPZ")), -80);
        assert_eq!(
            string(&moved, "Level/Structures/Starts/Village/id"),
            "INVALID"
        );
        for key in [
            "Sections",
            "Biomes",
            "Heightmaps",
            "Lights",
            "PostProcessing",
        ] {
            assert_eq!(
                at(&moved, &format!("Level/{key}")),
                at(&original, &format!("Level/{key}"))
            );
        }
        let mut back = moved.clone();
        relocate_region(&mut back, XZ_BACK).unwrap();
        assert!(same(&back, &original));
    }

    #[test]
    fn relocates_level_chunk_vertically() {
        let original = chunk(REGION, 2);
        let mut up = original.clone();
        relocate_region(&mut up, UP).unwrap();
        assert_eq!(ys(list(&up, "Level/Sections")), [1, 2, 3, 4, 5]);
        let biomes = ints(&up, "Level/Biomes");
        assert!(biomes[..64].iter().all(|&biome| biome == 0));
        assert_eq!(biomes[5 * 16], 1);
        assert_eq!(biomes[63 * 16], 1403);
        assert_eq!(list(&up, "Level/Lights/0").len(), 0);
        assert_eq!(list(&up, "Level/Lights/2").len(), 1);
        assert_eq!(list(&up, "Level/ToBeTicked/3").len(), 2);
        assert_eq!(int(&up, "Level/TileEntities/0/y"), 36);
        assert_eq!(doubles(&up, "Level/Entities/0/Pos")[1], 81.0);
        assert_eq!(ints(&up, "Level/Structures/Starts/Mineshaft/BB")[1], 26);
        assert!(up.compound("Level").unwrap().get("Heightmaps").is_none());

        let mut back = up.clone();
        relocate_region(&mut back, DOWN).unwrap();
        let mut expected = original.clone();
        let level = expected.compound_mut("Level").unwrap();
        level.remove("Heightmaps");
        level.get_mut("Biomes").and_then(Tag::as_ints_mut).unwrap()[960..].fill(0);
        assert!(same(&back, &expected));

        let mut down = original.clone();
        relocate_region(&mut down, DOWN).unwrap();
        assert_eq!(ys(list(&down, "Level/Sections")), [0, 1, 2, 3]);
        let biomes = ints(&down, "Level/Biomes");
        assert_eq!(biomes[0], 100);
        assert!(biomes[960..].iter().all(|&biome| biome == 0));
    }

    #[test]
    fn relocates_entities() {
        let original = chunk(ENTITIES, 0);
        let mut moved = original.clone();
        relocate_entities(&mut moved, XZ).unwrap();
        assert_eq!(ints(&original, "Position"), [-9, 7]);
        assert_eq!(ints(&moved, "Position"), [24, 5]);
        assert_eq!(doubles(&moved, "Entities/0/Pos"), [397.0, 64.0, 94.0]);
        assert_eq!(
            doubles(&moved, "Entities/0/Paper.Origin"),
            [-131.0, 64.0, 126.0]
        );
        assert_ne!(
            ints(&moved, "Entities/0/UUID"),
            ints(&original, "Entities/0/UUID")
        );
        assert_eq!(ints(&moved, "Entities/3/leash"), [386, 69, 82]);
        assert_eq!(ints(&moved, "Entities/3/home_pos"), [387, 68, 83]);
        assert_eq!(
            doubles(&moved, "Entities/3/Passengers/0/Pos"),
            [388.5, 71.0, 84.5]
        );
        assert_eq!(
            ints(&moved, "Entities/3/Passengers/0/sleeping_pos"),
            [529, 2, -29]
        );
        assert_ne!(
            ints(&moved, "Entities/3/Passengers/0/UUID"),
            [50, 60, 70, 80]
        );
        assert_eq!(int(&moved, "Entities/4/Leash/X"), 389);
        assert_eq!(int(&moved, "Entities/4/Leash/Z"), 86);
        assert_eq!(
            ints(&moved, "Entities/5/Brain/memories/minecraft:home/value/pos"),
            [390, 64, 87]
        );
        assert_eq!(
            ints(&moved, "Entities/5/Brain/memories/minecraft:job_site/pos"),
            [392, 64, 88]
        );
        assert_eq!(
            ints(
                &moved,
                "Entities/5/equipment/mainhand/components/minecraft:lodestone_tracker/target/pos"
            ),
            [385, 64, 81]
        );
        assert_eq!(ints(&moved, "Entities/6/beam_target"), [528, 128, -32]);
        assert_eq!(ints(&moved, "Entities/7/block_pos"), [393, 65, 80]);
        assert_eq!(
            (
                int(&moved, "Entities/7/TileX"),
                int(&moved, "Entities/7/TileZ")
            ),
            (393, 80)
        );
        assert_eq!(
            (
                int(&moved, "Entities/8/TreasurePosX"),
                int(&moved, "Entities/8/TreasurePosZ")
            ),
            (828, -332)
        );
        assert_eq!(
            ints(
                &moved,
                "Entities/9/Items/0/components/minecraft:container/0/item/components/minecraft:lodestone_tracker/target/pos"
            ),
            [535, 8, -23]
        );
        let mut back = moved.clone();
        relocate_entities(&mut back, XZ_BACK).unwrap();
        assert!(same(&back, &original));

        let mut up = original.clone();
        relocate_entities(&mut up, UP).unwrap();
        assert_eq!(ints(&up, "Position"), [-9, 7]);
        assert_eq!(doubles(&up, "Entities/0/Pos"), [-131.0, 80.0, 126.0]);
        assert_eq!(ints(&up, "Entities/3/leash"), [-142, 85, 114]);
        let mut back = up.clone();
        relocate_entities(&mut back, DOWN).unwrap();
        assert!(same(&back, &original));
    }

    #[test]
    fn relocates_poi() {
        let original = chunk(POI, 0);
        let mut moved = original.clone();
        relocate_poi(&mut moved, Offset { x: 33, y: 1, z: -2 }).unwrap();
        let keys: Vec<&str> = moved
            .compound("Sections")
            .unwrap()
            .iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(keys, ["-3", "4", "20"]);
        assert_eq!(ints(&original, "Sections/-4/Records/0/pos"), [67, -59, -28]);
        assert_eq!(ints(&moved, "Sections/-3/Records/0/pos"), [595, -43, -60]);
        assert_eq!(ints(&moved, "Sections/20/Records/0/pos"), [595, 325, -60]);
        let mut back = moved.clone();
        relocate_poi(
            &mut back,
            Offset {
                x: -33,
                y: -1,
                z: 2,
            },
        )
        .unwrap();
        assert_eq!(back, original);

        // before 1.18 the world is 16 sections high: sections moved out of it are dropped
        let record = |y: i32| {
            Tag::Compound(compound([(
                "Records",
                Tag::List(vec![Tag::Compound(compound([(
                    "pos",
                    Tag::IntArray(vec![0, y * 16, 0]),
                )]))]),
            )]))
        };
        let mut old = compound([
            (
                "Sections",
                Tag::Compound(compound([
                    ("15", record(15)),
                    ("3", record(3)),
                    ("x", record(0)),
                ])),
            ),
            ("DataVersion", Tag::Int(2586)),
        ]);
        relocate_poi(&mut old, UP).unwrap();
        let keys: Vec<&str> = old
            .compound("Sections")
            .unwrap()
            .iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(keys, ["4"]);
        assert_eq!(ints(&old, "Sections/4/Records/0/pos"), [0, 64, 0]);
    }

    #[test]
    fn refuses_unsupported_chunks() {
        let ancient = compound([(
            "Level",
            Tag::Compound(compound([("xPos", Tag::Int(0)), ("zPos", Tag::Int(0))])),
        )]);
        let error = relocate_region(&mut ancient.clone(), XZ).unwrap_err();
        assert!(error.contains("before Minecraft 1.13"), "{error}");
        let mut old = compound([
            ("DataVersion", Tag::Int(1343)),
            ("Level", Tag::Compound(Compound::new())),
        ]);
        assert!(relocate_region(&mut old, XZ).is_err());
        assert!(merge_region(&ancient, &mut Compound::new(), &Ranges::all(), 0).is_err());
        let mut no_level = compound([("DataVersion", Tag::Int(2586))]);
        assert!(relocate_region(&mut no_level, XZ).is_err());
        let mut no_position = compound([
            ("DataVersion", Tag::Int(4189)),
            ("Entities", Tag::List(Vec::new())),
        ]);
        assert!(relocate_entities(&mut no_position, XZ).is_err());
        let mut no_sections = compound([("DataVersion", Tag::Int(4189))]);
        assert!(relocate_poi(&mut no_sections, XZ).is_err());
    }

    #[test]
    fn merges_1_21_sections() {
        let source = chunk(REGION, 0);
        let destination = chunk(REGION, 1);
        let mut merged = destination.clone();
        merge_region(&source, &mut merged, &parse_ranges("-4:3").unwrap(), 0).unwrap();
        let sections = list(&merged, "sections");
        let mut expected_ys = vec![-5];
        expected_ys.extend(4..=19);
        expected_ys.extend(-4..=3);
        assert_eq!(ys(sections), expected_ys);
        for section in sections {
            let y = section_y(section);
            let from = if (-4..=3).contains(&y) {
                &source
            } else {
                &destination
            };
            let original = list(from, "sections")
                .iter()
                .find(|old| section_y(old) == y)
                .unwrap();
            assert_eq!(section, original, "section {y}");
        }
        assert_eq!(
            ids(&merged, "block_entities"),
            [
                "minecraft:mob_spawner",
                "minecraft:chest",
                "minecraft:spawner"
            ]
        );
        assert_eq!(int(&merged, "block_entities/1/z"), 108);
        assert_eq!(list(&merged, "block_ticks"), list(&source, "block_ticks"));
        assert_eq!(list(&merged, "fluid_ticks"), list(&source, "fluid_ticks"));
        let post = list(&merged, "PostProcessing");
        assert_eq!(&post[..8], &list(&source, "PostProcessing")[..8]);
        assert_eq!(&post[8..], &list(&destination, "PostProcessing")[8..]);
        assert!(merged.get("Heightmaps").is_none());
        assert_eq!(
            list(
                &merged,
                "structures/starts/minecraft:village_plains/Children"
            )
            .len(),
            2
        );
        assert!(
            merged
                .compound("structures")
                .unwrap()
                .compound("starts")
                .unwrap()
                .get("minecraft:stronghold")
                .is_none()
        );
        assert_eq!(
            at(&merged, "structures/References"),
            at(&destination, "structures/References")
        );
        for key in ["xPos", "zPos", "LastUpdate", "Status", "InhabitedTime"] {
            assert_eq!(at(&merged, key), at(&destination, key));
        }

        // ranges select source sections before the move: y_offset 1 takes source section 0 (now 1)
        let mut lifted = source.clone();
        relocate_region(&mut lifted, UP).unwrap();
        let mut merged = destination.clone();
        merge_region(&lifted, &mut merged, &parse_ranges("0").unwrap(), 1).unwrap();
        let sections = list(&merged, "sections");
        assert_eq!(sections.len(), 25);
        for section in sections {
            let y = section_y(section);
            let from = if y == 1 { &lifted } else { &destination };
            assert_eq!(
                section,
                list(from, "sections")
                    .iter()
                    .find(|old| section_y(old) == y)
                    .unwrap()
            );
        }
    }

    #[test]
    fn merges_level_sections() {
        let source = chunk(REGION, 2);
        let destination = chunk(REGION, 3);
        let mut merged = destination.clone();
        merge_region(&source, &mut merged, &parse_ranges("1:2").unwrap(), 0).unwrap();
        let sections = list(&merged, "Level/Sections");
        assert_eq!(ys(sections), [0, 3, 4, 1, 2]);
        for section in sections {
            let name = section.as_compound().unwrap().list("Palette").unwrap()[0]
                .as_compound()
                .unwrap()
                .str("Name")
                .unwrap();
            let expected = if (1..=2).contains(&section_y(section)) {
                "minecraft:stone"
            } else {
                "minecraft:dirt"
            };
            assert_eq!(name, expected);
        }
        let biomes = ints(&merged, "Level/Biomes");
        assert_eq!(biomes[0], 5000);
        assert_eq!(&biomes[64..192], &ints(&source, "Level/Biomes")[64..192]);
        assert_eq!(&biomes[192..], &ints(&destination, "Level/Biomes")[192..]);
        assert_eq!(
            ids(&merged, "Level/Entities"),
            ["minecraft:pig", "minecraft:turtle"]
        );
        assert!(
            (
                int(&merged, "Level/Entities/0/UUIDMost"),
                int(&merged, "Level/Entities/0/UUIDLeast")
            ) != (11, 12)
        );
        assert_eq!(
            ids(&merged, "Level/TileEntities"),
            [
                "minecraft:end_gateway",
                "minecraft:mob_spawner",
                "minecraft:chest"
            ]
        );
        assert_eq!(list(&merged, "Level/TileTicks").len(), 1);
        assert_eq!(list(&merged, "Level/LiquidTicks").len(), 1);
        assert_eq!(
            ids(&merged, "Level/Structures/Starts/Mineshaft/Children"),
            ["MSRoom", "MSCorridor"]
        );
        assert_eq!(
            string(&merged, "Level/Structures/Starts/Village/id"),
            "INVALID"
        );
        assert!(
            merged
                .compound("Level")
                .unwrap()
                .get("Heightmaps")
                .is_none()
        );

        // a start whose children all go is invalidated, then rebuilt from the source's children
        let mut merged = destination.clone();
        merge_region(&source, &mut merged, &parse_ranges("0:3").unwrap(), 0).unwrap();
        assert_eq!(
            string(&merged, "Level/Structures/Starts/Mineshaft/id"),
            "Mineshaft"
        );
        assert_eq!(
            ids(&merged, "Level/Structures/Starts/Mineshaft/Children"),
            ["MSRoom", "MSCorridor"]
        );
        assert_eq!(ys(list(&merged, "Level/Sections")), [4, 0, 1, 2, 3]);
    }

    #[test]
    fn merges_entities_and_poi() {
        let source = chunk(ENTITIES, 0);
        let mut merged = chunk(ENTITIES, 1);
        merge_entities(&source, &mut merged, &parse_ranges("4").unwrap(), 0).unwrap();
        assert_eq!(
            ids(&merged, "Entities"),
            [
                "minecraft:cow",
                "minecraft:bat",
                "minecraft:rabbit",
                "minecraft:rabbit",
                "minecraft:rabbit",
                "minecraft:horse",
                "minecraft:wolf",
                "minecraft:villager",
                "minecraft:item_frame"
            ]
        );
        let mut fresh = empty_entities(-9, 7, 4189);
        merge_entities(&source, &mut fresh, &parse_ranges("4").unwrap(), 0).unwrap();
        assert_eq!(list(&fresh, "Entities").len(), 7);
        assert_eq!(ints(&fresh, "Position"), [-9, 7]);

        let source = chunk(POI, 0);
        let mut merged = chunk(POI, 1);
        merge_poi(&source, &mut merged, &parse_ranges("-4:3").unwrap(), 0).unwrap();
        let keys: Vec<&str> = merged
            .compound("Sections")
            .unwrap()
            .iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(keys, ["-4", "5", "3"]);
        assert_eq!(
            string(&merged, "Sections/-4/Records/0/type"),
            "minecraft:home"
        );
        assert_eq!(
            string(&merged, "Sections/5/Records/0/type"),
            "minecraft:fletcher"
        );
        assert_eq!(
            string(&merged, "Sections/3/Records/0/type"),
            "minecraft:meeting"
        );
        let mut fresh = empty_poi(4, -2, 4189);
        merge_poi(&source, &mut fresh, &parse_ranges("19:").unwrap(), 0).unwrap();
        let keys: Vec<&str> = fresh
            .compound("Sections")
            .unwrap()
            .iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(keys, ["19"]);
    }

    #[test]
    fn builds_empty_chunks() {
        let modern = empty_region(3, -2, 4189);
        assert_eq!(
            modern.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            ["xPos", "yPos", "zPos", "Status", "DataVersion"]
        );
        assert_eq!(
            (
                int(&modern, "xPos"),
                int(&modern, "yPos"),
                int(&modern, "zPos")
            ),
            (3, -4, -2)
        );
        assert_eq!(string(&modern, "Status"), "minecraft:full");
        assert_eq!(string(&empty_region(0, 0, 2975), "Status"), "full");
        let level = empty_region(5, 6, 2586);
        assert_eq!(
            (
                int(&level, "Level/xPos"),
                int(&level, "Level/zPos"),
                int(&level, "DataVersion")
            ),
            (5, 6, 2586)
        );
        assert_eq!(string(&level, "Level/Status"), "full");
        assert_eq!(
            string(&empty_region(0, 0, 1519), "Level/Status"),
            "postprocessed"
        );
        let entities = empty_entities(-1, 2, 4189);
        assert_eq!(ints(&entities, "Position"), [-1, 2]);
        assert!(list(&entities, "Entities").is_empty());
        let poi = empty_poi(0, 0, 4189);
        assert!(poi.compound("Sections").unwrap().is_empty());
        assert_eq!(int(&poi, "DataVersion"), 4189);
    }

    #[test]
    fn matches_mca_selector_offset_import() {
        // (-9, 7) -> (24, 5) and (-2, 0) -> (31, -2): --x-offset 33 --z-offset -2
        for (source_slot, expected, expected_slot) in
            [(247, MCA_OFFSET_R00, 184), (30, MCA_OFFSET_R0M1, 991)]
        {
            let mut moved = chunk(MCA_SOURCE_REGION, source_slot);
            relocate_region(&mut moved, XZ).unwrap();
            assert!(
                same(&moved, &chunk(expected, expected_slot)),
                "region chunk {source_slot}"
            );
        }
        let original = chunk(MCA_SOURCE_ENTITIES, 247);
        let mut moved = original.clone();
        relocate_entities(&mut moved, XZ).unwrap();
        assert!(same(&moved, &chunk(MCA_OFFSET_E00, 184)));
        for index in 0..3 {
            let path = format!("Entities/{index}/UUID");
            assert_ne!(ints(&moved, &path), ints(&original, &path));
        }
    }

    #[test]
    fn matches_mca_selector_section_import() {
        // --x-offset -40 --z-offset 5 --y-offset 2 --sections "0:3" into an empty world
        let offset = Offset { x: -40, y: 2, z: 5 };
        let ranges = parse_ranges("0:3").unwrap();
        for (source_slot, (x, z), expected_slot) in [(247, (-49, 12), 399), (30, (-42, 5), 182)] {
            let mut source = chunk(MCA_SOURCE_REGION, source_slot);
            relocate_region(&mut source, offset).unwrap();
            let mut merged = empty_region(x, z, data_version(&source));
            merge_region(&source, &mut merged, &ranges, offset.y).unwrap();
            let expected = chunk(MCA_SECTIONS_REGION, expected_slot);
            assert_eq!(ys(list(&merged, "sections")), [2, 3, 4, 5]);
            assert!(same(&merged, &expected), "region chunk {source_slot}");
        }
        let mut source = chunk(MCA_SOURCE_ENTITIES, 247);
        relocate_entities(&mut source, offset).unwrap();
        let mut merged = empty_entities(-49, 12, data_version(&source));
        merge_entities(&source, &mut merged, &ranges, offset.y).unwrap();
        assert!(same(&merged, &chunk(MCA_SECTIONS_ENTITIES, 399)));
    }
}
