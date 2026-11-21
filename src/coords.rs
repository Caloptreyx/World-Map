//! Block, chunk and region coordinates: a chunk is 16 × 16 blocks, a region 32 × 32 chunks
//! stored in `r.<x>.<z>.mca` with the chunk in slot `(x & 31) + (z & 31) * 32`.
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// The chunk holding a block coordinate (floored, so -1 is in chunk -1).
pub fn chunk_of_block(block: f64) -> i32 {
    (block.floor() as i64 >> 4) as i32
}

pub fn region_of_chunk(chunk: i32) -> i32 {
    chunk >> 5
}

/// The slot of a chunk within its region file.
pub fn slot(chunk_x: i32, chunk_z: i32) -> usize {
    ((chunk_x & 31) + (chunk_z & 31) * 32) as usize
}

/// `(x, z)` of a region file name like `r.-1.2.mca`.
pub fn region_file(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (x, z) = rest.split_once('.')?;
    Some((x.parse().ok()?, z.parse().ok()?))
}

pub fn region_file_name(x: i32, z: i32) -> String {
    format!("r.{x}.{z}.mca")
}

/// An inclusive rectangle of chunks, corners in any order.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct ChunkRect {
    pub x1: i32,
    pub z1: i32,
    pub x2: i32,
    pub z2: i32,
}

impl ChunkRect {
    fn bounds(&self) -> (i32, i32, i32, i32) {
        (
            self.x1.min(self.x2),
            self.z1.min(self.z2),
            self.x1.max(self.x2),
            self.z1.max(self.z2),
        )
    }

    pub fn area(&self) -> u64 {
        let (x1, z1, x2, z2) = self.bounds();
        (x2 as i64 - x1 as i64 + 1) as u64 * (z2 as i64 - z1 as i64 + 1) as u64
    }
}

/// The chunks of a selection (rectangles may overlap) grouped by region: region `(x, z)` to the
/// slots to clear. `None` when the selection covers more than `limit` chunks.
pub fn by_region(rects: &[ChunkRect], limit: u64) -> Option<BTreeMap<(i32, i32), BTreeSet<usize>>> {
    let total: u64 = rects.iter().map(ChunkRect::area).sum();
    if total > limit {
        return None;
    }
    let mut regions: BTreeMap<(i32, i32), BTreeSet<usize>> = BTreeMap::new();
    for rect in rects {
        let (x1, z1, x2, z2) = rect.bounds();
        for z in z1..=z2 {
            for x in x1..=x2 {
                regions
                    .entry((region_of_chunk(x), region_of_chunk(z)))
                    .or_default()
                    .insert(slot(x, z));
            }
        }
    }
    Some(regions)
}

/// The regions a block rectangle touches, or `None` when there are more than `limit`.
pub fn regions_in_blocks(
    (x1, z1): (f64, f64),
    (x2, z2): (f64, f64),
    limit: usize,
) -> Option<Vec<(i32, i32)>> {
    let rx = (
        region_of_chunk(chunk_of_block(x1.min(x2))),
        region_of_chunk(chunk_of_block(x1.max(x2))),
    );
    let rz = (
        region_of_chunk(chunk_of_block(z1.min(z2))),
        region_of_chunk(chunk_of_block(z1.max(z2))),
    );
    let count = (rx.1 as i64 - rx.0 as i64 + 1) * (rz.1 as i64 - rz.0 as i64 + 1);
    if count > limit as i64 {
        return None;
    }
    Some(
        (rz.0..=rz.1)
            .flat_map(|z| (rx.0..=rx.1).map(move |x| (x, z)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_chunks_and_regions_floor_towards_negative_infinity() {
        assert_eq!(chunk_of_block(0.0), 0);
        assert_eq!(chunk_of_block(15.9), 0);
        assert_eq!(chunk_of_block(16.0), 1);
        assert_eq!(chunk_of_block(-0.5), -1);
        assert_eq!(chunk_of_block(-16.0), -1);
        assert_eq!(chunk_of_block(-17.0), -2);
        assert_eq!(region_of_chunk(31), 0);
        assert_eq!(region_of_chunk(32), 1);
        assert_eq!(region_of_chunk(-1), -1);
        assert_eq!(region_of_chunk(-32), -1);
        assert_eq!(region_of_chunk(-33), -2);
    }

    #[test]
    fn slots_wrap_within_the_region() {
        assert_eq!(slot(0, 0), 0);
        assert_eq!(slot(31, 0), 31);
        assert_eq!(slot(0, 1), 32);
        assert_eq!(slot(-1, -1), 1023);
        assert_eq!(slot(-12, 1), 20 + 32);
        assert_eq!(slot(33, -33), 1 + 31 * 32);
    }

    #[test]
    fn region_file_names() {
        assert_eq!(region_file("r.-1.2.mca"), Some((-1, 2)));
        assert_eq!(region_file("r.0.0.mca"), Some((0, 0)));
        assert_eq!(region_file("r.0.0.mcc"), None);
        assert_eq!(region_file("c.0.0.mca"), None);
        assert_eq!(region_file("r.a.0.mca"), None);
        assert_eq!(region_file_name(-3, 7), "r.-3.7.mca");
    }

    #[test]
    fn selections_group_by_region_without_duplicates() {
        let rects = [
            ChunkRect {
                x1: 30,
                z1: 0,
                x2: 33,
                z2: 1,
            },
            // overlaps the first one, corners swapped
            ChunkRect {
                x1: 31,
                z1: 1,
                x2: 31,
                z2: 0,
            },
            ChunkRect {
                x1: -1,
                z1: -1,
                x2: -1,
                z2: -1,
            },
        ];
        let regions = by_region(&rects, 100).unwrap();
        assert_eq!(regions.len(), 3);
        assert_eq!(
            regions[&(0, 0)],
            BTreeSet::from([30, 31, 30 + 32, 31 + 32])
        );
        assert_eq!(regions[&(1, 0)], BTreeSet::from([0, 1, 32, 33]));
        assert_eq!(regions[&(-1, -1)], BTreeSet::from([1023]));
        assert_eq!(rects[0].area(), 8);
        assert!(by_region(&rects, 9).is_none());
    }

    #[test]
    fn regions_in_a_block_rectangle() {
        assert_eq!(
            regions_in_blocks((-1.0, 0.0), (511.0, 600.0), 9),
            Some(vec![(-1, 0), (0, 0), (-1, 1), (0, 1)])
        );
        assert_eq!(regions_in_blocks((0.0, 0.0), (5000.0, 5000.0), 9), None);
    }
}
