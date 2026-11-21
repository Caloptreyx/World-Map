//! A server as the map sees it: its Wings connection, its worlds (cached briefly) and region
//! listings. Every path handed to Wings comes from the server's own folder listings, its
//! `server.properties` and `bukkit.yml`, or world folders checked by
//! [`worlds::relative_path`], never from request input.
use crate::{
    cache::Memo,
    coords, db,
    files::Wings,
    nbt,
    settings::Settings,
    worlds::{self, Dimension, Entry, Files, LevelInfo, World},
};
use axum::http::StatusCode;
use futures_util::{StreamExt, stream};
use serde::Serialize;
use shared::{State, database::Database, models::server::Server, response::ApiResponse};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};
use uuid::Uuid;

/// Small files read whole: `server.properties`, `bukkit.yml`, `level.dat`, `uid.dat`,
/// `usercache.json`, player files.
pub const SMALL_FILE: u64 = 8 << 20;

#[async_trait::async_trait]
impl Files for Wings {
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

    async fn read(&self, path: &str, max: u64) -> Result<Option<Vec<u8>>, ApiResponse> {
        Wings::read(self, &format!("/{path}"), max).await
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

/// A world folder added by hand, and whether a world was found in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtraWorld {
    pub path: String,
    pub found: bool,
}

#[derive(Debug, Serialize)]
pub struct Layout {
    /// `level-name` as `server.properties` has it.
    pub level_name: String,
    /// The folder `level-name` points to, when it exists.
    pub main_folder: Option<String>,
    /// Bukkit's world container (`bukkit.yml`), when one is set.
    pub world_container: Option<String>,
    /// The main world first.
    pub worlds: Vec<World>,
    pub extra_worlds: Vec<ExtraWorld>,
    #[serde(skip)]
    pub properties: Option<String>,
    /// Bukkit's world UUIDs (`uid.dat`) to world folders.
    #[serde(skip)]
    pub uids: HashMap<Uuid, String>,
    #[serde(skip)]
    pub unsupported: Option<Unsupported>,
}

impl Layout {
    /// The world `level-name` points to, when it holds a world.
    pub fn main(&self) -> Option<&World> {
        self.worlds.first().filter(|world| world.main)
    }

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
    /// one, else the dimension key the player file names (Bukkit names other worlds
    /// `minecraft:<world>`), else that dimension of the main world.
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
        if let Some(found) = worlds::by_key(&self.worlds, dimension) {
            return Some(found);
        }
        let main = self.main()?;
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
/// Whether a datapack dimension (by its folder) has a ceiling; datapacks rarely change.
static CEILINGS: LazyLock<Memo<(Uuid, String), bool>> =
    LazyLock::new(|| Memo::new(Duration::from_secs(600), 4096));

pub struct Map {
    pub wings: Wings,
    pub database: Arc<Database>,
    pub server: Uuid,
    pub egg: Uuid,
    pub settings: Settings,
}

impl Map {
    pub async fn open(state: &State, server: &Server) -> Result<Self, ApiResponse> {
        Ok(Self {
            wings: Wings::connect(state, server).await?,
            database: state.database.clone(),
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

    /// Forgets the cached worlds after their settings changed.
    pub fn forget_layout(&self) {
        LAYOUTS.remove(&self.server);
    }

    /// What the world's `level.dat` says; nothing when it is missing or unreadable.
    async fn level(&self, world: &str) -> LevelInfo {
        match self.read(&format!("{world}/level.dat"), SMALL_FILE).await {
            Ok(Some(bytes)) => nbt::decompress(&bytes)
                .and_then(|data| nbt::read(&data))
                .map(|root| worlds::level_info(&root))
                .unwrap_or_default(),
            _ => LevelInfo::default(),
        }
    }

    /// Looks up which datapack dimensions of `world` have a ceiling.
    async fn find_ceilings(&self, world: &mut World) {
        let Some(datapacks) = world.datapacks.clone() else {
            return;
        };
        let mut wanted = Vec::new();
        for dimension in &mut world.dimensions {
            if [worlds::OVERWORLD, worlds::NETHER, worlds::END].contains(&dimension.id.as_str()) {
                continue;
            }
            match CEILINGS.get(&(self.server, dimension.folder.clone())) {
                Some(ceiling) => dimension.ceiling = ceiling,
                None => wanted.push(dimension.id.clone()),
            }
        }
        if wanted.is_empty() {
            return;
        }
        let found = worlds::ceilings(&self.wings, &datapacks, &wanted).await;
        for dimension in &mut world.dimensions {
            if let Some(&ceiling) = found.get(&dimension.id) {
                dimension.ceiling = ceiling;
                CEILINGS.insert((self.server, dimension.folder.clone()), ceiling);
            }
        }
    }

    async fn load_layout(&self) -> Result<Layout, ApiResponse> {
        let root = Files::list(&self.wings, "").await?.unwrap_or_default();
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
                main_folder: None,
                world_container: None,
                worlds: Vec::new(),
                extra_worlds: Vec::new(),
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
        let level_path = worlds::relative_path(&level_name);
        let bukkit = has_file("bukkit.yml");
        let world_container = if bukkit {
            self.read("bukkit.yml", SMALL_FILE)
                .await
                .ok()
                .flatten()
                .and_then(|bytes| worlds::world_container(&String::from_utf8_lossy(&bytes)))
        } else {
            None
        };
        let extra = db::extra_worlds(&self.database, self.server).await?;
        let detected = worlds::detect(
            &self.wings,
            &worlds::Options {
                level_name: level_path.as_deref(),
                container: world_container.as_deref(),
                extra: &extra,
                bukkit,
            },
        )
        .await?;
        let mut worlds = detected.worlds;

        let names: Vec<String> = worlds.iter().map(|world| world.name.clone()).collect();
        let levels: Vec<LevelInfo> = stream::iter(names)
            .map(|name| async move { self.level(&name).await })
            .buffered(6)
            .collect()
            .await;
        for (world, level) in worlds.iter_mut().zip(levels) {
            world.level = level;
        }
        for world in &mut worlds {
            self.find_ceilings(world).await;
        }

        let mut uids = HashMap::new();
        for folder in worlds.iter().flat_map(|world| world.folders.iter()) {
            if let Ok(Some(bytes)) = self.read(&format!("{folder}/uid.dat"), 64).await
                && let Ok(raw) = <[u8; 16]>::try_from(bytes.as_slice())
            {
                uids.insert(Uuid::from_bytes(raw), folder.clone());
            }
        }

        let extra_worlds = extra
            .into_iter()
            .map(|path| ExtraWorld {
                found: worlds
                    .iter()
                    .any(|world| world.name == path || world.folders.contains(&path)),
                path,
            })
            .collect();
        Ok(Layout {
            level_name,
            main_folder: detected.main_folder,
            world_container,
            worlds,
            extra_worlds,
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

    fn world(name: &str, dimensions: &[(&str, &str, Option<&str>)]) -> World {
        World {
            name: name.into(),
            main: false,
            extra: false,
            dimensions: dimensions
                .iter()
                .map(|(id, folder, key)| Dimension {
                    id: id.to_string(),
                    folder: folder.to_string(),
                    key: key.map(str::to_string),
                    ceiling: false,
                })
                .collect(),
            players: None,
            level: LevelInfo::default(),
            folders: vec![name.into()],
            datapacks: None,
            level_modified: 0,
        }
    }

    fn layout() -> Layout {
        let mut main = world(
            "world",
            &[
                (worlds::OVERWORLD, "world", Some(worlds::OVERWORLD)),
                (worlds::NETHER, "world_nether/DIM-1", Some(worlds::NETHER)),
            ],
        );
        main.main = true;
        main.folders.push("world_nether".into());
        Layout {
            level_name: "world".into(),
            main_folder: Some("world".into()),
            world_container: None,
            worlds: vec![
                main,
                world(
                    "creative",
                    &[(worlds::OVERWORLD, "creative", Some("minecraft:creative"))],
                ),
                world("old", &[(worlds::OVERWORLD, "old", None)]),
            ],
            extra_worlds: Vec::new(),
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
        // Paper's per-world key, without a uid.dat to go by
        assert_eq!(
            located(&layout, None, "minecraft:creative"),
            at("creative", worlds::OVERWORLD)
        );
        assert_eq!(located(&layout, None, worlds::END), None);
        assert_eq!(located(&layout, None, "minecraft:old"), None);
        assert_eq!(
            located(&layout, Some(9), worlds::OVERWORLD),
            at("world", worlds::OVERWORLD)
        );
    }
}
