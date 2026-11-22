//! Players' last known positions: `<uuid>.dat` files in the world's player folder (gzipped NBT
//! with `Pos`, `Rotation` and `Dimension`), named through `usercache.json`, and which of them
//! the map shows.
use crate::{
    nbt::{self, Compound, Tag},
    online::Presence,
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

/// Which players the map shows on a server, set per server. Both by default, also for servers
/// that never saved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visibility {
    pub online: bool,
    pub offline: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Self {
            online: true,
            offline: true,
        }
    }
}

impl Visibility {
    pub fn shows(self, online: bool) -> bool {
        if online { self.online } else { self.offline }
    }
}

/// A player on the player layer: where their file last put them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Marker {
    pub uuid: uuid::Uuid,
    pub name: Option<String>,
    pub world: String,
    pub dimension: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    /// When the player file was saved, Unix seconds.
    pub last_seen: i64,
    pub online: bool,
}

/// The markers with who is online marked, without the players the server's setting hides
/// (they never leave the backend).
pub fn visible(markers: Vec<Marker>, presence: &Presence, visibility: Visibility) -> Vec<Marker> {
    markers
        .into_iter()
        .filter_map(|mut marker| {
            marker.online = presence.is_online(marker.uuid, marker.name.as_deref());
            visibility.shows(marker.online).then_some(marker)
        })
        .collect()
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

    fn marker(id: u128, name: Option<&str>) -> Marker {
        Marker {
            uuid: uuid::Uuid::from_u128(id),
            name: name.map(str::to_string),
            world: "world".into(),
            dimension: worlds::OVERWORLD.into(),
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            last_seen: 0,
            online: false,
        }
    }

    fn shown(presence: &Presence, visibility: Visibility) -> Vec<(String, bool)> {
        let markers = vec![
            marker(1, Some("Steve")),
            marker(2, Some("Alex")),
            marker(3, None),
        ];
        visible(markers, presence, visibility)
            .into_iter()
            .map(|marker| (marker.name.unwrap_or_default(), marker.online))
            .collect()
    }

    #[test]
    fn the_setting_filters_by_online_state() {
        use crate::online::{Online, Source};
        let presence = Presence::Answered(Online {
            count: 1,
            max: 20,
            players: vec!["Steve".into()],
            ids: Vec::new(),
            complete: true,
            source: Source::Query,
        });
        let everyone = vec![
            ("Steve".to_string(), true),
            ("Alex".to_string(), false),
            (String::new(), false),
        ];
        assert_eq!(shown(&presence, Visibility::default()), everyone);
        let online_only = Visibility {
            online: true,
            offline: false,
        };
        assert_eq!(
            shown(&presence, online_only),
            vec![("Steve".to_string(), true)]
        );
        let offline_only = Visibility {
            online: false,
            offline: true,
        };
        assert_eq!(shown(&presence, offline_only), everyone[1..].to_vec());
        let nobody = Visibility {
            online: false,
            offline: false,
        };
        assert!(shown(&presence, nobody).is_empty());

        // a stopped server, or no answer: everyone is offline, so "online only" shows nobody
        for unknown in [Presence::NotRunning, Presence::Unknown("no answer".into())] {
            assert!(shown(&unknown, online_only).is_empty());
            assert_eq!(shown(&unknown, offline_only).len(), 3);
        }
    }

    #[test]
    fn every_server_shows_both_until_changed() {
        // servers without a saved setting (every server before 0.1.2) get the default
        assert_eq!(
            Visibility::default(),
            Visibility {
                online: true,
                offline: true,
            }
        );
        let saved: Visibility =
            serde_json::from_str(r#"{"online":false,"offline":true}"#).unwrap();
        assert!(!saved.shows(true) && saved.shows(false));
        // the page sends both; a missing one is an error, not a silent default
        assert!(serde_json::from_str::<Visibility>(r#"{"online":false}"#).is_err());
    }
}
