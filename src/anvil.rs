//! Region files (`r.<x>.<z>.mca`): an 8 KiB header of 1024 chunk locations and timestamps,
//! then chunks in 4 KiB sectors, each a length, a compression type and the compressed NBT.
//! Chunks needing more than 255 sectors live in `c.<x>.<z>.mcc` files next to the region file,
//! with a stub in the region (length 1, compression type | 0x80).
use crate::nbt::{self, Compound};
use std::borrow::Cow;

pub const SECTOR: usize = 4096;
pub const HEADER: usize = 2 * SECTOR;
pub const CHUNKS: usize = 1024;

/// Where a chunk is stored: its slot index (`x + z * 32` within the region) and its sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub index: usize,
    pub sector: usize,
    pub count: usize,
}

/// The occupied slots of a region file, in slot order. An empty file has none (the game
/// creates them); anything else shorter than the header is not a region file.
pub fn slots(data: &[u8]) -> Result<Vec<Slot>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.len() < HEADER {
        return Err("not a region file (shorter than its header)".into());
    }
    Ok((0..CHUNKS)
        .filter_map(|index| {
            let location = u32::from_be_bytes(data[index * 4..index * 4 + 4].try_into().ok()?);
            let sector = (location >> 8) as usize;
            let count = (location & 0xFF) as usize;
            // slots pointing into the header or past the end are unreadable, as for the game
            (location != 0 && sector >= 2 && sector * SECTOR < data.len()).then_some(Slot {
                index,
                sector,
                count,
            })
        })
        .collect())
}

/// The compression type and compressed bytes of a chunk; `None` when the chunk is stored in
/// an external `c.<x>.<z>.mcc` file (more than 1 MiB compressed).
pub fn payload(data: &[u8], slot: Slot) -> Result<Option<(u8, &[u8])>, String> {
    let start = slot.sector * SECTOR;
    let head = data
        .get(start..start + 5)
        .ok_or("a chunk starts past the end of the file")?;
    let length = u32::from_be_bytes(head[..4].try_into().expect("4 bytes")) as usize;
    let compression = head[4];
    if compression & 0x80 != 0 {
        return Ok(None);
    }
    if length == 0 {
        return Err("a chunk has no data".into());
    }
    let body = data
        .get(start + 5..start + 4 + length)
        .ok_or("a chunk is longer than the file")?;
    Ok(Some((compression, body)))
}

const LZ4_MAGIC: &[u8] = b"LZ4Block";
/// lz4-java's block size (`1 << 16`) and the level bits it stores for it in the token byte
/// (`blockSize == 1 << (COMPRESSION_LEVEL_BASE + level)`, base 10).
const LZ4_BLOCK: usize = 1 << 16;
const LZ4_LEVEL: u8 = 6;
const LZ4_RAW: u8 = 0x10;
const LZ4_COMPRESSED: u8 = 0x20;
/// lz4-java's default checksum: XXHash32 with this seed, through `asChecksum()`, which keeps
/// only the low 28 bits.
const LZ4_SEED: u32 = 0x9747_B28C;
const LZ4_CHECKSUM_MASK: u32 = 0x0FFF_FFFF;

/// lz4-java's `LZ4BlockOutputStream` framing, which Minecraft uses for compression type 4:
/// blocks of `"LZ4Block"`, a method/level byte, compressed and original length and a checksum
/// (little endian), then the block; an empty block ends the stream.
fn lz4_blocks(mut data: &[u8]) -> Result<Vec<u8>, String> {
    const MAGIC: &[u8] = LZ4_MAGIC;
    const HEAD: usize = MAGIC.len() + 1 + 12;
    let mut output = Vec::new();
    while !data.is_empty() {
        if data.len() < HEAD || &data[..MAGIC.len()] != MAGIC {
            return Err("broken LZ4 block header".into());
        }
        let method = data[MAGIC.len()] & 0xF0;
        let number = |at: usize| {
            u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes")) as usize
        };
        let compressed = number(MAGIC.len() + 1);
        let original = number(MAGIC.len() + 5);
        data = &data[HEAD..];
        if original == 0 {
            break;
        }
        if compressed > data.len() {
            return Err("an LZ4 block is longer than the chunk".into());
        }
        if output.len() as u64 + original as u64 > nbt::MAX_DECOMPRESSED {
            return Err("an LZ4 chunk is too large".into());
        }
        let (block, rest) = data.split_at(compressed);
        match method {
            LZ4_RAW => output.extend_from_slice(block),
            LZ4_COMPRESSED => output.extend(
                lz4_flex::block::decompress(block, original)
                    .map_err(|err| format!("broken LZ4 data ({err})"))?,
            ),
            other => return Err(format!("unknown LZ4 block method {other:#x}")),
        }
        data = rest;
    }
    Ok(output)
}

/// Encodes `data` as `LZ4BlockOutputStream` does with its defaults (64 KiB blocks, fast
/// compressor, XXHash32 checksums): blocks that do not shrink are stored raw.
fn lz4_java(data: &[u8]) -> Vec<u8> {
    fn header(output: &mut Vec<u8>, method: u8, compressed: usize, original: usize, check: u32) {
        output.extend_from_slice(LZ4_MAGIC);
        output.push(method | LZ4_LEVEL);
        output.extend((compressed as u32).to_le_bytes());
        output.extend((original as u32).to_le_bytes());
        output.extend(check.to_le_bytes());
    }
    let mut output = Vec::with_capacity(data.len() / 2 + 64);
    for block in data.chunks(LZ4_BLOCK) {
        let check = xxhash_rust::xxh32::xxh32(block, LZ4_SEED) & LZ4_CHECKSUM_MASK;
        let compressed = lz4_flex::block::compress(block);
        if compressed.len() >= block.len() {
            header(&mut output, LZ4_RAW, block.len(), block.len(), check);
            output.extend_from_slice(block);
        } else {
            header(&mut output, LZ4_COMPRESSED, compressed.len(), block.len(), check);
            output.extend_from_slice(&compressed);
        }
    }
    header(&mut output, LZ4_RAW, 0, 0, 0);
    output
}

/// Compresses NBT bytes with a region compression type (1 gzip, 2 zlib, 3 none, 4 = lz4-java
/// block stream with XXHash32 checksums, seed 0x9747b28c, masked & 0x0FFFFFFF as lz4-java does).
pub fn compress(compression: u8, nbt: &[u8]) -> Result<Vec<u8>, String> {
    match compression {
        1 => Ok(nbt::gzip(nbt)),
        2 => Ok(nbt::deflate(nbt)),
        3 => Ok(nbt.to_vec()),
        4 => Ok(lz4_java(nbt)),
        127 => Err("custom chunk compression is not supported".into()),
        other => Err(format!("unknown chunk compression {other}")),
    }
}

/// The file name of a chunk's external storage: `c.<x>.<z>.mcc` (absolute chunk coordinates).
pub fn mcc_name(chunk_x: i32, chunk_z: i32) -> String {
    format!("c.{chunk_x}.{chunk_z}.mcc")
}

/// The NBT bytes of a chunk.
pub fn decompress(compression: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    match compression {
        1 => nbt::gunzip(payload),
        2 => nbt::inflate(payload),
        3 => Ok(payload.to_vec()),
        4 => lz4_blocks(payload),
        127 => Err("custom chunk compression is not supported".into()),
        other => Err(format!("unknown chunk compression {other}")),
    }
}

/// The NBT of the chunk in `slot`; `None` when it is stored externally.
pub fn chunk(data: &[u8], slot: Slot) -> Result<Option<Compound>, String> {
    match payload(data, slot)? {
        None => Ok(None),
        Some((compression, body)) => nbt::read(&decompress(compression, body)?).map(Some),
    }
}

/// The largest compressed payload stored inline: 255 sectors minus the length and compression
/// bytes (`4096 * 255 - 5`). The sector count is one byte, so bigger chunks go to a `.mcc` file,
/// as the game's `RegionFile` and MCA Selector do.
const MAX_INLINE: usize = 255 * SECTOR - 5;
/// The first DataVersion (19w36a, 1.15) whose game reads `.mcc` files; MCA Selector refuses to
/// store older oversized chunks.
const EXTERNAL_DATA_VERSION: i64 = 2203;

/// A chunk as stored in a region file.
#[derive(Clone, Debug, PartialEq)]
pub enum ChunkData {
    /// Inline and unchanged: the compression type and the compressed bytes, written back byte for byte.
    Raw { compression: u8, payload: Vec<u8> },
    /// Stored in `c.<x>.<z>.mcc` and unchanged: written back as the same stub; the .mcc stays as is.
    External { compression: u8 },
    /// Changed or new: encoded with `compression` (1 gzip, 2 zlib, 3 none, 4 lz4) on write.
    Nbt { compression: u8, root: nbt::Compound },
}

impl ChunkData {
    /// The compression type (without the external flag).
    fn compression(&self) -> u8 {
        match *self {
            Self::Raw { compression, .. }
            | Self::External { compression }
            | Self::Nbt { compression, .. } => compression,
        }
    }
}

/// A chunk slot's contents: the header timestamp (seconds) and the data.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionChunk {
    pub timestamp: u32,
    pub data: ChunkData,
}

/// The 1024 slots of a region file (`index = (x & 31) + (z & 31) * 32`).
#[derive(Clone, Debug)]
pub struct Region {
    chunks: Vec<Option<RegionChunk>>,
    /// The sector each parsed chunk started at, so `write` keeps the file's chunk order (an
    /// unchanged compact file is written back byte for byte); `None` for chunks added later.
    sectors: Vec<Option<usize>>,
}

/// A serialized region.
pub struct Written {
    pub region: Vec<u8>,
    /// Slot index -> contents of the `c.<x>.<z>.mcc` file to write (compressed payload, no header).
    pub external: Vec<(usize, Vec<u8>)>,
}

impl Region {
    /// A region without chunks.
    pub fn empty() -> Self {
        Self {
            chunks: vec![None; CHUNKS],
            sectors: vec![None; CHUNKS],
        }
    }

    /// Parses a region file (empty data = empty region). Slots that cannot be read (pointing past the
    /// end, zero length, ...) are dropped, like the game does.
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut region = Self::empty();
        for slot in slots(data)? {
            let start = slot.sector * SECTOR;
            // the game reads the slot's sectors, fewer bytes when the file ends early
            let end = (start + slot.count * SECTOR).min(data.len());
            let Some(head) = data.get(start..start + 5).filter(|_| start + 5 <= end) else {
                continue;
            };
            let length = u32::from_be_bytes(head[..4].try_into().expect("4 bytes")) as usize;
            let compression = head[4];
            let chunk_data = if compression & 0x80 != 0 {
                if length == 0 {
                    continue;
                }
                ChunkData::External {
                    compression: compression & 0x7F,
                }
            } else {
                // the length counts the compression byte
                let size = length.saturating_sub(1);
                if size == 0 || start + 5 + size > end {
                    continue;
                }
                ChunkData::Raw {
                    compression,
                    payload: data[start + 5..start + 5 + size].to_vec(),
                }
            };
            let stamp = SECTOR + slot.index * 4;
            let timestamp = u32::from_be_bytes(data[stamp..stamp + 4].try_into().expect("4 bytes"));
            region.chunks[slot.index] = Some(RegionChunk {
                timestamp,
                data: chunk_data,
            });
            region.sectors[slot.index] = Some(slot.sector);
        }
        Ok(region)
    }

    pub fn get(&self, index: usize) -> Option<&RegionChunk> {
        self.chunks.get(index)?.as_ref()
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut RegionChunk> {
        self.chunks.get_mut(index)?.as_mut()
    }

    /// Puts a chunk into a slot (or clears it with `None`); the previous chunk. A chunk put into
    /// an occupied slot keeps that slot's place in the file. Panics when `index >= 1024`.
    pub fn set(&mut self, index: usize, chunk: Option<RegionChunk>) -> Option<RegionChunk> {
        if chunk.is_none() {
            self.sectors[index] = None;
        }
        std::mem::replace(&mut self.chunks[index], chunk)
    }

    /// The occupied slot indexes, ascending.
    pub fn occupied(&self) -> Vec<usize> {
        (0..CHUNKS)
            .filter(|&index| self.chunks[index].is_some())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(Option::is_none)
    }

    /// The compressed size in bytes of a stored chunk (payload length; 0 for External unless known).
    /// Changed chunks are compressed to measure them; `None` for an empty slot or when the chunk
    /// cannot be compressed (unknown compression type).
    pub fn stored_size(&self, index: usize) -> Option<usize> {
        match &self.get(index)?.data {
            ChunkData::Raw { payload, .. } => Some(payload.len()),
            ChunkData::External { .. } => Some(0),
            ChunkData::Nbt { compression, root } => {
                compress(*compression, &nbt::write(root)).ok().map(|payload| payload.len())
            }
        }
    }

    /// Decodes the chunk's NBT. `external` must hold the `.mcc` file's bytes for External chunks
    /// (Wings serves gzip files decompressed: for compression 1, bytes that do not start with the gzip
    /// magic are already raw NBT).
    pub fn decode(&self, index: usize, external: Option<&[u8]>) -> Result<Option<nbt::Compound>, String> {
        let Some(chunk) = self.get(index) else {
            return Ok(None);
        };
        let root = match &chunk.data {
            ChunkData::Raw {
                compression,
                payload,
            } => nbt::read(&decompress(*compression, payload)?)?,
            ChunkData::External { compression } => {
                let bytes = external.ok_or_else(|| {
                    format!("chunk {index} is stored in a .mcc file that was not read")
                })?;
                if *compression == 1 && !bytes.starts_with(&[0x1F, 0x8B]) {
                    nbt::read(bytes)?
                } else {
                    nbt::read(&decompress(*compression, bytes)?)?
                }
            }
            ChunkData::Nbt { root, .. } => root.clone(),
        };
        Ok(Some(root))
    }

    /// Replaces the chunk's data with changed NBT, keeping its timestamp and compression
    /// (External chunks keep their compression type). An empty slot gets a chunk with timestamp 0
    /// and zlib compression (2, what the game writes). Panics when `index >= 1024`.
    pub fn replace(&mut self, index: usize, root: nbt::Compound) {
        if let Some(chunk) = &mut self.chunks[index] {
            chunk.data = ChunkData::Nbt {
                compression: chunk.data.compression(),
                root,
            };
            return;
        }
        self.chunks[index] = Some(RegionChunk {
            timestamp: 0,
            data: ChunkData::Nbt {
                compression: 2,
                root,
            },
        });
    }

    /// Serializes: header (locations + timestamps), then the chunks packed after it, each padded to
    /// 4 KiB sectors. Chunks keep the order they had in the parsed file (by their old sector, which
    /// is slot order for files the game's region optimizer or MCA Selector wrote), so an unchanged
    /// compact file comes out byte for byte; added chunks follow in slot order. Chunks needing more
    /// than 255 sectors go to `.mcc` files as the game does (stub: length 1 + compression | 0x80);
    /// like MCA Selector, that is refused below DataVersion 2203 (the game could not read them).
    /// A region without chunks is the bare 8 KiB header.
    pub fn write(&self) -> Result<Written, String> {
        let mut order = self.occupied();
        order.sort_by_key(|&index| (self.sectors[index].unwrap_or(usize::MAX), index));

        let mut output = Written {
            region: vec![0u8; HEADER],
            external: Vec::new(),
        };
        for index in order {
            let Some(chunk) = &self.chunks[index] else {
                continue;
            };
            let (compression, payload): (u8, Cow<'_, [u8]>) = match &chunk.data {
                ChunkData::Raw {
                    compression,
                    payload,
                } => (*compression, Cow::Borrowed(payload.as_slice())),
                ChunkData::External { compression } => {
                    let flagged = compression | 0x80;
                    put_chunk(&mut output.region, index, chunk.timestamp, flagged, &[])?;
                    continue;
                }
                ChunkData::Nbt { compression, root } => (
                    *compression,
                    Cow::Owned(
                        compress(*compression, &nbt::write(root))
                            .map_err(|err| format!("chunk {index}: {err}"))?,
                    ),
                ),
            };
            if payload.len() <= MAX_INLINE {
                put_chunk(&mut output.region, index, chunk.timestamp, compression, &payload)?;
                continue;
            }
            let version = match &chunk.data {
                ChunkData::Nbt { root, .. } => root.i64("DataVersion"),
                _ => self
                    .decode(index, None)
                    .ok()
                    .flatten()
                    .and_then(|root| root.i64("DataVersion")),
            };
            if version.is_none_or(|version| version < EXTERNAL_DATA_VERSION) {
                return Err(format!(
                    "chunk {index} is oversized ({} bytes compressed) and can't be saved when DataVersion is below {EXTERNAL_DATA_VERSION}",
                    payload.len()
                ));
            }
            put_chunk(&mut output.region, index, chunk.timestamp, compression | 0x80, &[])?;
            output.external.push((index, payload.into_owned()));
        }
        Ok(output)
    }
}

/// Appends a chunk (length, compression byte, payload) padded to whole sectors and points the
/// slot's location and timestamp at it.
fn put_chunk(
    region: &mut Vec<u8>,
    index: usize,
    timestamp: u32,
    compression: u8,
    payload: &[u8],
) -> Result<(), String> {
    let sector = region.len() / SECTOR;
    let count = (5 + payload.len()).div_ceil(SECTOR);
    if sector + count >= 1 << 24 {
        return Err("the region file would be larger than its header can address".into());
    }
    region.extend(((payload.len() + 1) as u32).to_be_bytes());
    region.push(compression);
    region.extend_from_slice(payload);
    region.resize((sector + count) * SECTOR, 0);
    let location = ((sector as u32) << 8) | count as u32;
    region[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());
    let stamp = SECTOR + index * 4;
    region[stamp..stamp + 4].copy_from_slice(&timestamp.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const COMPRESSIONS: &[u8] = include_bytes!("../tests/fixtures/compressions.mca");

    #[test]
    fn lists_the_occupied_slots() {
        let found: Vec<usize> = slots(OVERWORLD).unwrap().iter().map(|s| s.index).collect();
        assert_eq!(found, vec![9, 52, 53, 84, 85]);
        assert!(slots(&[]).unwrap().is_empty());
        assert!(slots(&[0; 100]).is_err());
        assert!(slots(&[0; HEADER]).unwrap().is_empty());
    }

    #[test]
    fn reads_every_compression_to_the_same_chunk() {
        let chunks: Vec<Compound> = slots(COMPRESSIONS)
            .unwrap()
            .into_iter()
            .map(|slot| chunk(COMPRESSIONS, slot).unwrap().unwrap())
            .collect();
        assert_eq!(chunks.len(), 4);
        for other in &chunks[1..] {
            assert_eq!(other, &chunks[0]);
        }
        assert_eq!(chunks[0].i64("DataVersion"), Some(4189));
        assert!(decompress(9, b"x").is_err());
        assert!(decompress(4, b"LZ4Blo").is_err());
    }

    #[test]
    fn external_chunks_are_reported_not_read() {
        let mut data = vec![0u8; HEADER + SECTOR];
        data[0..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        data[HEADER..HEADER + 5].copy_from_slice(&[0, 0, 0, 1, 0x82]);
        let slot = slots(&data).unwrap()[0];
        assert_eq!(payload(&data, slot).unwrap(), None);
    }

    /// Checks a written region file: whole sectors, a valid header, every chunk inside the file,
    /// no two chunks sharing a sector, chunks packed without gaps and padded with zeros, and no
    /// timestamp without a chunk.
    fn check_header(data: &[u8]) -> Result<(), String> {
        if data.len() < HEADER || data.len() % SECTOR != 0 {
            return Err(format!("length {} is not whole sectors", data.len()));
        }
        let sectors = data.len() / SECTOR;
        let mut used = vec![false; sectors];
        for index in 0..CHUNKS {
            let location = u32::from_be_bytes(data[index * 4..index * 4 + 4].try_into().unwrap());
            let stamp = SECTOR + index * 4;
            if location == 0 {
                if data[stamp..stamp + 4] != [0; 4] {
                    return Err(format!("slot {index} has a timestamp but no chunk"));
                }
                continue;
            }
            let (sector, count) = ((location >> 8) as usize, (location & 0xFF) as usize);
            if sector < 2 || count == 0 || sector + count > sectors {
                return Err(format!("slot {index} points at sectors {sector}+{count} of {sectors}"));
            }
            for taken in &mut used[sector..sector + count] {
                if *taken {
                    return Err(format!("slot {index} overlaps another chunk"));
                }
                *taken = true;
            }
            let start = sector * SECTOR;
            let length = u32::from_be_bytes(data[start..start + 4].try_into().unwrap()) as usize;
            if length == 0 || (4 + length).div_ceil(SECTOR) != count {
                return Err(format!("slot {index}: length {length} does not fill {count} sectors"));
            }
            if data[start + 4 + length..start + count * SECTOR].iter().any(|&byte| byte != 0) {
                return Err(format!("slot {index} is not padded with zeros"));
            }
            if data[start + 4] & 0x80 != 0 && length != 1 {
                return Err(format!("slot {index}: an external stub with data"));
            }
        }
        if let Some(gap) = used[2..].iter().position(|taken| !taken) {
            return Err(format!("sector {} is unused", gap + 2));
        }
        Ok(())
    }

    /// Deterministic pseudo-random bytes (xorshift64).
    fn noise(length: usize, mut state: u64) -> Vec<u8> {
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    fn stored(data: &[u8], index: usize) -> (Option<(u8, Vec<u8>)>, [u8; 4]) {
        let slot = slots(data).unwrap().into_iter().find(|slot| slot.index == index).unwrap();
        let stamp = SECTOR + index * 4;
        (
            payload(data, slot).unwrap().map(|(compression, body)| (compression, body.to_vec())),
            data[stamp..stamp + 4].try_into().unwrap(),
        )
    }

    fn indexes(data: &[u8]) -> Vec<usize> {
        slots(data).unwrap().iter().map(|slot| slot.index).collect()
    }

    #[test]
    fn rewrites_unchanged_regions_byte_for_byte() {
        for (name, data) in [("overworld", OVERWORLD), ("compressions", COMPRESSIONS)] {
            let region = Region::parse(data).unwrap();
            assert_eq!(region.occupied(), indexes(data), "{name}");
            let written = region.write().unwrap();
            check_header(&written.region).unwrap();
            assert!(written.external.is_empty());
            assert_eq!(indexes(&written.region), indexes(data));
            for index in indexes(data) {
                assert_eq!(stored(&written.region, index), stored(data, index), "{name} {index}");
                assert_eq!(region.get(index).unwrap().timestamp, 1_700_000_000);
                assert_eq!(
                    region.stored_size(index),
                    stored(data, index).0.map(|(_, body)| body.len())
                );
            }
            // the fixtures are compact, so nothing moves at all
            assert_eq!(written.region, data, "{name}");
            let again = Region::parse(&written.region).unwrap().write().unwrap();
            assert_eq!(again.region, written.region);
        }
        // every compression decodes to the same chunk through the model too
        let region = Region::parse(COMPRESSIONS).unwrap();
        let first = region.decode(0, None).unwrap().unwrap();
        for index in 1..4 {
            assert_eq!(region.decode(index, None).unwrap().as_ref(), Some(&first));
        }
        assert_eq!(region.decode(500, None).unwrap(), None);
    }

    #[test]
    fn keeps_the_parsed_chunk_order() {
        // slot 1 stored after slot 3, with an unused sector in between
        let mut data = vec![0u8; HEADER + 3 * SECTOR];
        let mut place = |index: usize, sector: usize, body: &[u8]| {
            data[index * 4..index * 4 + 4].copy_from_slice(&(((sector as u32) << 8) | 1).to_be_bytes());
            data[SECTOR + index * 4..SECTOR + index * 4 + 4].copy_from_slice(&(index as u32).to_be_bytes());
            let start = sector * SECTOR;
            data[start..start + 4].copy_from_slice(&(body.len() as u32 + 1).to_be_bytes());
            data[start + 4] = 3;
            data[start + 5..start + 5 + body.len()].copy_from_slice(body);
        };
        place(3, 2, b"three");
        place(1, 4, b"one");
        let mut region = Region::parse(&data).unwrap();
        let written = region.write().unwrap().region;
        check_header(&written).unwrap();
        assert_eq!(written.len(), HEADER + 2 * SECTOR);
        assert_eq!(written[3 * 4..3 * 4 + 4], ((2u32 << 8) | 1).to_be_bytes());
        assert_eq!(written[4..8], ((3u32 << 8) | 1).to_be_bytes());
        assert_eq!(stored(&written, 1), (Some((3, b"one".to_vec())), 1u32.to_be_bytes()));
        assert_eq!(stored(&written, 3), (Some((3, b"three".to_vec())), 3u32.to_be_bytes()));
        // an added chunk goes last
        region.set(0, Some(RegionChunk { timestamp: 9, data: ChunkData::Raw { compression: 3, payload: b"zero".to_vec() } }));
        let written = region.write().unwrap().region;
        check_header(&written).unwrap();
        assert_eq!(written[0..4], ((4u32 << 8) | 1).to_be_bytes());
    }

    #[test]
    fn drops_unreadable_slots() {
        let mut data = vec![0u8; HEADER + 2 * SECTOR];
        let location = |sector: u32, count: u32| ((sector << 8) | count).to_be_bytes();
        data[0..4].copy_from_slice(&location(2, 1)); // valid
        data[HEADER..HEADER + 6].copy_from_slice(&[0, 0, 0, 2, 3, 7]);
        data[4..8].copy_from_slice(&location(3, 1)); // zero length
        data[8..12].copy_from_slice(&location(9, 1)); // past the end
        data[12..16].copy_from_slice(&location(2, 0)); // no sectors
        data[16..20].copy_from_slice(&location(1, 1)); // inside the header
        data[HEADER + SECTOR..HEADER + SECTOR + 4].copy_from_slice(&0u32.to_be_bytes());
        let region = Region::parse(&data).unwrap();
        assert_eq!(region.occupied(), vec![0]);
        assert_eq!(
            region.get(0).unwrap().data,
            ChunkData::Raw { compression: 3, payload: vec![7] }
        );
        // a length running past the slot's sectors
        data[HEADER..HEADER + 4].copy_from_slice(&5000u32.to_be_bytes());
        assert!(Region::parse(&data).unwrap().is_empty());
        assert!(Region::parse(&[0; 100]).is_err());
    }

    #[test]
    fn replacing_a_chunk_keeps_the_others_byte_for_byte() {
        let mut region = Region::parse(OVERWORLD).unwrap();
        let mut root = region.decode(52, None).unwrap().unwrap();
        root.insert("InhabitedTime", nbt::Tag::Long(123_456));
        region.replace(52, root.clone());
        assert_eq!(region.get(52).unwrap().timestamp, 1_700_000_000);
        let written = region.write().unwrap();
        check_header(&written.region).unwrap();
        assert_eq!(indexes(&written.region), indexes(OVERWORLD));
        for index in [9, 53, 84, 85] {
            assert_eq!(stored(&written.region, index), stored(OVERWORLD, index));
        }
        let (changed, stamp) = stored(&written.region, 52);
        assert_eq!(changed.as_ref().map(|(compression, _)| *compression), Some(2));
        assert_eq!(stamp, stored(OVERWORLD, 52).1);
        let reparsed = Region::parse(&written.region).unwrap();
        assert_eq!(reparsed.decode(52, None).unwrap(), Some(root.clone()));
        assert_eq!(reparsed.decode(53, None).unwrap(), region.decode(53, None).unwrap());
        assert_eq!(
            region.stored_size(52),
            changed.map(|(_, body)| body.len())
        );

        // a chunk put into an empty slot gets zlib and timestamp 0
        let mut empty = Region::empty();
        empty.replace(7, root.clone());
        assert_eq!(
            empty.get(7),
            Some(&RegionChunk { timestamp: 0, data: ChunkData::Nbt { compression: 2, root } })
        );
        empty.get_mut(7).unwrap().timestamp = 5;
        check_header(&empty.write().unwrap().region).unwrap();
        assert_eq!(empty.set(7, None).map(|chunk| chunk.timestamp), Some(5));
    }

    #[test]
    fn removes_chunks_and_keeps_the_rest_byte_for_byte() {
        let mut region = Region::parse(OVERWORLD).unwrap();
        for index in [52, 85, 700] {
            region.set(index, None);
        }
        let rewritten = region.write().unwrap().region;
        check_header(&rewritten).unwrap();
        assert!(rewritten.len() < OVERWORLD.len());
        assert_eq!(indexes(&rewritten), vec![9, 53, 84]);
        for index in [9, 53, 84] {
            assert_eq!(stored(&rewritten, index), stored(OVERWORLD, index));
        }
        // removed slots have neither a location nor a timestamp
        assert_eq!(rewritten[52 * 4..52 * 4 + 4], [0; 4]);
        assert_eq!(rewritten[SECTOR + 52 * 4..SECTOR + 52 * 4 + 4], [0; 4]);

        for index in region.occupied() {
            region.set(index, None);
        }
        assert!(region.is_empty());
        let all_gone = region.write().unwrap();
        assert_eq!(all_gone.region, vec![0u8; HEADER]);
        assert!(all_gone.external.is_empty());
    }

    #[test]
    fn empty_regions() {
        assert!(Region::empty().is_empty());
        assert!(Region::parse(&[]).unwrap().is_empty());
        assert!(Region::parse(&[0; HEADER]).unwrap().is_empty());
        assert_eq!(Region::empty().write().unwrap().region.len(), HEADER);
        assert_eq!(Region::empty().occupied(), Vec::<usize>::new());
        assert_eq!(Region::empty().get(3), None);
        assert_eq!(Region::empty().get(5000), None);
        assert_eq!(Region::empty().stored_size(3), None);
        assert!(!Region::parse(OVERWORLD).unwrap().is_empty());
    }

    #[test]
    fn compresses_with_every_type() {
        let slot = slots(OVERWORLD).unwrap()[1];
        let chunk_nbt = {
            let (compression, body) = payload(OVERWORLD, slot).unwrap().unwrap();
            decompress(compression, body).unwrap()
        };
        // more than 64 KiB, partly incompressible, so LZ4 writes compressed and raw blocks
        let mut data = chunk_nbt.repeat(200_000 / chunk_nbt.len() + 1);
        data.extend(noise(70_000, 0x1234_5678_9ABC_DEF0));
        for compression in 1..=4 {
            let packed = compress(compression, &data).unwrap();
            assert_eq!(decompress(compression, &packed).unwrap(), data, "type {compression}");
        }
        assert_eq!(compress(3, &data).unwrap(), data);
        assert!(compress(9, &data).is_err());
        assert!(compress(127, &data).is_err());

        // lz4-java framing: header fields, checksums of the original bytes, the empty end block
        let packed = compress(4, &data).unwrap();
        let mut rest = &packed[..];
        let mut original_bytes = &data[..];
        let mut methods = Vec::new();
        loop {
            assert_eq!(&rest[..8], b"LZ4Block");
            let token = rest[8];
            let number = |at: usize| u32::from_le_bytes(rest[at..at + 4].try_into().unwrap());
            let (compressed, original, check) = (number(9) as usize, number(13) as usize, number(17));
            assert_eq!(token & 0x0F, 6);
            rest = &rest[21..];
            if original == 0 {
                assert_eq!((token, compressed, check), (0x16, 0, 0));
                break;
            }
            assert!(original <= 1 << 16);
            let (block, tail) = original_bytes.split_at(original);
            assert_eq!(check, xxhash_rust::xxh32::xxh32(block, 0x9747_B28C) & 0x0FFF_FFFF);
            methods.push(token & 0xF0);
            rest = &rest[compressed..];
            original_bytes = tail;
        }
        assert!(rest.is_empty() && original_bytes.is_empty());
        assert_eq!(methods.len(), data.len().div_ceil(1 << 16));
        assert!(methods.contains(&0x20) && methods.contains(&0x10));

        let empty = compress(4, &[]).unwrap();
        assert_eq!(empty.len(), 21);
        assert!(decompress(4, &empty).unwrap().is_empty());
    }

    #[test]
    fn oversized_chunks_go_to_mcc_files() {
        let noise_tag =
            nbt::Tag::ByteArray(noise(1_100_000, 42).into_iter().map(|b| b as i8).collect());
        let root: Compound = [
            ("DataVersion".to_string(), nbt::Tag::Int(4189)),
            ("xPos".to_string(), nbt::Tag::Int(5)),
            ("Noise".to_string(), noise_tag),
        ]
        .into_iter()
        .collect();
        let mut region = Region::empty();
        region.set(5, Some(RegionChunk { timestamp: 42, data: ChunkData::Nbt { compression: 3, root: root.clone() } }));
        let small: Compound = [("DataVersion".to_string(), nbt::Tag::Int(4189))].into_iter().collect();
        region.replace(6, small.clone());
        let written = region.write().unwrap();
        check_header(&written.region).unwrap();
        assert_eq!(written.external.len(), 1);
        let (index, mcc) = &written.external[0];
        assert_eq!(*index, 5);
        assert_eq!(mcc, &nbt::write(&root));
        // the stub: one sector, length 1, compression | 0x80
        assert_eq!(written.region[5 * 4..5 * 4 + 4], ((2u32 << 8) | 1).to_be_bytes());
        assert_eq!(written.region[HEADER..HEADER + 5], [0, 0, 0, 1, 0x83]);
        assert_eq!(written.region[SECTOR + 5 * 4..SECTOR + 5 * 4 + 4], 42u32.to_be_bytes());

        let parsed = Region::parse(&written.region).unwrap();
        assert_eq!(
            parsed.get(5),
            Some(&RegionChunk { timestamp: 42, data: ChunkData::External { compression: 3 } })
        );
        assert_eq!(parsed.stored_size(5), Some(0));
        assert_eq!(parsed.decode(5, Some(mcc.as_slice())).unwrap(), Some(root.clone()));
        assert!(parsed.decode(5, None).is_err());
        assert_eq!(parsed.decode(6, None).unwrap(), Some(small));
        // unchanged external chunks keep their stub and their .mcc file is not rewritten
        let again = parsed.write().unwrap();
        assert_eq!(again.region, written.region);
        assert!(again.external.is_empty());

        // the game before 1.15 cannot read .mcc files
        let mut old = root;
        old.insert("DataVersion", nbt::Tag::Int(1976));
        region.replace(5, old);
        assert!(region.write().is_err());
    }

    #[test]
    fn decodes_external_chunks_of_every_compression() {
        let root: Compound = [
            ("DataVersion".to_string(), nbt::Tag::Int(4189)),
            ("Status".to_string(), nbt::Tag::String("minecraft:full".into())),
        ]
        .into_iter()
        .collect();
        let bytes = nbt::write(&root);
        let mut region = Region::empty();
        for compression in 1..=4 {
            region.set(
                compression as usize,
                Some(RegionChunk { timestamp: 1, data: ChunkData::External { compression } }),
            );
            let mcc = compress(compression, &bytes).unwrap();
            let decoded = region.decode(compression as usize, Some(mcc.as_slice())).unwrap();
            assert_eq!(decoded.as_ref(), Some(&root));
        }
        // Wings serves gzip files decompressed
        assert_eq!(region.decode(1, Some(bytes.as_slice())).unwrap(), Some(root.clone()));
        assert!(region.decode(2, Some(bytes.as_slice())).is_err());
        // replacing keeps the external chunk's compression
        region.replace(4, root.clone());
        assert_eq!(
            region.get(4).unwrap().data,
            ChunkData::Nbt { compression: 4, root }
        );
        check_header(&region.write().unwrap().region).unwrap();
        assert_eq!(mcc_name(-33, 7), "c.-33.7.mcc");
    }
}
