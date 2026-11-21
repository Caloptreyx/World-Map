//! Region files (`r.<x>.<z>.mca`): an 8 KiB header of 1024 chunk locations and timestamps,
//! then chunks in 4 KiB sectors, each a length, a compression type and the compressed NBT.
use crate::nbt::{self, Compound};
use std::collections::BTreeSet;

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

/// lz4-java's `LZ4BlockOutputStream` framing, which Minecraft uses for compression type 4:
/// blocks of `"LZ4Block"`, a method/level byte, compressed and original length and a checksum
/// (little endian), then the block; an empty block ends the stream.
fn lz4_blocks(mut data: &[u8]) -> Result<Vec<u8>, String> {
    const MAGIC: &[u8] = b"LZ4Block";
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
            0x10 => output.extend_from_slice(block),
            0x20 => output.extend(
                lz4_flex::block::decompress(block, original)
                    .map_err(|err| format!("broken LZ4 data ({err})"))?,
            ),
            other => return Err(format!("unknown LZ4 block method {other:#x}")),
        }
        data = rest;
    }
    Ok(output)
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

/// The region file without the chunks in the slots `remove`, its remaining chunks packed
/// after the header in their old order, byte for byte with their timestamps; and how many
/// chunks were removed. The game regenerates removed chunks when they load again.
pub fn without_chunks(data: &[u8], remove: &BTreeSet<usize>) -> Result<(Vec<u8>, usize), String> {
    let mut kept = slots(data)?;
    let before = kept.len();
    kept.retain(|slot| !remove.contains(&slot.index));
    let removed = before - kept.len();
    kept.sort_by_key(|slot| slot.sector);

    let mut output = vec![0u8; HEADER];
    for slot in kept {
        let sector = output.len() / SECTOR;
        let count = slot.count.max(1);
        let start = slot.sector * SECTOR;
        let end = (start + count * SECTOR).min(data.len());
        output.extend_from_slice(&data[start..end]);
        output.resize((sector + count) * SECTOR, 0);
        let location = ((sector as u32) << 8) | count as u32;
        output[slot.index * 4..slot.index * 4 + 4].copy_from_slice(&location.to_be_bytes());
        let stamp = SECTOR + slot.index * 4;
        let timestamp: [u8; 4] = data[stamp..stamp + 4].try_into().expect("4 bytes");
        output[stamp..stamp + 4].copy_from_slice(&timestamp);
    }
    Ok((output, removed))
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

    #[test]
    fn removes_chunks_and_keeps_the_rest_byte_for_byte() {
        let remove = BTreeSet::from([52, 85, 700]);
        let (rewritten, removed) = without_chunks(OVERWORLD, &remove).unwrap();
        assert_eq!(removed, 2);
        assert!(rewritten.len() < OVERWORLD.len());
        assert_eq!(rewritten.len() % SECTOR, 0);
        let left = slots(&rewritten).unwrap();
        assert_eq!(
            left.iter().map(|s| s.index).collect::<Vec<_>>(),
            vec![9, 53, 84]
        );
        for slot in left {
            let original = slots(OVERWORLD)
                .unwrap()
                .into_iter()
                .find(|s| s.index == slot.index)
                .unwrap();
            assert_eq!(
                payload(&rewritten, slot).unwrap(),
                payload(OVERWORLD, original).unwrap()
            );
            let stamp = SECTOR + slot.index * 4;
            assert_eq!(rewritten[stamp..stamp + 4], OVERWORLD[stamp..stamp + 4]);
        }
        // removed slots have neither a location nor a timestamp
        assert_eq!(rewritten[52 * 4..52 * 4 + 4], [0; 4]);
        assert_eq!(rewritten[SECTOR + 52 * 4..SECTOR + 52 * 4 + 4], [0; 4]);

        let (all_gone, removed) =
            without_chunks(OVERWORLD, &(0..CHUNKS).collect::<BTreeSet<_>>()).unwrap();
        assert_eq!(removed, 5);
        assert_eq!(all_gone.len(), HEADER);
    }
}
