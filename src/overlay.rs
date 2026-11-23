//! MCA Selector's chunk overlays: one number per chunk, drawn as a colour gradient over the map.
//!
//! - `InhabitedTime`, `LastUpdate`: ticks.
//! - `Timestamp`: the region header timestamp (unix seconds).
//! - `#Entities`: entities (entities/ chunk since 1.17, region/ `Level.Entities` before).
//! - `#TileEntities`: block entities.
//! - `DataVersion`: the chunk root's DataVersion (0 without one).
//! - `AverageHeight`: the mean height of the topmost block of the 256 columns that is not air,
//!   cave air, a barrier or a structure void (property-free ones), 0 for empty columns, rounded
//!   towards zero as MCA Selector's integer division does.
//! - `#Blocks` (parameter: block names, `stone,minecraft:dirt,'custom:block'`): how many blocks of
//!   these kinds the chunk holds.
//! - World Map addition `Size`: the compressed size of the region/ chunk in bytes.
//!
//! MCA Selector skips 1.18+ sections whose palette has a single entry (no `data`) when it counts
//! blocks and heights; here they count as 4096 of that block. Pre-1.13 chunks (numeric block ids)
//! count no `#Blocks`. `Path` and `Script` overlays are not available.
use crate::{
    chunk,
    filter::{self, ChunkContext, Needs},
    nbt::{Compound, Tag},
};

/// Bump when values change: invalidates cached overlay values.
pub const OVERLAY_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    InhabitedTime,
    Timestamp,
    LastUpdate,
    EntityAmount,
    TileEntityAmount,
    DataVersion,
    AverageHeight,
    BlockAmount,
    Size,
}

impl Kind {
    const ALL: [Self; 9] = [
        Self::InhabitedTime,
        Self::Timestamp,
        Self::LastUpdate,
        Self::EntityAmount,
        Self::TileEntityAmount,
        Self::DataVersion,
        Self::AverageHeight,
        Self::BlockAmount,
        Self::Size,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::InhabitedTime => "InhabitedTime",
            Self::Timestamp => "Timestamp",
            Self::LastUpdate => "LastUpdate",
            Self::EntityAmount => "#Entities",
            Self::TileEntityAmount => "#TileEntities",
            Self::DataVersion => "DataVersion",
            Self::AverageHeight => "AverageHeight",
            Self::BlockAmount => "#Blocks",
            Self::Size => "Size",
        }
    }
}

/// An overlay type with its parameter (the block names of `#Blocks`).
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    kind: Kind,
    /// `#Blocks`: namespaced block names (and quoted custom names), as MCA Selector parses them.
    blocks: Vec<String>,
}

/// The overlay named `name` (MCA Selector's names, plus `Size`).
pub fn parse(name: &str, parameter: Option<&str>) -> Result<Overlay, String> {
    let Some(kind) = Kind::ALL.into_iter().find(|kind| kind.name() == name) else {
        return Err(match name {
            "Path" | "Script" => format!("the {name} overlay is not available in World Map"),
            _ => format!("unknown overlay \"{name}\""),
        });
    };
    let parameter = parameter
        .map(str::trim)
        .filter(|parameter| !parameter.is_empty());
    if kind != Kind::BlockAmount {
        if parameter.is_some() {
            return Err(format!("the {name} overlay takes no parameter"));
        }
        return Ok(Overlay {
            kind,
            blocks: Vec::new(),
        });
    }
    let parameter = parameter.ok_or("the #Blocks overlay needs block names")?;
    let blocks = filter::parse_block_names(parameter)
        .ok_or_else(|| format!("no known block in \"{parameter}\""))?;
    Ok(Overlay { kind, blocks })
}

impl Overlay {
    /// A stable cache key: the name, with the parsed block names for `#Blocks`.
    pub fn key(&self) -> String {
        if self.kind == Kind::BlockAmount {
            format!("{}:{}", self.kind.name(), self.blocks.join(","))
        } else {
            self.kind.name().to_string()
        }
    }

    pub fn needs(&self) -> Needs {
        Needs {
            region: true,
            entities: self.kind == Kind::EntityAmount,
            poi: false,
        }
    }

    /// The chunk's value; `None` where there is no chunk to take it from.
    pub fn value(&self, chunk: &ChunkContext) -> Option<f64> {
        let version = filter::version(chunk);
        let level_number = |key: &str| {
            chunk.region?;
            Some(
                filter::level(chunk, version)
                    .and_then(|level| level.i64(key))
                    .unwrap_or(0) as f64,
            )
        };
        match self.kind {
            Kind::InhabitedTime => level_number("InhabitedTime"),
            Kind::LastUpdate => level_number("LastUpdate"),
            Kind::Timestamp => (chunk.region.is_some() || chunk.compression.is_some())
                .then_some(f64::from(chunk.timestamp)),
            Kind::EntityAmount => {
                if chunk.region.is_none() && chunk.entities.is_none() {
                    return None;
                }
                Some(filter::entities(chunk, version).map_or(0, <[Tag]>::len) as f64)
            }
            Kind::TileEntityAmount => {
                chunk.region?;
                Some(filter::tile_entities(chunk, version).map_or(0, <[Tag]>::len) as f64)
            }
            Kind::DataVersion => Some(chunk.region?.i64("DataVersion").unwrap_or(0) as f64),
            Kind::AverageHeight => Some(average_height(chunk.region?, version) as f64),
            Kind::BlockAmount => Some(block_amount(chunk.region?, version, &self.blocks) as f64),
            Kind::Size => chunk.size.map(|size| size as f64),
        }
    }
}

/// The blocks MCA Selector's AverageHeight looks through.
const EMPTY: [&str; 4] = [
    "minecraft:air",
    "minecraft:cave_air",
    "minecraft:barrier",
    "minecraft:structure_void",
];

/// A palette entry that counts as empty: one of [`EMPTY`] without block state properties.
fn empty_entry(entry: &Tag, name: &str) -> bool {
    EMPTY.contains(&name)
        && match entry {
            Tag::String(_) => true,
            Tag::Compound(compound) => {
                compound.get("Properties").is_none() && compound.get("properties").is_none()
            }
            _ => false,
        }
}

/// A section's 4096 palette indices (`y * 256 + z * 16 + x`); a single-entry palette is that
/// block everywhere.
fn block_indices(section: &Compound, version: i64, palette: usize) -> Option<Vec<u16>> {
    if palette == 1 {
        return Some(vec![0; 4096]);
    }
    let data = if version >= filter::SECTION_PALETTES {
        section.compound("block_states")?.get("data")?.as_longs()?
    } else {
        section.get("BlockStates")?.as_longs()?
    };
    chunk::unpack(
        data,
        chunk::bits_for(palette).max(4),
        4096,
        version < filter::NO_SPANNING,
    )
}

/// One section's blocks: its Y, per palette entry its name and emptiness, and the indices.
struct Section<'a> {
    y: i64,
    names: Vec<&'a str>,
    empty: Vec<bool>,
    indices: Vec<u16>,
}

/// The block sections of a 1.13+ chunk; sections without a readable palette are skipped.
fn palette_sections(root: &Compound, version: i64) -> Vec<Section<'_>> {
    let Some(sections) = filter::sections(root, version) else {
        return Vec::new();
    };
    sections
        .iter()
        .filter_map(Tag::as_compound)
        .filter_map(|section| {
            let y = section.i64("Y")?;
            let entries = filter::block_palette(section, version)?;
            let names = chunk::palette_names(entries).filter(|names| !names.is_empty())?;
            let indices = block_indices(section, version, names.len())?;
            let empty = entries
                .iter()
                .zip(&names)
                .map(|(entry, name)| empty_entry(entry, name))
                .collect();
            Some(Section {
                y,
                names,
                empty,
                indices,
            })
        })
        .collect()
}

/// `getAverageHeight`: the sum of the top non-empty block of every column over 256.
fn average_height(root: &Compound, version: i64) -> i64 {
    // per section from the top: whether each of its 4096 blocks is solid
    let mut solid: Vec<(i64, Vec<bool>)> = if version < filter::PALETTES {
        // numeric block ids: air (0), barrier (166), structure void (217)
        filter::sections(root, version)
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_compound)
            .filter_map(|section| {
                let y = section.i64("Y")?;
                let blocks = section.get("Blocks")?.as_bytes()?;
                (blocks.len() >= 4096).then(|| {
                    let solid = blocks[..4096]
                        .iter()
                        .map(|&id| !matches!(id as u8, 0 | 166 | 217))
                        .collect();
                    (y, solid)
                })
            })
            .collect()
    } else {
        palette_sections(root, version)
            .into_iter()
            .map(|section| {
                let solid = section
                    .indices
                    .iter()
                    .map(|&index| section.empty.get(index as usize) == Some(&false))
                    .collect();
                (section.y, solid)
            })
            .collect()
    };
    solid.sort_by(|a, b| b.0.cmp(&a.0));

    let mut total = 0;
    for x in 0..16 {
        for z in 0..16 {
            'column: for (y, blocks) in &solid {
                for cy in (0..16).rev() {
                    if blocks[cy * 256 + z * 16 + x] {
                        total += y * 16 + cy as i64;
                        break 'column;
                    }
                }
            }
        }
    }
    total / 256
}

/// `getBlockAmount`: blocks whose palette entry is one of `blocks`.
fn block_amount(root: &Compound, version: i64, blocks: &[String]) -> i64 {
    if version < filter::PALETTES {
        return 0;
    }
    palette_sections(root, version)
        .iter()
        .map(|section| {
            let wanted: Vec<bool> = section
                .names
                .iter()
                .map(|name| blocks.iter().any(|block| block == *name))
                .collect();
            section
                .indices
                .iter()
                .filter(|&&index| wanted.get(index as usize) == Some(&true))
                .count() as i64
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil;

    /// The NBT, header timestamp, compression and size of the chunk in slot `index`.
    fn stored(data: &[u8], index: usize) -> (Compound, u32, u8, usize) {
        let slot = anvil::slots(data)
            .unwrap()
            .into_iter()
            .find(|slot| slot.index == index)
            .expect("slot");
        let (compression, payload) = anvil::payload(data, slot).unwrap().unwrap();
        let start = anvil::SECTOR + index * 4;
        (
            anvil::chunk(data, slot).unwrap().unwrap(),
            u32::from_be_bytes(data[start..start + 4].try_into().unwrap()),
            compression,
            payload.len(),
        )
    }

    fn context<'a>(
        index: usize,
        region: Option<&'a (Compound, u32, u8, usize)>,
        entities: Option<&'a Compound>,
    ) -> ChunkContext<'a> {
        ChunkContext {
            x: (index % 32) as i32,
            z: (index / 32) as i32,
            region: region.map(|stored| &stored.0),
            entities,
            poi: None,
            timestamp: region.map_or(0, |stored| stored.1),
            compression: region.map(|stored| stored.2),
            external: false,
            size: region.map(|stored| stored.3),
            neighbours: None,
        }
    }

    const MODERN: &[u8] = include_bytes!("../tests/fixtures/filter_modern.mca");
    const ENTITIES: &[u8] = include_bytes!("../tests/fixtures/filter_entities.mca");
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/filter_legacy.mca");
    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const V26: &[u8] = include_bytes!("../tests/fixtures/v26.mca");
    const OLD: &[u8] = include_bytes!("../tests/fixtures/legacy.mca");

    fn value(name: &str, parameter: Option<&str>, chunk: &ChunkContext) -> Option<f64> {
        parse(name, parameter).unwrap().value(chunk)
    }

    #[test]
    fn parsing_and_keys() {
        assert_eq!(parse("InhabitedTime", None).unwrap().key(), "InhabitedTime");
        assert_eq!(parse("Size", Some("")).unwrap().key(), "Size");
        assert_eq!(
            parse(
                "#Blocks",
                Some("stone, minecraft:dirt,'custom:block',notablock")
            )
            .unwrap()
            .key(),
            "#Blocks:minecraft:stone,minecraft:dirt,custom:block"
        );
        assert!(parse("#Blocks", None).is_err());
        assert!(parse("#Blocks", Some("notablock")).is_err());
        assert!(parse("DataVersion", Some("stone")).is_err());
        assert!(parse("Script", None).unwrap_err().contains("not available"));
        assert!(parse("Path", None).unwrap_err().contains("not available"));
        assert!(parse("Nope", None).is_err());
        assert_eq!(
            parse("#Entities", None).unwrap().needs(),
            Needs {
                region: true,
                entities: true,
                poi: false
            }
        );
        assert_eq!(
            parse("AverageHeight", None).unwrap().needs(),
            Needs {
                region: true,
                entities: false,
                poi: false
            }
        );
    }

    #[test]
    fn modern_values() {
        let spawner = stored(MODERN, 121);
        let proto = stored(MODERN, 18);
        let chest = stored(MODERN, 89);
        let entities = stored(ENTITIES, 121);
        let spawner = context(121, Some(&spawner), Some(&entities.0));
        let proto = context(18, Some(&proto), None);
        let chest = context(89, Some(&chest), None);

        assert_eq!(value("InhabitedTime", None, &spawner), Some(1200.0));
        assert_eq!(value("LastUpdate", None, &spawner), Some(99_389_128.0));
        assert_eq!(value("Timestamp", None, &spawner), Some(1_710_000_000.0));
        assert_eq!(value("Timestamp", None, &proto), Some(1_600_000_000.0));
        assert_eq!(value("#Entities", None, &spawner), Some(3.0));
        assert_eq!(value("#Entities", None, &chest), Some(0.0));
        assert_eq!(value("#TileEntities", None, &spawner), Some(2.0));
        assert_eq!(value("#TileEntities", None, &chest), Some(1.0));
        assert_eq!(value("DataVersion", None, &spawner), Some(4189.0));
        assert_eq!(value("Size", None, &spawner), Some(5347.0));
        assert_eq!(value("AverageHeight", None, &proto), Some(67.0));
        assert_eq!(value("AverageHeight", None, &chest), Some(62.0));
        assert_eq!(value("AverageHeight", None, &spawner), Some(62.0));
        let stone = Some("stone,spawner");
        assert_eq!(value("#Blocks", stone, &proto), Some(9975.0));
        assert_eq!(value("#Blocks", stone, &chest), Some(8578.0));
        assert_eq!(value("#Blocks", stone, &spawner), Some(9540.0));
        let diamonds = Some("diamond_ore,deepslate_diamond_ore");
        assert_eq!(value("#Blocks", diamonds, &proto), Some(0.0));
        assert_eq!(value("#Blocks", diamonds, &chest), Some(17.0));
        assert_eq!(value("#Blocks", diamonds, &spawner), Some(25.0));

        let overworld = stored(OVERWORLD, 52);
        let overworld = context(52, Some(&overworld), None);
        assert_eq!(value("AverageHeight", None, &overworld), Some(69.0));
        assert_eq!(value("#Blocks", stone, &overworld), Some(9225.0));
        assert_eq!(value("#Blocks", diamonds, &overworld), Some(34.0));
        let unfinished = stored(OVERWORLD, 9);
        let unfinished = context(9, Some(&unfinished), None);
        assert_eq!(value("AverageHeight", None, &unfinished), Some(0.0));

        // 26.x palettes of plain strings
        let v26 = stored(V26, 97);
        let v26 = context(97, Some(&v26), None);
        assert_eq!(value("InhabitedTime", None, &v26), Some(7174.0));
        assert_eq!(value("DataVersion", None, &v26), Some(5023.0));
        assert_eq!(value("AverageHeight", None, &v26), Some(73.0));
        assert_eq!(value("#Blocks", stone, &v26), Some(13_411.0));
        assert_eq!(value("#Blocks", diamonds, &v26), Some(32.0));
    }

    #[test]
    fn legacy_values() {
        let v116 = stored(LEGACY, 0);
        let v112 = stored(LEGACY, 1);
        let v113 = stored(OLD, 1);
        let v116 = context(0, Some(&v116), None);
        let v112 = context(1, Some(&v112), None);
        let v113 = context(1, Some(&v113), None);
        let stone = Some("stone");

        assert_eq!(value("InhabitedTime", None, &v116), Some(72_000.0));
        assert_eq!(value("LastUpdate", None, &v116), Some(1000.0));
        assert_eq!(value("#Entities", None, &v116), Some(2.0));
        assert_eq!(value("#TileEntities", None, &v116), Some(1.0));
        assert_eq!(value("DataVersion", None, &v116), Some(2586.0));
        // stone up to y = 59 (sand under the pool), grass or water at 60
        assert_eq!(value("AverageHeight", None, &v116), Some(60.0));
        assert_eq!(value("#Blocks", stone, &v116), Some(15_344.0));
        // 1.13: block states spanning longs
        assert_eq!(value("AverageHeight", None, &v113), Some(60.0));
        assert_eq!(value("#Blocks", stone, &v113), Some(15_344.0));
        // 1.12: numeric ids, one section of stone; no block names to count
        assert_eq!(value("AverageHeight", None, &v112), Some(15.0));
        assert_eq!(value("#Blocks", stone, &v112), Some(0.0));
        assert_eq!(value("#Entities", None, &v112), Some(1.0));
        assert_eq!(value("DataVersion", None, &v112), Some(1343.0));
    }

    #[test]
    fn missing_chunks() {
        let entities = stored(ENTITIES, 121);
        let only_entities = context(121, None, Some(&entities.0));
        let nothing = context(5, None, None);
        for name in [
            "InhabitedTime",
            "Timestamp",
            "LastUpdate",
            "#TileEntities",
            "DataVersion",
            "AverageHeight",
            "Size",
        ] {
            assert_eq!(value(name, None, &only_entities), None, "{name}");
            assert_eq!(value(name, None, &nothing), None, "{name}");
        }
        assert_eq!(value("#Blocks", Some("stone"), &nothing), None);
        assert_eq!(value("#Entities", None, &only_entities), Some(3.0));
        assert_eq!(value("#Entities", None, &nothing), None);
    }
}
