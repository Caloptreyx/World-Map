//! Block and biome colours from `data/blocks.tsv` and `data/biomes.tsv`, which
//! `scripts/gen_colors.py` derives from the vanilla client jar: a block's colour is the average
//! of the texture it shows from above; grayscale textures the game tints by biome are marked
//! and get the biome's grass, foliage or water colour here.
use std::{collections::HashMap, sync::LazyLock};

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    None,
    Grass,
    Foliage,
    DryFoliage,
    Water,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockColour {
    pub rgb: Rgb,
    pub tint: Tint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeColours {
    pub grass: Rgb,
    pub foliage: Rgb,
    pub dry_foliage: Rgb,
    pub water: Rgb,
}

/// Blocks a top-down view looks through.
pub fn invisible(name: &str) -> bool {
    matches!(
        strip(name),
        "air" | "cave_air" | "void_air" | "barrier" | "light" | "structure_void"
    )
}

/// Blocks that are (or stand in) water: the map shows the water and its depth.
pub fn watery(name: &str) -> bool {
    matches!(
        strip(name),
        "water" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
    )
}

fn strip(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn hex(text: &str) -> Option<Rgb> {
    let value = u32::from_str_radix(text, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

fn rows(table: &'static str) -> impl Iterator<Item = Vec<&'static str>> {
    table
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').collect())
}

static BLOCKS: LazyLock<HashMap<&'static str, BlockColour>> = LazyLock::new(|| {
    rows(include_str!("data/blocks.tsv"))
        .filter_map(|row| {
            let tint = match *row.get(2)? {
                "grass" => Tint::Grass,
                "foliage" => Tint::Foliage,
                "dry_foliage" => Tint::DryFoliage,
                "water" => Tint::Water,
                _ => Tint::None,
            };
            Some((
                row[0],
                BlockColour {
                    rgb: hex(row.get(1)?)?,
                    tint,
                },
            ))
        })
        .collect()
});

static BIOMES: LazyLock<HashMap<&'static str, BiomeColours>> = LazyLock::new(|| {
    rows(include_str!("data/biomes.tsv"))
        .filter_map(|row| {
            Some((
                row[0],
                BiomeColours {
                    grass: hex(row.get(1)?)?,
                    foliage: hex(row.get(2)?)?,
                    dry_foliage: hex(row.get(3)?)?,
                    water: hex(row.get(4)?)?,
                },
            ))
        })
        .collect()
});

/// What blocks of other mods and unknown blocks look like: the vanilla block their name ends
/// with (`mymod:redwood_planks` as planks), else neutral gray.
const FALLBACKS: &[(&str, &str)] = &[
    ("leaves", "oak_leaves"),
    ("log", "oak_log"),
    ("wood", "oak_wood"),
    ("planks", "oak_planks"),
    ("grass", "grass_block"),
    ("dirt", "dirt"),
    ("sand", "sand"),
    ("gravel", "gravel"),
    ("stone", "stone"),
    ("ore", "stone"),
    ("water", "water"),
    ("lava", "lava"),
    ("snow", "snow_block"),
    ("ice", "ice"),
];

const UNKNOWN: BlockColour = BlockColour {
    rgb: [0x80, 0x80, 0x80],
    tint: Tint::None,
};

/// The colour of a block by its id (`minecraft:stone`, `stone`, `mod:name`).
pub fn block(name: &str) -> BlockColour {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    if let Some(colour) = BLOCKS.get(path).filter(|_| namespace == "minecraft") {
        return *colour;
    }
    // another namespace: the longest vanilla block the name ends with, then a generic word
    let mut rest = path;
    while let Some((_, tail)) = rest.split_once('_') {
        if let Some(colour) = BLOCKS.get(tail) {
            return *colour;
        }
        rest = tail;
    }
    FALLBACKS
        .iter()
        .find(|(word, _)| path.ends_with(word))
        .and_then(|(_, vanilla)| BLOCKS.get(vanilla))
        .copied()
        .or_else(|| BLOCKS.get(path).copied())
        .unwrap_or(UNKNOWN)
}

/// The colours of a biome; plains for unknown or missing biomes.
pub fn biome(name: Option<&str>) -> BiomeColours {
    name.map(strip)
        .and_then(|name| BIOMES.get(name))
        .or_else(|| BIOMES.get("plains"))
        .copied()
        .expect("the biome table has plains")
}

/// The block's colour in a biome: tinted textures multiplied by the biome's colour.
pub fn colour(block: BlockColour, biome: &BiomeColours) -> Rgb {
    let tint = match block.tint {
        Tint::None => return block.rgb,
        Tint::Grass => biome.grass,
        Tint::Foliage => biome.foliage,
        Tint::DryFoliage => biome.dry_foliage,
        Tint::Water => biome.water,
    };
    std::array::from_fn(|i| (u16::from(block.rgb[i]) * u16::from(tint[i]) / 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_vanilla_blocks_with_and_without_namespace() {
        let stone = block("minecraft:stone");
        assert_eq!(stone, block("stone"));
        assert_eq!(stone.tint, Tint::None);
        assert_eq!(stone.rgb, [0x7e, 0x7e, 0x7e]);
        assert_eq!(block("minecraft:sand").rgb, [0xdb, 0xcf, 0xa3]);
        assert_eq!(block("minecraft:grass_block").tint, Tint::Grass);
        assert_eq!(block("minecraft:oak_leaves").tint, Tint::Foliage);
        assert_eq!(block("minecraft:water").tint, Tint::Water);
        // birch and spruce leaves have a fixed tint baked in
        assert_eq!(block("minecraft:birch_leaves").tint, Tint::None);
        assert!(BLOCKS.len() > 1000, "{}", BLOCKS.len());
    }

    #[test]
    fn other_namespaces_borrow_vanilla_colours() {
        assert_eq!(block("mymod:redwood_oak_planks"), block("oak_planks"));
        assert_eq!(block("mymod:redwood_leaves"), block("oak_leaves"));
        assert_eq!(block("mymod:strange_machine"), UNKNOWN);
        assert_eq!(block("minecraft:not_a_block"), UNKNOWN);
    }

    #[test]
    fn tints_by_biome() {
        let plains = biome(Some("minecraft:plains"));
        assert_eq!(plains.grass, [0x91, 0xbd, 0x59]);
        assert_eq!(plains.water, [0x3f, 0x76, 0xe4]);
        assert_eq!(biome(Some("swamp")).grass, [0x4c, 0x76, 0x3c]);
        assert_eq!(biome(None), plains);
        assert_eq!(biome(Some("mymod:unknown")), plains);

        let grass = colour(block("grass_block"), &plains);
        let gray = block("grass_block").rgb;
        assert_eq!(
            grass,
            std::array::from_fn(|i| (gray[i] as u16 * plains.grass[i] as u16 / 255) as u8)
        );
        assert_eq!(colour(block("stone"), &plains), block("stone").rgb);
    }

    #[test]
    fn knows_what_the_map_looks_through() {
        assert!(invisible("minecraft:air") && invisible("cave_air") && invisible("barrier"));
        assert!(!invisible("minecraft:glass"));
        assert!(watery("minecraft:water") && watery("minecraft:kelp_plant"));
        assert!(!watery("minecraft:ice"));
    }
}
