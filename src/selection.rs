//! MCA Selector's selection model (`selection/Selection.java`): per region either the whole region
//! or a set of its 1024 chunks, plus `inverted` ("everything except"). Inverted selections are
//! resolved within the regions that exist (region ∪ entities ∪ poi files), as
//! `Selection.getTrueSelection` does. Also the JSON wire format shared with the frontend (the
//! frontend reads and writes MCA Selector's CSV files).
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most regions one selection may name.
pub const MAX_REGIONS: usize = 1_000_000;

/// Chunks of one region: bit `i = (x & 31) + (z & 31) * 32`, bit `i & 63` of word `i >> 6`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ChunkSet(pub [u64; 16]);

impl ChunkSet {
    /// Every chunk of the region.
    pub fn full() -> Self {
        Self([u64::MAX; 16])
    }

    pub fn get(&self, i: usize) -> bool {
        i < 1024 && self.0[i >> 6] & (1 << (i & 63)) != 0
    }

    /// Ignores indexes beyond 1023.
    pub fn set(&mut self, i: usize) {
        if i < 1024 {
            self.0[i >> 6] |= 1 << (i & 63);
        }
    }

    pub fn clear(&mut self, i: usize) {
        if i < 1024 {
            self.0[i >> 6] &= !(1 << (i & 63));
        }
    }

    pub fn count(&self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&word| word == 0)
    }

    pub fn is_full(&self) -> bool {
        self.0.iter().all(|&word| word == u64::MAX)
    }

    /// The set chunk indexes, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..1024).filter(move |&i| self.get(i))
    }

    /// The chunks not in this set (MCA Selector's `invertChunks`).
    pub fn invert(&self) -> Self {
        let mut words = self.0;
        for word in &mut words {
            *word = !*word;
        }
        Self(words)
    }

    /// Union with `other`.
    pub fn or(&mut self, other: &Self) {
        for (word, theirs) in self.0.iter_mut().zip(other.0) {
            *word |= theirs;
        }
    }

    /// The 128 wire bytes: bit `i` in byte `i >> 3`, mask `1 << (i & 7)`.
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut bytes = [0u8; 128];
        for (chunk, word) in bytes.chunks_exact_mut(8).zip(self.0) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    /// Parses the 128 wire bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 128 {
            return Err(format!("a chunk set has 128 bytes, not {}", bytes.len()));
        }
        let mut words = [0u64; 16];
        for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(8)) {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(chunk);
            *word = u64::from_le_bytes(raw);
        }
        Ok(Self(words))
    }
}

/// The chunk index of absolute chunk coordinates within their region.
pub fn chunk_index(chunk_x: i32, chunk_z: i32) -> usize {
    ((chunk_x & 31) + (chunk_z & 31) * 32) as usize
}

/// The region of absolute chunk coordinates.
pub fn chunk_region(chunk_x: i32, chunk_z: i32) -> (i32, i32) {
    (chunk_x >> 5, chunk_z >> 5)
}

/// Absolute chunk coordinates of chunk index `i` of region `(rx, rz)`.
pub fn chunk_at(region: (i32, i32), i: usize) -> (i32, i32) {
    (
        region.0.wrapping_mul(32).wrapping_add((i & 31) as i32),
        region.1.wrapping_mul(32).wrapping_add((i >> 5) as i32),
    )
}

/// `regions`: `None` = the whole region, `Some(set)` = those chunks (never empty, never full once
/// normalized). With `inverted`, every chunk is selected except the listed ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub inverted: bool,
    pub regions: BTreeMap<(i32, i32), Option<ChunkSet>>,
}

/// Wire format (JSON), shared with the frontend:
/// `{"inverted": false, "regions": [{"x": 0, "z": -1, "chunks": null}, {"x": 1, "z": 0, "chunks": "<base64>"}]}`.
/// `chunks`: null = whole region; else standard base64 (with padding) of 128 bytes, bit `i` in
/// byte `i >> 3`, mask `1 << (i & 7)`, `i = (x & 31) + (z & 31) * 32`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireSelection {
    pub inverted: bool,
    pub regions: Vec<WireRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireRegion {
    pub x: i32,
    pub z: i32,
    pub chunks: Option<String>,
}

impl Selection {
    /// Parses the wire format: refuses more than [`MAX_REGIONS`] regions and chunk sets that are
    /// not base64 of 128 bytes. Repeated regions are merged; empty sets are dropped and full ones
    /// become whole regions.
    pub fn from_wire(wire: &WireSelection) -> Result<Self, String> {
        if wire.regions.len() > MAX_REGIONS {
            return Err(format!(
                "a selection may name at most {MAX_REGIONS} regions"
            ));
        }
        let mut selection = Self {
            inverted: wire.inverted,
            regions: BTreeMap::new(),
        };
        for region in &wire.regions {
            let chunks = match &region.chunks {
                None => None,
                Some(text) => {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map_err(|err| {
                            format!("region {} {}: invalid chunk set: {err}", region.x, region.z)
                        })?;
                    let set = ChunkSet::from_bytes(&bytes)
                        .map_err(|err| format!("region {} {}: {err}", region.x, region.z))?;
                    if set.is_empty() {
                        continue;
                    }
                    Some(set)
                }
            };
            selection.merge_region((region.x, region.z), chunks);
        }
        Ok(selection)
    }

    /// Unions `chunks` into the stored entry of `region` (raw entries, regardless of `inverted`).
    fn merge_region(&mut self, region: (i32, i32), chunks: Option<ChunkSet>) {
        let merged = match (self.regions.get(&region), chunks) {
            (Some(Some(existing)), Some(new)) => {
                let mut union = *existing;
                union.or(&new);
                Some(union)
            }
            (Some(None), _) | (_, None) => None,
            (None, Some(new)) => Some(new),
        };
        self.regions
            .insert(region, merged.filter(|set| !set.is_full()));
    }

    /// The wire format: empty chunk sets dropped, full sets sent as whole regions.
    pub fn to_wire(&self) -> WireSelection {
        WireSelection {
            inverted: self.inverted,
            regions: self
                .regions
                .iter()
                .filter(|(_, chunks)| !chunks.is_some_and(|set| set.is_empty()))
                .map(|(&(x, z), chunks)| WireRegion {
                    x,
                    z,
                    chunks: chunks.filter(|set| !set.is_full()).map(|set| {
                        base64::engine::general_purpose::STANDARD.encode(set.to_bytes())
                    }),
                })
                .collect(),
        }
    }

    /// MCA Selector's `isChunkSelected`.
    pub fn is_selected(&self, chunk_x: i32, chunk_z: i32) -> bool {
        match self.regions.get(&chunk_region(chunk_x, chunk_z)) {
            Some(chunks) => {
                chunks.is_none_or(|set| set.get(chunk_index(chunk_x, chunk_z))) != self.inverted
            }
            None => self.inverted,
        }
    }

    /// The chunks to act on (MCA Selector's `getTrueSelection`): a non-inverted selection as is,
    /// an inverted one within the `existing` regions (every region absent from the selection
    /// whole, partially listed regions with their chunks inverted). Empty sets are left out.
    pub fn resolve(&self, existing: &BTreeSet<(i32, i32)>) -> BTreeMap<(i32, i32), ChunkSet> {
        if !self.inverted {
            return self
                .regions
                .iter()
                .map(|(&region, chunks)| (region, chunks.unwrap_or_else(ChunkSet::full)))
                .filter(|(_, set)| !set.is_empty())
                .collect();
        }
        existing
            .iter()
            .filter_map(|&region| match self.regions.get(&region) {
                None => Some((region, ChunkSet::full())),
                Some(Some(chunks)) => Some((region, chunks.invert())),
                Some(None) => None,
            })
            .filter(|(_, set)| !set.is_empty())
            .collect()
    }

    /// Selects one chunk (MCA Selector's `addChunk`, both the plain and the inverted branch).
    pub fn add_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        let region = chunk_region(chunk_x, chunk_z);
        let index = chunk_index(chunk_x, chunk_z);
        if self.inverted {
            if let Some(entry) = self.regions.get_mut(&region) {
                let mut set = entry.unwrap_or_else(ChunkSet::full);
                set.clear(index);
                if set.is_empty() {
                    self.regions.remove(&region);
                } else {
                    *entry = Some(set);
                }
            }
        } else {
            match self.regions.get_mut(&region) {
                Some(Some(set)) => {
                    set.set(index);
                    if set.is_full() {
                        self.regions.insert(region, None);
                    }
                }
                Some(None) => {}
                None => {
                    let mut set = ChunkSet::default();
                    set.set(index);
                    self.regions.insert(region, Some(set));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_of(indexes: &[usize]) -> ChunkSet {
        let mut set = ChunkSet::default();
        for &i in indexes {
            set.set(i);
        }
        set
    }

    #[test]
    fn chunk_sets_count_invert_and_iterate() {
        let set = set_of(&[0, 63, 64, 1023]);
        assert_eq!(set.count(), 4);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 63, 64, 1023]);
        assert_eq!(set.invert().count(), 1020);
        assert!(!set.invert().get(63));
        assert!(ChunkSet::full().is_full());
        assert!(ChunkSet::default().is_empty());
        let mut cleared = set;
        cleared.clear(63);
        assert!(!cleared.get(63));
        assert!(!set.get(5000));
    }

    #[test]
    fn wire_bytes_put_bit_i_in_byte_i_shr_3() {
        for i in [0usize, 1, 7, 8, 9, 63, 64, 100, 513, 1023] {
            let bytes = set_of(&[i]).to_bytes();
            for (byte_index, &byte) in bytes.iter().enumerate() {
                let expected = if byte_index == i >> 3 {
                    1u8 << (i & 7)
                } else {
                    0
                };
                assert_eq!(byte, expected, "bit {i}, byte {byte_index}");
            }
            assert_eq!(ChunkSet::from_bytes(&bytes).unwrap(), set_of(&[i]));
        }
        // chunk (x 3, z 1) of a region: index 35 = byte 4, mask 1 << 3
        let wire = Selection {
            inverted: false,
            regions: BTreeMap::from([((0, 0), Some(set_of(&[chunk_index(3, 1)])))]),
        }
        .to_wire();
        let encoded = wire.regions[0].chunks.as_deref().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(bytes.len(), 128);
        assert_eq!(bytes[4], 1 << 3);
        assert!(encoded.ends_with('='));
    }

    #[test]
    fn wire_round_trip_normalizes_sets() {
        let selection = Selection {
            inverted: true,
            regions: BTreeMap::from([
                ((0, -1), None),
                ((1, 0), Some(set_of(&[0, 5, 1000]))),
                ((-3, 7), Some(ChunkSet::full())),
                ((9, 9), Some(ChunkSet::default())),
            ]),
        };
        let wire = selection.to_wire();
        assert_eq!(wire.regions.len(), 3);
        assert!(wire.regions.iter().all(|r| (r.x, r.z) != (9, 9)));
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSelection = serde_json::from_str(&json).unwrap();
        let back = Selection::from_wire(&parsed).unwrap();
        assert!(back.inverted);
        assert_eq!(back.regions[&(0, -1)], None);
        assert_eq!(back.regions[&(-3, 7)], None);
        assert_eq!(back.regions[&(1, 0)], Some(set_of(&[0, 5, 1000])));
        assert!(!back.regions.contains_key(&(9, 9)));
        assert_eq!(back.to_wire(), wire);
    }

    #[test]
    fn bad_wire_sets_are_refused() {
        let wire = |chunks: &str| WireSelection {
            inverted: false,
            regions: vec![WireRegion {
                x: 0,
                z: 0,
                chunks: Some(chunks.into()),
            }],
        };
        assert!(Selection::from_wire(&wire("not base64!")).is_err());
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        assert!(Selection::from_wire(&wire(&short)).is_err());
    }

    #[test]
    fn selection_checks_chunks_like_mca_selector() {
        let mut selection = Selection::default();
        selection.add_chunk(-1, -1);
        selection.regions.insert((2, 0), None);
        assert!(selection.is_selected(-1, -1));
        assert!(!selection.is_selected(-2, -1));
        assert!(selection.is_selected(64, 31));
        assert_eq!(selection.regions[&(-1, -1)], Some(set_of(&[1023])));
        selection.inverted = true;
        assert!(!selection.is_selected(-1, -1));
        assert!(selection.is_selected(-2, -1));
        assert!(selection.is_selected(1000, 1000));
        assert!(!selection.is_selected(64, 31));
    }

    #[test]
    fn filling_a_region_makes_it_whole() {
        let mut selection = Selection::default();
        for z in 0..32 {
            for x in 0..32 {
                selection.add_chunk(x, z);
            }
        }
        assert_eq!(selection.regions[&(0, 0)], None);
    }

    #[test]
    fn inverted_selections_resolve_within_existing_regions() {
        let selection = Selection {
            inverted: true,
            regions: BTreeMap::from([((0, 0), None), ((1, 0), Some(set_of(&[0, 1])))]),
        };
        let existing = BTreeSet::from([(0, 0), (1, 0), (5, 5)]);
        let resolved = selection.resolve(&existing);
        assert_eq!(resolved.len(), 2);
        assert!(!resolved.contains_key(&(0, 0)));
        assert_eq!(resolved[&(1, 0)].count(), 1022);
        assert!(!resolved[&(1, 0)].get(0));
        assert!(resolved[&(1, 0)].get(2));
        assert_eq!(resolved[&(5, 5)], ChunkSet::full());

        let plain = Selection {
            inverted: false,
            regions: BTreeMap::from([((0, 0), None), ((9, 9), Some(set_of(&[3])))]),
        };
        let resolved = plain.resolve(&existing);
        assert_eq!(resolved[&(0, 0)], ChunkSet::full());
        assert_eq!(resolved[&(9, 9)], set_of(&[3]));
    }
}
