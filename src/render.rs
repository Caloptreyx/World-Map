//! Renders a region file as a 512 × 512 top-down image, one pixel per block: block colours
//! tinted by biome, water by its depth, and slopes shaded against the block to the north (as
//! the game's map items do). Chunks that are missing, unfinished or unreadable stay
//! transparent.
use crate::{
    anvil,
    chunk::{self, Parsed},
    colors::{self, Rgb},
};

pub const SIZE: usize = 512;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rendered: usize,
    pub unfinished: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub external: usize,
}

pub struct Image {
    /// RGBA, `SIZE * SIZE * 4` bytes, row by row from north to south.
    pub rgba: Vec<u8>,
    pub stats: Stats,
}

const EMPTY: i32 = i32::MIN;

fn scale(rgb: Rgb, factor: f32) -> Rgb {
    rgb.map(|c| (c as f32 * factor).round().clamp(0.0, 255.0) as u8)
}

fn mix(a: Rgb, b: Rgb, amount_of_b: f32) -> Rgb {
    std::array::from_fn(|i| {
        (a[i] as f32 * (1.0 - amount_of_b) + b[i] as f32 * amount_of_b).round() as u8
    })
}

/// The colour of a column before shading.
fn column_colour(surface: &chunk::Surface, column: &chunk::Column) -> Rgb {
    let biome = colors::biome(column.biome.map(|b| surface.biomes[b as usize].as_str()));
    let top = colors::colour(colors::block(&surface.names[column.top as usize]), &biome);
    if column.depth == 0 {
        return top;
    }
    // shallow water shows its floor, deep water gets darker
    let water = colors::colour(colors::block("minecraft:water"), &biome);
    let floor = column
        .floor
        .map(|floor| colors::colour(colors::block(&surface.names[floor as usize]), &biome))
        .unwrap_or(water);
    let depth = column.depth as f32;
    let shown = mix(water, floor, (0.45 - depth * 0.1).max(0.0));
    scale(shown, (1.1 - depth * 0.025).clamp(0.65, 1.1))
}

/// The index of column `column` (`z * 16 + x`) of the chunk in slot `slot` within the
/// region's `SIZE * SIZE` grid, row by row from north to south.
pub fn pixel(slot: usize, column: usize) -> usize {
    let (cx, cz) = (slot % 32, slot / 32);
    (cz * 16 + column / 16) * SIZE + cx * 16 + column % 16
}

/// Reads the surface of every chunk of a region file and hands each one to `visit` with its
/// slot index; chunks that cannot be shown (missing, unfinished, too old, unreadable or
/// external) are only counted. Shared by the 2D tiles and the 3D surfaces.
pub fn surfaces(
    data: &[u8],
    ceiling: Option<i32>,
    mut visit: impl FnMut(usize, &chunk::Surface),
) -> Result<Stats, String> {
    let mut stats = Stats::default();
    for slot in anvil::slots(data)? {
        let root = match anvil::chunk(data, slot) {
            Ok(Some(root)) => root,
            Ok(None) => {
                stats.external += 1;
                continue;
            }
            Err(_) => {
                stats.failed += 1;
                continue;
            }
        };
        match chunk::surface(&root, ceiling) {
            Ok(Parsed::Surface(surface)) => {
                stats.rendered += 1;
                visit(slot.index, &surface);
            }
            Ok(Parsed::Unfinished) => stats.unfinished += 1,
            Ok(Parsed::Unsupported) => stats.unsupported += 1,
            Err(_) => stats.failed += 1,
        }
    }
    Ok(stats)
}

/// Renders the chunks of a region file. `ceiling` is the Nether's scan start (see
/// [`chunk::surface`]).
pub fn region(data: &[u8], ceiling: Option<i32>) -> Result<Image, String> {
    let mut colours = vec![[0u8; 3]; SIZE * SIZE];
    let mut heights = vec![EMPTY; SIZE * SIZE];
    let mut wet = vec![false; SIZE * SIZE];
    let stats = surfaces(data, ceiling, |slot, surface| {
        for (i, column) in surface.columns.iter().enumerate() {
            let Some(column) = column else { continue };
            let pixel = pixel(slot, i);
            colours[pixel] = column_colour(surface, column);
            heights[pixel] = column.y;
            wet[pixel] = column.depth > 0;
        }
    })?;

    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for pixel in 0..SIZE * SIZE {
        let height = heights[pixel];
        if height == EMPTY {
            continue;
        }
        let north = if pixel >= SIZE {
            heights[pixel - SIZE]
        } else {
            EMPTY
        };
        let factor = if wet[pixel] || north == EMPTY {
            0.94
        } else if height > north {
            1.0
        } else if height < north {
            0.8
        } else {
            0.94
        };
        let rgb = scale(colours[pixel], factor);
        rgba[pixel * 4..pixel * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(Image { rgba, stats })
}

/// The block column `(x, z)` (block coordinates within the region, 0 to 511) with the
/// surface of its chunk, reading only that chunk. `None` when the chunk is not there (or
/// external), not finished or too old, or the column has no visible block.
pub fn column(
    data: &[u8],
    x: usize,
    z: usize,
    ceiling: Option<i32>,
) -> Result<Option<(chunk::Surface, chunk::Column)>, String> {
    let index = (z / 16) * 32 + x / 16;
    let Some(slot) = anvil::slots(data)?.into_iter().find(|slot| slot.index == index) else {
        return Ok(None);
    };
    let Some(root) = anvil::chunk(data, slot)? else {
        return Ok(None);
    };
    Ok(match chunk::surface(&root, ceiling)? {
        Parsed::Surface(surface) => {
            let column = surface.columns[(z % 16) * 16 + x % 16];
            column.map(|column| (surface, column))
        }
        Parsed::Unfinished | Parsed::Unsupported => None,
    })
}

/// The height a player can stand at on the block column `(x, z)` (block coordinates within
/// the region, 0 to 511): one above its topmost visible block. `None` when the chunk is not
/// there or not finished.
pub fn standing_height(data: &[u8], x: usize, z: usize, ceiling: Option<i32>) -> Result<Option<i32>, String> {
    Ok(column(data, x, z, ceiling)?.map(|(_, column)| column.y + 1))
}

/// A PNG of RGBA pixels.
pub fn png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
    writer
        .write_image_data(rgba)
        .map_err(|err| err.to_string())?;
    writer.finish().map_err(|err| err.to_string())?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/legacy.mca");

    fn pixel(image: &Image, x: usize, z: usize) -> [u8; 4] {
        image.rgba[(z * SIZE + x) * 4..(z * SIZE + x) * 4 + 4]
            .try_into()
            .unwrap()
    }

    #[test]
    fn renders_finished_chunks_and_leaves_the_rest_transparent() {
        let image = region(OVERWORLD, None).unwrap();
        assert_eq!(
            image.stats,
            Stats {
                rendered: 4,
                unfinished: 1,
                ..Stats::default()
            }
        );
        // chunk slot 52 is (20, 1): its first column is desert sand
        let sand = pixel(&image, 20 * 16, 16);
        assert_eq!(sand[3], 255);
        let expected = colors::block("minecraft:sand").rgb;
        for i in 0..3 {
            assert!(
                (sand[i] as i32 - expected[i] as i32).abs() <= 52,
                "{sand:?} vs {expected:?}"
            );
        }
        // nothing outside the stored chunks
        assert_eq!(pixel(&image, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&image, 511, 511), [0, 0, 0, 0]);
    }

    #[test]
    fn counts_unsupported_chunks_and_shows_water() {
        let image = region(LEGACY, None).unwrap();
        assert_eq!(image.stats.rendered, 2);
        assert_eq!(image.stats.unsupported, 1);
        let pool = pixel(&image, 5, 5);
        assert!(pool[2] > pool[0], "water is blue: {pool:?}");
        let grass = pixel(&image, 0, 1);
        assert!(grass[1] > grass[2], "grass is green: {grass:?}");
    }

    #[test]
    fn finds_where_to_stand() {
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/expected.json")).unwrap();
        let top = expected["overworld"]["chunks"]["52"][3][5][1].as_i64().unwrap() as i32;
        // slot 52 is chunk (20, 1) of the region: blocks 320..336, 16..32
        assert_eq!(standing_height(OVERWORLD, 325, 19, None).unwrap(), Some(top + 1));
        assert_eq!(standing_height(OVERWORLD, 0, 0, None).unwrap(), None);
    }

    #[test]
    fn encodes_png() {
        let image = region(LEGACY, None).unwrap();
        let encoded = png(&image.rgba, SIZE as u32, SIZE as u32).unwrap();
        assert!(encoded.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoder = png::Decoder::new(std::io::Cursor::new(&encoded));
        let reader = decoder.read_info().unwrap();
        assert_eq!(reader.info().width, SIZE as u32);
    }
}
