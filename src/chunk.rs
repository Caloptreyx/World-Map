//! What a chunk looks like from above: the topmost visible block of every column, the water
//! above it and the biome there. Reads the 1.18+ format (`sections` with `block_states` and
//! `biomes` palettes; since 26.x palette entries may be plain strings or `{id, properties}`)
//! and the 1.13 to 1.17 format (`Level.Sections` with `Palette` and `BlockStates`); older
//! chunks (numeric block ids) are reported as unsupported.
use crate::{
    colors,
    nbt::{Compound, Tag},
};
use std::collections::HashMap;

/// 20w17a (1.16): block states stopped spanning two longs.
const NO_SPANNING: i64 = 2529;
/// 17w47a (1.13): block palettes.
const PALETTES: i64 = 1451;

/// The topmost visible block of a column. Under water `top` is the water and `floor` the first
/// block below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// Index into [`Surface::names`].
    pub top: u16,
    pub y: i32,
    /// Blocks of water above the floor (0 on dry land).
    pub depth: u16,
    /// Index into [`Surface::names`] of the block under the water.
    pub floor: Option<u16>,
    /// Index into [`Surface::biomes`].
    pub biome: Option<u16>,
}

#[derive(Debug, Default)]
pub struct Surface {
    pub names: Vec<String>,
    pub biomes: Vec<String>,
    /// `z * 16 + x`; `None` for columns without any visible block.
    pub columns: Vec<Option<Column>>,
}

#[derive(Debug)]
pub enum Parsed {
    Surface(Surface),
    /// Still being generated: the game has not finished it.
    Unfinished,
    /// Saved before 1.13 (numeric block ids).
    Unsupported,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Invisible,
    Water,
    Solid,
}

struct Interner {
    names: Vec<String>,
    index: HashMap<String, u16>,
}

impl Interner {
    fn new() -> Self {
        Self {
            names: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn id(&mut self, name: &str) -> u16 {
        if let Some(&id) = self.index.get(name) {
            return id;
        }
        let id = self.names.len() as u16;
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), id);
        id
    }
}

/// Bits per entry for a palette of `n` entries (0 for one entry).
pub fn bits_for(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// `count` palette indices of `bits` bits packed in longs, either never spanning two longs
/// (1.16+) or spanning (1.13 to 1.15).
pub fn unpack(longs: &[i64], bits: u32, count: usize, spanning: bool) -> Option<Vec<u16>> {
    if bits == 0 || bits > 16 {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    let bits = bits as usize;
    let mut output = Vec::with_capacity(count);
    if spanning {
        if longs.len() * 64 < count * bits {
            return None;
        }
        for i in 0..count {
            let bit = i * bits;
            let (word, offset) = (bit / 64, bit % 64);
            let mut value = (longs[word] as u64) >> offset;
            if offset + bits > 64 {
                value |= (longs[word + 1] as u64) << (64 - offset);
            }
            output.push((value & mask) as u16);
        }
    } else {
        let per = 64 / bits;
        if longs.len() < count.div_ceil(per) {
            return None;
        }
        for i in 0..count {
            let value = (longs[i / per] as u64) >> ((i % per) * bits);
            output.push((value & mask) as u16);
        }
    }
    Some(output)
}

/// One section's blocks: palette entries as interned names with their kind, and 4096 palette
/// indices (`y * 256 + z * 16 + x`), absent for single-entry palettes.
struct Section {
    y: i32,
    palette: Vec<(u16, Kind)>,
    indices: Option<Vec<u16>>,
    biomes: Vec<u16>,
    biome_indices: Option<Vec<u16>>,
}

impl Section {
    fn block(&self, x: usize, y: usize, z: usize) -> (u16, Kind) {
        let index = match &self.indices {
            Some(indices) => indices[y * 256 + z * 16 + x] as usize,
            None => 0,
        };
        self.palette
            .get(index)
            .copied()
            .unwrap_or(self.palette[0])
    }

    fn biome(&self, x: usize, y: usize, z: usize) -> Option<u16> {
        let index = match &self.biome_indices {
            Some(indices) => indices[(y >> 2) * 16 + (z >> 2) * 4 + (x >> 2)] as usize,
            None => 0,
        };
        self.biomes.get(index).or(self.biomes.first()).copied()
    }

    fn all_invisible(&self) -> bool {
        self.palette.iter().all(|&(_, kind)| kind == Kind::Invisible)
    }
}

fn kind(name: &str) -> Kind {
    if colors::invisible(name) {
        Kind::Invisible
    } else if colors::watery(name) {
        Kind::Water
    } else {
        Kind::Solid
    }
}

/// The block names of a palette: `{Name, Properties}` entries before 26.x; since then plain
/// strings or `{id, properties}`, with strings wrapped as `{"": name}` when a list mixes both.
fn palette_names(entries: &[Tag]) -> Option<Vec<&str>> {
    entries
        .iter()
        .map(|entry| match entry {
            Tag::String(name) => Some(name.as_str()),
            Tag::Compound(compound) => compound
                .str("Name")
                .or_else(|| compound.str("id"))
                .or_else(|| compound.str("")),
            _ => None,
        })
        .collect()
}

fn block_palette(names: &[&str], interner: &mut Interner) -> Vec<(u16, Kind)> {
    names
        .iter()
        .map(|name| (interner.id(name), kind(name)))
        .collect()
}

/// A 1.18+ section; `None` for sections without blocks (light-only sections above and below
/// the world).
fn modern_section(
    section: &Compound,
    blocks: &mut Interner,
    biomes: &mut Interner,
) -> Result<Option<Section>, String> {
    let Some(states) = section.compound("block_states") else {
        return Ok(None);
    };
    let y = section.i64("Y").ok_or("a section has no Y")? as i32;
    let names = states
        .list("palette")
        .and_then(palette_names)
        .filter(|names| !names.is_empty())
        .ok_or("a section has no block palette")?;
    let palette = block_palette(&names, blocks);
    let indices = if palette.len() > 1 {
        let data = states
            .get("data")
            .and_then(Tag::as_longs)
            .ok_or("a section has no block data")?;
        Some(
            unpack(data, bits_for(palette.len()).max(4), 4096, false)
                .ok_or("a section's block data is too short")?,
        )
    } else {
        None
    };

    let mut biome_palette = Vec::new();
    let mut biome_indices = None;
    if let Some(section_biomes) = section.compound("biomes") {
        let names: Vec<&str> = section_biomes
            .list("palette")
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_str)
            .collect();
        biome_palette = names.iter().map(|name| biomes.id(name)).collect();
        if biome_palette.len() > 1
            && let Some(data) = section_biomes.get("data").and_then(Tag::as_longs)
        {
            biome_indices = unpack(data, bits_for(biome_palette.len()), 64, false);
        }
    }
    Ok(Some(Section {
        y,
        palette,
        indices,
        biomes: biome_palette,
        biome_indices,
    }))
}

/// A 1.13 to 1.17 section; `None` without a palette (empty or light-only).
fn legacy_section(
    section: &Compound,
    blocks: &mut Interner,
    spanning: bool,
) -> Result<Option<Section>, String> {
    let Some(names) = section.list("Palette").and_then(palette_names) else {
        return Ok(None);
    };
    if names.is_empty() {
        return Ok(None);
    }
    let y = section.i64("Y").ok_or("a section has no Y")? as i32;
    let palette = block_palette(&names, blocks);
    let indices = if palette.len() > 1 {
        let data = section
            .get("BlockStates")
            .and_then(Tag::as_longs)
            .ok_or("a section has no block states")?;
        Some(
            unpack(data, bits_for(palette.len()).max(4), 4096, spanning)
                .ok_or("a section's block states are too short")?,
        )
    } else {
        None
    };
    Ok(Some(Section {
        y,
        palette,
        indices,
        biomes: Vec::new(),
        biome_indices: None,
    }))
}

fn finished(status: Option<&str>) -> bool {
    status.is_some_and(|status| {
        matches!(
            status.strip_prefix("minecraft:").unwrap_or(status),
            "full" | "postprocessed" | "fullchunk"
        )
    })
}

#[derive(Clone, Copy, Default)]
struct State {
    /// A see-through block was passed (always true without a ceiling).
    open: bool,
    water: Option<(u16, i32, Option<u16>)>,
    depth: u16,
    done: Option<Column>,
}

/// The surface of a chunk. With `ceiling` (the Nether), the scan starts at that height and
/// first has to pass a see-through block, so the roof is skipped.
pub fn surface(root: &Compound, ceiling: Option<i32>) -> Result<Parsed, String> {
    let data_version = root
        .i64("DataVersion")
        .or_else(|| root.compound("Level").and_then(|level| level.i64("DataVersion")));
    let mut blocks = Interner::new();
    let mut biomes = Interner::new();
    let mut sections = Vec::new();

    if let Some(list) = root.list("sections") {
        if !finished(root.str("Status")) {
            return Ok(Parsed::Unfinished);
        }
        for section in list.iter().filter_map(Tag::as_compound) {
            if let Some(section) = modern_section(section, &mut blocks, &mut biomes)? {
                sections.push(section);
            }
        }
    } else if let Some(level) = root.compound("Level") {
        let list = level.list("Sections").unwrap_or_default();
        let ancient = list
            .iter()
            .filter_map(Tag::as_compound)
            .any(|section| section.get("Blocks").and_then(Tag::as_bytes).is_some());
        if ancient || data_version.is_none_or(|version| version < PALETTES) {
            return Ok(Parsed::Unsupported);
        }
        if !finished(level.str("Status")) {
            return Ok(Parsed::Unfinished);
        }
        let spanning = data_version.is_some_and(|version| version < NO_SPANNING);
        for section in list.iter().filter_map(Tag::as_compound) {
            if let Some(section) = legacy_section(section, &mut blocks, spanning)? {
                sections.push(section);
            }
        }
    } else {
        return Err("not a chunk (no sections)".into());
    }
    sections.sort_by_key(|section| std::cmp::Reverse(section.y));

    let mut states = [State {
        open: ceiling.is_none(),
        ..State::default()
    }; 256];
    let mut remaining = 256;
    let mut previous: Option<i32> = None;
    for section in &sections {
        if let Some(ceiling) = ceiling
            && section.y * 16 > ceiling
        {
            continue;
        }
        // a missing section between two present ones is air
        if previous.is_some_and(|above| above - section.y > 1) {
            for state in states.iter_mut() {
                state.open = true;
            }
        }
        previous = Some(section.y);
        if section.all_invisible() {
            for state in states.iter_mut() {
                state.open = true;
            }
            continue;
        }
        for (column, state) in states.iter_mut().enumerate() {
            if state.done.is_some() {
                continue;
            }
            let (x, z) = (column % 16, column / 16);
            for ly in (0..16).rev() {
                let y = section.y * 16 + ly as i32;
                if ceiling.is_some_and(|ceiling| y > ceiling) {
                    continue;
                }
                let (block, kind) = section.block(x, ly, z);
                match kind {
                    Kind::Invisible => state.open = true,
                    _ if !state.open => {}
                    Kind::Water => {
                        if state.water.is_none() {
                            state.water = Some((block, y, section.biome(x, ly, z)));
                        }
                        state.depth = state.depth.saturating_add(1);
                    }
                    Kind::Solid => {
                        state.done = Some(match state.water {
                            Some((water, top, biome)) => Column {
                                top: water,
                                y: top,
                                depth: state.depth,
                                floor: Some(block),
                                biome,
                            },
                            None => Column {
                                top: block,
                                y,
                                depth: 0,
                                floor: None,
                                biome: section.biome(x, ly, z),
                            },
                        });
                        remaining -= 1;
                        break;
                    }
                }
            }
        }
        if remaining == 0 {
            break;
        }
    }

    let columns = states
        .iter()
        .map(|state| {
            state.done.or_else(|| {
                // water down to the bottom of the world
                state.water.map(|(water, top, biome)| Column {
                    top: water,
                    y: top,
                    depth: state.depth,
                    floor: None,
                    biome,
                })
            })
        })
        .collect();
    Ok(Parsed::Surface(Surface {
        names: blocks.names,
        biomes: biomes.names,
        columns,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil;
    use serde_json::Value;

    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const NETHER: &[u8] = include_bytes!("../tests/fixtures/nether.mca");
    const COMPRESSIONS: &[u8] = include_bytes!("../tests/fixtures/compressions.mca");
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/legacy.mca");
    const V26: &[u8] = include_bytes!("../tests/fixtures/v26.mca");
    const EXPECTED: &str = include_str!("../tests/fixtures/expected.json");

    fn expected() -> Value {
        serde_json::from_str(EXPECTED).unwrap()
    }

    fn parse(region: &[u8], index: usize, ceiling: Option<i32>) -> Parsed {
        let slot = anvil::slots(region)
            .unwrap()
            .into_iter()
            .find(|slot| slot.index == index)
            .unwrap();
        let root = anvil::chunk(region, slot).unwrap().unwrap();
        surface(&root, ceiling).unwrap()
    }

    /// Compares every column's topmost block with the reference of scripts/make_fixtures.py.
    fn assert_matches(parsed: Parsed, reference: &Value) {
        let Parsed::Surface(surface) = parsed else {
            panic!("expected a surface, got {parsed:?}");
        };
        for z in 0..16 {
            for x in 0..16 {
                let want = &reference[z][x];
                let got = surface.columns[z * 16 + x];
                match (want.as_array(), got) {
                    (None, None) => {}
                    (Some(want), Some(column)) => {
                        assert_eq!(
                            surface.names[column.top as usize],
                            want[0].as_str().unwrap(),
                            "block at x={x} z={z}"
                        );
                        assert_eq!(column.y as i64, want[1].as_i64().unwrap(), "y at x={x} z={z}");
                    }
                    (want, got) => panic!("x={x} z={z}: expected {want:?}, got {got:?}"),
                }
            }
        }
    }

    #[test]
    fn modern_chunks_match_the_reference() {
        let expected = expected();
        for (index, reference) in expected["overworld"]["chunks"].as_object().unwrap() {
            assert_matches(parse(OVERWORLD, index.parse().unwrap(), None), reference);
        }
    }

    #[test]
    fn unfinished_chunks_are_skipped() {
        let index = expected()["overworld"]["unfinished"].as_u64().unwrap() as usize;
        assert!(matches!(parse(OVERWORLD, index, None), Parsed::Unfinished));
    }

    #[test]
    fn the_nether_is_seen_below_its_roof() {
        let expected = expected();
        let ceiling = expected["nether"]["cap"].as_i64().unwrap() as i32;
        for (index, reference) in expected["nether"]["chunks"].as_object().unwrap() {
            let index = index.parse().unwrap();
            assert_matches(parse(NETHER, index, Some(ceiling)), reference);
            // from above, the nether is its bedrock roof
            let Parsed::Surface(roof) = parse(NETHER, index, None) else {
                panic!("no surface");
            };
            let top = roof.columns[0].unwrap();
            assert_eq!(roof.names[top.top as usize], "minecraft:bedrock");
        }
    }

    #[test]
    fn every_compression_gives_the_same_surface() {
        let expected = expected();
        for slot in anvil::slots(COMPRESSIONS).unwrap() {
            assert_matches(
                parse(COMPRESSIONS, slot.index, None),
                &expected["compressions"],
            );
        }
    }

    #[test]
    fn biomes_and_water_depth() {
        let Parsed::Surface(surface) = parse(OVERWORLD, 52, None) else {
            panic!("no surface");
        };
        let column = surface.columns[0].unwrap();
        assert_eq!(
            surface.biomes[column.biome.unwrap() as usize],
            "minecraft:desert"
        );

        // the 1.16 fixture has a pool four blocks wide with one block of water over sand
        let Parsed::Surface(legacy) = parse(LEGACY, 0, None) else {
            panic!("no surface");
        };
        let pool = legacy.columns[5 * 16 + 5].unwrap();
        assert_eq!(legacy.names[pool.top as usize], "minecraft:water");
        assert_eq!(pool.depth, 1);
        assert_eq!(pool.y, 60);
        assert_eq!(legacy.names[pool.floor.unwrap() as usize], "minecraft:sand");
        let dry = legacy.columns[0].unwrap();
        assert_eq!((dry.depth, dry.floor), (0, None));
    }

    #[test]
    fn chunks_saved_by_26_match_the_reference() {
        let expected = expected();
        for (index, reference) in expected["v26"].as_object().unwrap() {
            assert_matches(parse(V26, index.parse().unwrap(), None), reference);
        }
    }

    #[test]
    fn older_formats() {
        let expected = expected();
        assert_matches(parse(LEGACY, 0, None), &expected["legacy"]["1.16"]);
        assert_matches(parse(LEGACY, 1, None), &expected["legacy"]["1.13"]);
        assert!(matches!(parse(LEGACY, 2, None), Parsed::Unsupported));
    }

    #[test]
    fn unpacks_both_layouts() {
        // 5-bit values: 12 per long without spanning, crossing long boundaries with spanning
        let values: Vec<u16> = (0..20).map(|i| (i * 7 % 32) as u16).collect();
        let mut packed = vec![0i64; 2];
        for (i, value) in values.iter().enumerate() {
            packed[i / 12] |= (*value as i64) << ((i % 12) * 5);
        }
        assert_eq!(unpack(&packed, 5, 20, false).unwrap(), values);

        let mut spanning = vec![0u64; 2];
        for (i, value) in values.iter().enumerate() {
            let bit = i * 5;
            spanning[bit / 64] |= (*value as u64) << (bit % 64);
            if bit % 64 + 5 > 64 {
                spanning[bit / 64 + 1] |= (*value as u64) >> (64 - bit % 64);
            }
        }
        let spanning: Vec<i64> = spanning.into_iter().map(|v| v as i64).collect();
        assert_eq!(unpack(&spanning, 5, 20, true).unwrap(), values);
        assert_eq!(unpack(&packed[..1], 5, 20, false), None);
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(17), 5);
    }
}
