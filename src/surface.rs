//! What the 3D view draws of a region file: per block column the height of its topmost block,
//! the unshaded colour of what shows (the floor under water), and the water's depth and colour.
//! The browser lights the stepped columns itself, so none of the 2D map's slope or depth
//! shading is applied here. Served zlib-compressed; the body is (all little-endian):
//!
//! ```text
//! "WM3D", u8 format version, u8 flags (bit 0: scanned below a ceiling), u16 size, u16 palette
//! length P, then P * 3 bytes of RGB (entry 0 unused, so index 0 means none), then size * size
//! of each, row by row from north to south:
//!   i16 heights, MISSING without data; each the wrapping difference to the previous column of
//!       its row, the first of a row as is (neighbouring heights are close, so this compresses)
//!   u16 palette index of the surface colour, u8 water depth (saturated), u16 palette index of
//!       the water colour
//! ```
use crate::{
    chunk,
    colors::{self, Rgb},
    render::{self, SIZE, Stats},
};
use flate2::{Compression, write::ZlibEncoder};
use std::{collections::HashMap, io::Write};

pub const MAGIC: &[u8; 4] = b"WM3D";
pub const FORMAT: u8 = 1;
/// Flag: columns were scanned from below a ceiling (the Nether's roof).
pub const CEILING: u8 = 1;
/// The height of columns without data.
pub const MISSING: i16 = i16::MIN;

/// The uncompressed body of a region's surface and what could be read of the region.
pub struct Body {
    pub bytes: Vec<u8>,
    pub stats: Stats,
}

/// The distinct colours of a region, after the unused entry 0.
struct Palette {
    colours: Vec<Rgb>,
    ids: HashMap<Rgb, u16>,
}

impl Palette {
    fn new() -> Self {
        Self {
            colours: vec![[0; 3]],
            ids: HashMap::new(),
        }
    }

    fn id(&mut self, rgb: Rgb) -> u16 {
        if let Some(&id) = self.ids.get(&rgb) {
            return id;
        }
        // colors.rs knows about 1300 block colours and tints about 20 of them by about 70
        // biomes, so a region never comes near 65535 distinct colours
        let id = u16::try_from(self.colours.len()).expect("fewer than 65536 colours");
        self.colours.push(rgb);
        self.ids.insert(rgb, id);
        id
    }
}

/// The unshaded colour a column shows (its floor under water, the water itself when it reaches
/// the bottom of the world) and, for wet columns, the water's colour, both tinted by its biome.
fn column_colours(surface: &chunk::Surface, column: &chunk::Column) -> (Rgb, Option<Rgb>) {
    let biome = colors::biome(column.biome.map(|b| surface.biomes[b as usize].as_str()));
    let colour = |id: u16| colors::colour(colors::block(&surface.names[id as usize]), &biome);
    if column.depth == 0 {
        return (colour(column.top), None);
    }
    let water = colors::colour(colors::block("minecraft:water"), &biome);
    (column.floor.map(colour).unwrap_or(water), Some(water))
}

/// The surface of a region file. `ceiling` is the Nether's scan start (see
/// [`chunk::surface`]).
pub fn region(data: &[u8], ceiling: Option<i32>) -> Result<Body, String> {
    let count = SIZE * SIZE;
    let mut heights = vec![MISSING; count];
    let mut colours = vec![0u16; count];
    let mut depths = vec![0u8; count];
    let mut water = vec![0u16; count];
    let mut palette = Palette::new();
    let stats = render::surfaces(data, ceiling, |slot, surface| {
        for (i, column) in surface.columns.iter().enumerate() {
            let Some(column) = column else { continue };
            let pixel = render::pixel(slot, i);
            let (shown, wet) = column_colours(surface, column);
            // worlds are at most 4064 blocks tall; clamping only keeps MISSING unambiguous
            heights[pixel] = column.y.clamp(i32::from(MISSING) + 1, i32::from(i16::MAX)) as i16;
            colours[pixel] = palette.id(shown);
            depths[pixel] = column.depth.min(255) as u8;
            if let Some(rgb) = wet {
                water[pixel] = palette.id(rgb);
            }
        }
    })?;

    let mut bytes = Vec::with_capacity(10 + palette.colours.len() * 3 + count * 7);
    bytes.extend_from_slice(MAGIC);
    bytes.push(FORMAT);
    bytes.push(if ceiling.is_some() { CEILING } else { 0 });
    bytes.extend_from_slice(&(SIZE as u16).to_le_bytes());
    bytes.extend_from_slice(&(palette.colours.len() as u16).to_le_bytes());
    for rgb in &palette.colours {
        bytes.extend_from_slice(rgb);
    }
    for row in heights.chunks_exact(SIZE) {
        let mut previous = 0i16;
        for &height in row {
            bytes.extend_from_slice(&height.wrapping_sub(previous).to_le_bytes());
            previous = height;
        }
    }
    for colour in &colours {
        bytes.extend_from_slice(&colour.to_le_bytes());
    }
    bytes.extend_from_slice(&depths);
    for colour in &water {
        bytes.extend_from_slice(&colour.to_le_bytes());
    }
    Ok(Body { bytes, stats })
}

/// A zlib stream of `body`, what browsers inflate with `DecompressionStream('deflate')`.
pub fn encode(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(body.len() / 4), Compression::default());
    encoder.write_all(body).map_err(|err| err.to_string())?;
    encoder.finish().map_err(|err| err.to_string())
}

/// One block column for the block detail card.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockInfo {
    /// The topmost block (the water's surface for wet columns), as in the surface's heights.
    pub y: i32,
    pub block: String,
    pub biome: Option<String>,
    pub water_depth: u16,
    pub floor_block: Option<String>,
}

/// The block column `(x, z)` (block coordinates within the region, 0 to 511), reading only its
/// chunk. `None` when the chunk is missing, unfinished or too old or the column has no block.
pub fn inspect(data: &[u8], x: usize, z: usize, ceiling: Option<i32>) -> Result<Option<BlockInfo>, String> {
    Ok(render::column(data, x, z, ceiling)?.map(|(surface, column)| {
        let name = |id: u16| surface.names[id as usize].clone();
        BlockInfo {
            y: column.y,
            block: name(column.top),
            biome: column.biome.map(|biome| surface.biomes[biome as usize].clone()),
            water_depth: column.depth,
            floor_block: column.floor.map(name),
        }
    }))
}

/// A decoded surface, to check the encoder against.
#[cfg(test)]
#[derive(Debug)]
pub struct Decoded {
    pub ceiling: bool,
    pub palette: Vec<Rgb>,
    pub heights: Vec<i16>,
    pub colours: Vec<u16>,
    pub depths: Vec<u8>,
    pub water: Vec<u16>,
}

#[cfg(test)]
struct Reader<'a> {
    rest: &'a [u8],
}

#[cfg(test)]
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.rest.len() < n {
            return Err("the surface is truncated".into());
        }
        let rest: &'a [u8] = self.rest;
        let (head, tail) = rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn u16s(&mut self, n: usize) -> Result<Vec<u16>, String> {
        Ok(self
            .take(n * 2)?
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect())
    }
}

/// Inflates and parses a surface, undoing the height deltas.
#[cfg(test)]
pub fn decode(compressed: &[u8]) -> Result<Decoded, String> {
    use std::io::Read;
    let mut body = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut body)
        .map_err(|err| err.to_string())?;
    let mut reader = Reader { rest: &body };
    if reader.take(4)? != MAGIC.as_slice() {
        return Err("not a surface".into());
    }
    if reader.take(1)?[0] != FORMAT {
        return Err("unknown surface format".into());
    }
    let flags = reader.take(1)?[0];
    let size = reader.u16s(1)?[0] as usize;
    if size != SIZE {
        return Err(format!("unexpected size {size}"));
    }
    let entries = reader.u16s(1)?[0] as usize;
    let palette = reader
        .take(entries * 3)?
        .chunks_exact(3)
        .map(|rgb| [rgb[0], rgb[1], rgb[2]])
        .collect();
    let mut heights: Vec<i16> = reader
        .u16s(size * size)?
        .into_iter()
        .map(|stored| stored as i16)
        .collect();
    for row in heights.chunks_exact_mut(size) {
        let mut previous = 0i16;
        for height in row {
            *height = height.wrapping_add(previous);
            previous = *height;
        }
    }
    let colours = reader.u16s(size * size)?;
    let depths = reader.take(size * size)?.to_vec();
    let water = reader.u16s(size * size)?;
    if !reader.rest.is_empty() {
        return Err("the surface has trailing bytes".into());
    }
    Ok(Decoded {
        ceiling: flags & CEILING != 0,
        palette,
        heights,
        colours,
        depths,
        water,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{anvil, chunk::Parsed};
    use serde_json::Value;

    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const NETHER: &[u8] = include_bytes!("../tests/fixtures/nether.mca");
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/legacy.mca");
    const V26: &[u8] = include_bytes!("../tests/fixtures/v26.mca");
    const EXPECTED: &str = include_str!("../tests/fixtures/expected.json");

    fn expected() -> Value {
        serde_json::from_str(EXPECTED).unwrap()
    }

    fn nether_cap() -> i32 {
        expected()["nether"]["cap"].as_i64().unwrap() as i32
    }

    fn decoded(data: &[u8], ceiling: Option<i32>) -> Decoded {
        decode(&encode(&region(data, ceiling).unwrap().bytes).unwrap()).unwrap()
    }

    fn colour_of(surface: &Decoded, index: u16) -> Rgb {
        surface.palette[index as usize]
    }

    /// Every column of the decoded surface against the chunk parser, colours worked out here
    /// from colors.rs; columns of chunks it does not read are missing.
    fn assert_round_trip(data: &[u8], ceiling: Option<i32>) {
        let surface = decoded(data, ceiling);
        assert_eq!(surface.ceiling, ceiling.is_some());
        let mut seen = vec![false; SIZE * SIZE];
        for slot in anvil::slots(data).unwrap() {
            let Ok(Some(root)) = anvil::chunk(data, slot) else { continue };
            let Ok(Parsed::Surface(parsed)) = chunk::surface(&root, ceiling) else { continue };
            for (i, column) in parsed.columns.iter().enumerate() {
                let Some(column) = column else { continue };
                let pixel = render::pixel(slot.index, i);
                seen[pixel] = true;
                let biome = colors::biome(column.biome.map(|b| parsed.biomes[b as usize].as_str()));
                let rgb = |id: u16| colors::colour(colors::block(&parsed.names[id as usize]), &biome);
                let water = colors::colour(colors::block("minecraft:water"), &biome);
                assert_eq!(i32::from(surface.heights[pixel]), column.y, "slot {} column {i}", slot.index);
                assert_eq!(u16::from(surface.depths[pixel]), column.depth.min(255));
                assert_ne!(surface.colours[pixel], 0);
                if column.depth == 0 {
                    assert_eq!(colour_of(&surface, surface.colours[pixel]), rgb(column.top));
                    assert_eq!(surface.water[pixel], 0);
                } else {
                    let floor = column.floor.map(rgb).unwrap_or(water);
                    assert_eq!(colour_of(&surface, surface.colours[pixel]), floor);
                    assert_eq!(colour_of(&surface, surface.water[pixel]), water);
                }
            }
        }
        assert!(seen.iter().any(|&seen| seen), "nothing read");
        for pixel in (0..SIZE * SIZE).filter(|&pixel| !seen[pixel]) {
            assert_eq!(
                (
                    surface.heights[pixel],
                    surface.colours[pixel],
                    surface.depths[pixel],
                    surface.water[pixel]
                ),
                (MISSING, 0, 0, 0),
                "pixel {pixel}"
            );
        }
    }

    #[test]
    fn every_fixture_round_trips() {
        assert_round_trip(OVERWORLD, None);
        assert_round_trip(NETHER, Some(nether_cap()));
        assert_round_trip(NETHER, None);
        assert_round_trip(LEGACY, None);
        assert_round_trip(V26, None);
    }

    #[test]
    fn colours_are_unshaded() {
        // chunk slot 52 is (20, 1): its first column is desert sand
        let expected = expected();
        let reference = &expected["overworld"]["chunks"]["52"][0][0];
        assert_eq!(reference[0].as_str().unwrap(), "minecraft:sand");
        let surface = decoded(OVERWORLD, None);
        let pixel = 16 * SIZE + 20 * 16;
        assert_eq!(pixel, render::pixel(52, 0));
        let sand = colors::colour(
            colors::block("minecraft:sand"),
            &colors::biome(Some("minecraft:desert")),
        );
        assert_eq!(colour_of(&surface, surface.colours[pixel]), sand);
        assert_eq!(i64::from(surface.heights[pixel]), reference[1].as_i64().unwrap());
        assert_eq!((surface.depths[pixel], surface.water[pixel]), (0, 0));
        // the 2D tile darkens it (no chunk north of it), the 3D colour does not
        let image = render::region(OVERWORLD, None).unwrap();
        assert_ne!(&image.rgba[pixel * 4..pixel * 4 + 3], sand.as_slice());
    }

    #[test]
    fn water_shows_its_floor_and_depth() {
        // the 1.16 fixture has a pool with one block of water over sand
        let surface = decoded(LEGACY, None);
        let pixel = 5 * SIZE + 5;
        let plains = colors::biome(None);
        assert_eq!(surface.heights[pixel], 60);
        assert_eq!(surface.depths[pixel], 1);
        assert_eq!(
            colour_of(&surface, surface.colours[pixel]),
            colors::colour(colors::block("minecraft:sand"), &plains)
        );
        assert_eq!(
            colour_of(&surface, surface.water[pixel]),
            colors::colour(colors::block("minecraft:water"), &plains)
        );
        // the dry corner has no water colour
        assert_eq!((surface.depths[0], surface.water[0]), (0, 0));
    }

    #[test]
    fn the_nether_is_seen_below_its_roof() {
        let expected = expected();
        let below = decoded(NETHER, Some(nether_cap()));
        let roof = decoded(NETHER, None);
        assert!(below.ceiling);
        assert!(!roof.ceiling);
        let bedrock = colors::block("minecraft:bedrock").rgb;
        for (index, reference) in expected["nether"]["chunks"].as_object().unwrap() {
            let slot: usize = index.parse().unwrap();
            for z in 0..16 {
                for x in 0..16 {
                    let pixel = render::pixel(slot, z * 16 + x);
                    // columns with nothing below the roof (only air down to the void) stay empty
                    let want = reference[z][x][1].as_i64().unwrap_or(i64::from(MISSING));
                    assert_eq!(i64::from(below.heights[pixel]), want, "slot {slot} x={x} z={z}");
                    assert_ne!(below.heights[pixel], roof.heights[pixel]);
                }
            }
            let pixel = render::pixel(slot, 0);
            assert_ne!(colour_of(&below, below.colours[pixel]), bedrock);
            assert_eq!(colour_of(&roof, roof.colours[pixel]), bedrock);
        }
    }

    #[test]
    fn unfinished_and_unsupported_chunks_are_missing() {
        let unfinished = expected()["overworld"]["unfinished"].as_u64().unwrap() as usize;
        let overworld = decoded(OVERWORLD, None);
        let legacy = decoded(LEGACY, None);
        for i in 0..256 {
            let pixel = render::pixel(unfinished, i);
            assert_eq!((overworld.heights[pixel], overworld.colours[pixel]), (MISSING, 0));
            // slot 2 of the legacy fixture was saved before 1.13
            let pixel = render::pixel(2, i);
            assert_eq!((legacy.heights[pixel], legacy.colours[pixel]), (MISSING, 0));
        }
        let stats = region(LEGACY, None).unwrap().stats;
        assert_eq!((stats.rendered, stats.unsupported), (2, 1));
        assert_eq!(region(OVERWORLD, None).unwrap().stats.unfinished, 1);
    }

    #[test]
    fn bodies_stay_small() {
        for (data, ceiling) in [(OVERWORLD, None), (NETHER, Some(nether_cap())), (LEGACY, None), (V26, None)] {
            let body = region(data, ceiling).unwrap().bytes;
            let palette = u16::from_le_bytes([body[8], body[9]]) as usize;
            assert_eq!(&body[..4], MAGIC.as_slice());
            assert_eq!(body.len(), 10 + palette * 3 + SIZE * SIZE * 7);
            let compressed = encode(&body).unwrap();
            assert!(compressed.len() < 250_000, "{} bytes", compressed.len());
        }
        // an empty region file still makes a valid, all-missing surface
        let empty = decoded(&[], None);
        assert!(empty.heights.iter().all(|&height| height == MISSING));
        assert_eq!(empty.palette, vec![[0, 0, 0]]);
    }

    #[test]
    fn inspects_single_columns() {
        let expected = expected();
        // dry land: slot 52 is chunk (20, 1), blocks 320..336, 16..32
        assert_eq!(
            inspect(OVERWORLD, 320, 16, None).unwrap(),
            Some(BlockInfo {
                y: expected["overworld"]["chunks"]["52"][0][0][1].as_i64().unwrap() as i32,
                block: "minecraft:sand".into(),
                biome: Some("minecraft:desert".into()),
                water_depth: 0,
                floor_block: None,
            })
        );
        // under water: 1.16 chunks keep their biomes outside the sections, which are not read
        assert_eq!(
            inspect(LEGACY, 5, 5, None).unwrap(),
            Some(BlockInfo {
                y: 60,
                block: "minecraft:water".into(),
                biome: None,
                water_depth: 1,
                floor_block: Some("minecraft:sand".into()),
            })
        );
        // the Nether below its roof: slot 65 is chunk (1, 2), blocks 16..32, 32..48
        let reference = &expected["nether"]["chunks"]["65"][0][0];
        let below = inspect(NETHER, 16, 32, Some(nether_cap())).unwrap().unwrap();
        assert_eq!(below.block, reference[0].as_str().unwrap());
        assert_eq!(i64::from(below.y), reference[1].as_i64().unwrap());
        let roof = inspect(NETHER, 16, 32, None).unwrap().unwrap();
        assert_eq!(roof.block, "minecraft:bedrock");
        // missing, unfinished and pre-1.13 chunks
        assert_eq!(inspect(OVERWORLD, 0, 0, None).unwrap(), None);
        let unfinished = expected["overworld"]["unfinished"].as_u64().unwrap() as usize;
        let (x, z) = ((unfinished % 32) * 16, (unfinished / 32) * 16);
        assert_eq!(inspect(OVERWORLD, x, z, None).unwrap(), None);
        assert_eq!(inspect(LEGACY, 32, 0, None).unwrap(), None);
    }
}
