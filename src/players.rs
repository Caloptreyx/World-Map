//! Players' last known positions: `<uuid>.dat` files in the world's player folder (gzipped NBT
//! with `Pos`, `Rotation` and `Dimension`), named through `usercache.json`.
use crate::{
    nbt::{self, Compound, Tag},
    worlds,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Position {
    pub dimension: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Degrees, 0 facing south, growing clockwise.
    pub yaw: f32,
    /// Bukkit's UUID of the world the player is in (`WorldUUIDMost`/`WorldUUIDLeast`).
    #[serde(skip)]
    pub world: Option<uuid::Uuid>,
}

/// The UUID of a player file name (`<uuid>.dat`, not the `.dat_old` backups).
pub fn file_uuid(name: &str) -> Option<uuid::Uuid> {
    let stem = name.strip_suffix(".dat")?;
    (stem.len() == 36)
        .then(|| uuid::Uuid::parse_str(stem).ok())
        .flatten()
}

/// The dimension id of a player: a string since 1.16, a number before.
fn dimension(root: &Compound) -> String {
    match root.get("Dimension") {
        Some(Tag::String(id)) => id.clone(),
        Some(tag) => match tag.as_i64() {
            Some(-1) => worlds::NETHER.into(),
            Some(1) => worlds::END.into(),
            _ => worlds::OVERWORLD.into(),
        },
        None => worlds::OVERWORLD.into(),
    }
}

/// The position stored in a player file (raw or gzipped).
pub fn position(data: &[u8]) -> Result<Position, String> {
    let root = nbt::read(&nbt::decompress(data)?)?;
    let pos = root
        .numbers("Pos")
        .filter(|pos| pos.len() == 3 && pos.iter().all(|value| value.is_finite()))
        .ok_or("the player file has no position")?;
    let yaw = root
        .numbers("Rotation")
        .and_then(|rotation| rotation.first().copied())
        .filter(|yaw| yaw.is_finite())
        .unwrap_or(0.0) as f32;
    Ok(Position {
        dimension: dimension(&root),
        x: pos[0],
        y: pos[1],
        z: pos[2],
        yaw,
        world: match (root.i64("WorldUUIDMost"), root.i64("WorldUUIDLeast")) {
            (Some(most), Some(least)) => Some(uuid::Uuid::from_u64_pair(most as u64, least as u64)),
            _ => None,
        },
    })
}

#[derive(Deserialize)]
struct CacheEntry {
    name: String,
    uuid: String,
}

/// Names by UUID from `usercache.json`; empty when it does not parse.
pub fn usercache(data: &[u8]) -> HashMap<uuid::Uuid, String> {
    serde_json::from_slice::<Vec<CacheEntry>>(data)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| Some((uuid::Uuid::parse_str(&entry.uuid).ok()?, entry.name)))
        .collect()
}

/// Player names as Minecraft Java accepts them, plus Floodgate's `.` prefix for Bedrock
/// players; the only names commands are ever built from.
pub fn valid_name(name: &str) -> bool {
    let plain = name.strip_prefix('.').unwrap_or(name);
    (1..=16).contains(&plain.len())
        && plain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    fn player_file(dimension: Option<Tag>) -> Vec<u8> {
        // {Pos: [12.5d, 70d, -3.25d], Rotation: [90f, 0f], Dimension: ...}
        let mut data = vec![10, 0, 0];
        data.extend([9, 0, 3]);
        data.extend(b"Pos");
        data.extend([6, 0, 0, 0, 3]);
        for value in [12.5f64, 70.0, -3.25] {
            data.extend(value.to_be_bytes());
        }
        data.extend([9, 0, 8]);
        data.extend(b"Rotation");
        data.extend([5, 0, 0, 0, 2]);
        data.extend(90f32.to_be_bytes());
        data.extend(0f32.to_be_bytes());
        match dimension {
            Some(Tag::String(id)) => {
                data.extend([8, 0, 9]);
                data.extend(b"Dimension");
                data.extend((id.len() as u16).to_be_bytes());
                data.extend(id.as_bytes());
            }
            Some(Tag::Int(id)) => {
                data.extend([3, 0, 9]);
                data.extend(b"Dimension");
                data.extend(id.to_be_bytes());
            }
            _ => {}
        }
        data.push(0);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn reads_positions_of_every_era() {
        let modern = position(&player_file(Some(Tag::String(
            "minecraft:the_nether".into(),
        ))))
        .unwrap();
        assert_eq!(
            modern,
            Position {
                dimension: worlds::NETHER.into(),
                x: 12.5,
                y: 70.0,
                z: -3.25,
                yaw: 90.0,
                world: None,
            }
        );
        assert_eq!(
            position(&player_file(Some(Tag::Int(1)))).unwrap().dimension,
            worlds::END
        );
        assert_eq!(
            position(&player_file(None)).unwrap().dimension,
            worlds::OVERWORLD
        );
        assert!(position(b"not nbt").is_err());
    }

    #[test]
    fn player_files_and_names() {
        let id = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        assert_eq!(
            file_uuid(&format!("{id}.dat")),
            Some(uuid::Uuid::parse_str(id).unwrap())
        );
        assert_eq!(file_uuid(&format!("{id}.dat_old")), None);
        assert_eq!(file_uuid("level.dat"), None);

        let cache = usercache(
            format!(r#"[{{"name":"Notch","uuid":"{id}","expiresOn":"2030-01-01 00:00:00 +0000"}},{{"name":"x","uuid":"bad"}}]"#)
                .as_bytes(),
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[&uuid::Uuid::parse_str(id).unwrap()], "Notch");
        assert!(usercache(b"{}").is_empty());

        assert!(valid_name("Notch") && valid_name("jeb_") && valid_name(".BedrockGuy"));
        assert!(!valid_name("") && !valid_name("a b") && !valid_name("x;op me"));
        assert!(!valid_name("seventeen_chars_x") && !valid_name("@a"));
    }
}
