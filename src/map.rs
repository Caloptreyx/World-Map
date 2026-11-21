//! A server as the map sees it: its Wings connection, its worlds (cached briefly) and region
//! listings. Every path handed to Wings comes from the server's own folder listings, never from
//! request input.
use crate::{
    cache::Memo,
    coords,
    files::Wings,
    nbt,
    settings::Settings,
    worlds::{self, Dimension, Entry, LevelInfo, Lister, World},
};
use axum::http::StatusCode;
use serde::Serialize;
use shared::{State, models::server::Server, response::ApiResponse};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};
use uuid::Uuid;

/// Small files read whole: `server.properties`, `level.dat`, `uid.dat`, `usercache.json`,
/// player files.
pub const SMALL_FILE: u64 = 8 << 20;

#[async_trait::async_trait]
impl Lister for Wings {
    type Error = ApiResponse;

    async fn list(&self, path: &str) -> Result<Option<Vec<Entry>>, ApiResponse> {
        Ok(Wings::list(self, &format!("/{path}")).await?.map(|entries| {
            entries
                .into_iter()
                .map(|entry| Entry {
                    name: entry.name.to_string(),
                    directory: entry.directory,
                    modified: entry.modified.timestamp(),
                })
                .collect()
        }))
    }
}

/// Why a server gets no map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unsupported {
    /// The administrator limited the map to other eggs.
    Egg,
    /// A Bedrock server (no Anvil worlds).
    Bedrock,
    /// No `server.properties`: not a Minecraft Java server (or not installed yet).
    NotMinecraft,
}

#[derive(Debug, Serialize)]
pub struct Layout {
    pub level_name: String,
    pub worlds: Vec<World>,
    pub level: LevelInfo,
    #[serde(skip)]
    pub properties: Option<String>,
    /// Bukkit's world UUIDs (`uid.dat`) to world folders.
    #[serde(skip)]
    pub uids: HashMap<Uuid, String>,
    #[serde(skip)]
    pub unsupported: Option<Unsupported>,
}

impl Layout {
    pub fn world(&self, name: &str) -> Result<&World, ApiResponse> {
        self.worlds
            .iter()
            .find(|world| world.name == name)
            .ok_or_else(|| not_found("world not found"))
    }

    pub fn dimension(&self, world: &str, dimension: &str) -> Result<(&World, &Dimension), ApiResponse> {
        let world = self.world(world)?;
        let dimension = world
            .dimension(dimension)
            .ok_or_else(|| not_found("dimension not found"))?;
        Ok((world, dimension))
    }

    /// The world and dimension a player is in: Bukkit's world UUID when the player file has
    /// one, else the main world with the player's dimension.
    pub fn locate(&self, world_uid: Option<Uuid>, dimension: &str) -> Option<(&World, &Dimension)> {
        if let Some(folder) = world_uid.and_then(|uid| self.uids.get(&uid)) {
            for world in &self.worlds {
                let inside: Vec<&Dimension> = world
                    .dimensions
                    .iter()
                    .filter(|d| d.folder == *folder || d.folder.starts_with(&format!("{folder}/")))
                    .collect();
                if let Some(found) = inside
                    .iter()
                    .find(|d| d.id == dimension)
                    .or_else(|| inside.first())
                {
                    return Some((world, *found));
                }
            }
        }
        let main = self.worlds.first()?;
        Some((main, main.dimension(dimension)?))
    }
}

pub fn not_found(message: &str) -> ApiResponse {
    ApiResponse::error(message).with_status(StatusCode::NOT_FOUND)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RegionFile {
    pub x: i32,
    pub z: i32,
    pub size: u64,
    /// Unix seconds.
    pub modified: i64,
}

static LAYOUTS: LazyLock<Memo<Uuid, Arc<Layout>>> =
    LazyLock::new(|| Memo::new(Duration::from_secs(30), 4096));
static REGIONS: LazyLock<Memo<(Uuid, String), Arc<Vec<RegionFile>>>> =
    LazyLock::new(|| Memo::new(Duration::from_secs(10), 4096));

pub struct Map {
    pub wings: Wings,
    pub server: Uuid,
    pub egg: Uuid,
    pub settings: Settings,
}

impl Map {
    pub async fn open(state: &State, server: &Server) -> Result<Self, ApiResponse> {
        Ok(Self {
            wings: Wings::connect(state, server).await?,
            server: server.uuid,
            egg: server.egg.uuid,
            settings: crate::settings::load(state).await?,
        })
    }

    pub async fn read(&self, path: &str, max: u64) -> Result<Option<Vec<u8>>, ApiResponse> {
        self.wings.read(&format!("/{path}"), max).await
    }

    pub async fn write(&self, path: &str, user: Uuid, data: Vec<u8>) -> Result<Option<i64>, ApiResponse> {
        self.wings.write(&format!("/{path}"), user, data).await
    }

    pub fn region_limit(&self) -> u64 {
        u64::from(self.settings.max_region_mb) << 20
    }

    /// The worlds of the server, cached for 30 seconds unless `fresh`.
    pub async fn layout(&self, fresh: bool) -> Result<Arc<Layout>, ApiResponse> {
        if !fresh && let Some(layout) = LAYOUTS.get(&self.server) {
            return Ok(layout);
        }
        let layout = Arc::new(self.load_layout().await?);
        LAYOUTS.insert(self.server, layout.clone());
        Ok(layout)
    }

    async fn load_layout(&self) -> Result<Layout, ApiResponse> {
        let root = Lister::list(&self.wings, "").await?.unwrap_or_default();
        let has_file = |name: &str| root.iter().any(|e| !e.directory && e.name == name);
        let unsupported = if !self.settings.allows_egg(self.egg) {
            Some(Unsupported::Egg)
        } else if has_file("bedrock_server") || has_file("bedrock_server.exe") {
            Some(Unsupported::Bedrock)
        } else if !has_file("server.properties") {
            Some(Unsupported::NotMinecraft)
        } else {
            None
        };
        if let Some(reason) = unsupported {
            return Ok(Layout {
                level_name: "world".into(),
                worlds: Vec::new(),
                level: LevelInfo::default(),
                properties: None,
                uids: HashMap::new(),
                unsupported: Some(reason),
            });
        }

        let properties = self
            .read("server.properties", SMALL_FILE)
            .await?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let level_name = worlds::level_name(properties.as_deref());
        let worlds = worlds::detect(&self.wings, &level_name).await?;

        let level = match self.read(&format!("{level_name}/level.dat"), SMALL_FILE).await? {
            Some(bytes) => nbt::decompress(&bytes)
                .and_then(|data| nbt::read(&data))
                .map(|root| worlds::level_info(&root))
                .unwrap_or_default(),
            None => LevelInfo::default(),
        };

        let mut uids = HashMap::new();
        for folder in worlds.iter().flat_map(|world| world.folders.iter()) {
            if let Some(bytes) = self.read(&format!("{folder}/uid.dat"), 64).await?
                && let Ok(raw) = <[u8; 16]>::try_from(bytes.as_slice())
            {
                uids.insert(Uuid::from_bytes(raw), folder.clone());
            }
        }

        Ok(Layout {
            level_name,
            worlds,
            level,
            properties,
            uids,
            unsupported: None,
        })
    }

    /// The region files in `folder/<kind>` (`region` or `entities`), cached for 10 seconds
    /// unless `fresh`.
    pub async fn regions(
        &self,
        folder: &str,
        kind: &str,
        fresh: bool,
    ) -> Result<Arc<Vec<RegionFile>>, ApiResponse> {
        let key = (self.server, format!("{folder}/{kind}"));
        if !fresh && let Some(regions) = REGIONS.get(&key) {
            return Ok(regions);
        }
        let entries = self.wings.list(&format!("/{}", key.1)).await?.unwrap_or_default();
        let regions: Vec<RegionFile> = entries
            .into_iter()
            .filter(|entry| !entry.directory)
            .filter_map(|entry| {
                let (x, z) = coords::region_file(&entry.name)?;
                Some(RegionFile {
                    x,
                    z,
                    size: entry.size,
                    modified: entry.modified.timestamp(),
                })
            })
            .collect();
        let regions = Arc::new(regions);
        REGIONS.insert(key, regions.clone());
        Ok(regions)
    }

    /// Forgets cached listings after the server's files were changed here.
    pub fn forget(&self, folder: &str) {
        for kind in ["region", "entities", "poi"] {
            REGIONS.remove(&(self.server, format!("{folder}/{kind}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        let world = |name: &str, dimensions: &[(&str, &str)]| World {
            name: name.into(),
            dimensions: dimensions
                .iter()
                .map(|(id, folder)| Dimension {
                    id: id.to_string(),
                    folder: folder.to_string(),
                })
                .collect(),
            players: None,
            folders: vec![name.into()],
            level_modified: 0,
        };
        let mut main = world(
            "world",
            &[
                (worlds::OVERWORLD, "world"),
                (worlds::NETHER, "world_nether/DIM-1"),
            ],
        );
        main.folders.push("world_nether".into());
        Layout {
            level_name: "world".into(),
            worlds: vec![main, world("creative", &[(worlds::OVERWORLD, "creative")])],
            level: LevelInfo::default(),
            properties: None,
            uids: HashMap::from([
                (Uuid::from_u128(1), "world".into()),
                (Uuid::from_u128(2), "world_nether".into()),
                (Uuid::from_u128(3), "creative".into()),
            ]),
            unsupported: None,
        }
    }

    fn located(layout: &Layout, uid: Option<u128>, dimension: &str) -> Option<(String, String)> {
        layout
            .locate(uid.map(Uuid::from_u128), dimension)
            .map(|(world, dimension)| (world.name.clone(), dimension.id.clone()))
    }

    #[test]
    fn players_are_placed_by_their_world_uuid_then_dimension() {
        let layout = layout();
        let at = |world: &str, dimension: &str| Some((world.to_string(), dimension.to_string()));
        assert_eq!(
            located(&layout, Some(2), worlds::NETHER),
            at("world", worlds::NETHER)
        );
        // Bukkit names the Nether's dimension after the world's type, not the folder
        assert_eq!(
            located(&layout, Some(2), worlds::OVERWORLD),
            at("world", worlds::NETHER)
        );
        assert_eq!(
            located(&layout, Some(3), worlds::OVERWORLD),
            at("creative", worlds::OVERWORLD)
        );
        assert_eq!(
            located(&layout, None, worlds::NETHER),
            at("world", worlds::NETHER)
        );
        assert_eq!(located(&layout, None, worlds::END), None);
        assert_eq!(
            located(&layout, Some(9), worlds::OVERWORLD),
            at("world", worlds::OVERWORLD)
        );
    }
}
