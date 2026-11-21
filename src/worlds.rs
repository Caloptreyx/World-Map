//! Finding a server's worlds and their dimensions from its folders:
//! - vanilla before 26.1: `world/region`, `world/DIM-1/region`, `world/DIM1/region`;
//! - Paper and Spigot before 26.1: the Nether and End in `world_nether/DIM-1` and
//!   `world_the_end/DIM1`, each with its own `level.dat`;
//! - 26.1 and newer: `world/dimensions/<namespace>/<name>/region` for every dimension
//!   (Spigot keeps `world_nether/dimensions/minecraft/the_nether`);
//! - datapack dimensions in `dimensions/<namespace>/<name>` before 26.1 too;
//! - any other folder with a `level.dat` (Multiverse and similar plugins) as its own world.
//!
//! The `<level-name>_nether` and `<level-name>_the_end` folders are shown as dimensions of the
//! main world. When both they and the main world hold the same dimension (a server that
//! switched between Paper and vanilla), the copy whose `level.dat` the server saved last wins.
use crate::nbt::{Compound, Tag};
use serde::Serialize;

pub const OVERWORLD: &str = "minecraft:overworld";
pub const NETHER: &str = "minecraft:the_nether";
pub const END: &str = "minecraft:the_end";

/// Root folders that are never worlds.
const NOT_WORLDS: &[&str] = &[
    "plugins",
    "logs",
    "cache",
    "libraries",
    "versions",
    "config",
    "mods",
    "crash-reports",
    "bundler",
    "defaultconfigs",
    "kubejs",
    "resourcepacks",
    "shaderpacks",
    "backups",
    "debug",
    "journeymap",
];
/// Folders looked into for a `level.dat`, to bound the Wings requests.
const MAX_CANDIDATES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    /// Unix seconds.
    pub modified: i64,
}

/// Lists directories below the server root (`""` is the root); `None` when missing.
#[async_trait::async_trait]
pub trait Lister: Sync {
    type Error: Send;
    async fn list(&self, path: &str) -> Result<Option<Vec<Entry>>, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dimension {
    /// `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end` or a datapack's.
    pub id: String,
    /// The folder holding `region/`, `entities/` and `poi/`.
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct World {
    /// The folder name, which is also the world's id.
    pub name: String,
    pub dimensions: Vec<Dimension>,
    /// The folder with the player files (`playerdata`, or `players/data` since 26.1).
    pub players: Option<String>,
    /// The world folders it was read from: its own and the Paper/Spigot dimension folders
    /// folded into it.
    #[serde(skip)]
    pub folders: Vec<String>,
    #[serde(skip)]
    pub(crate) level_modified: i64,
}

impl World {
    pub fn dimension(&self, id: &str) -> Option<&Dimension> {
        self.dimensions.iter().find(|dimension| dimension.id == id)
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn has_dir(entries: &[Entry], name: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.directory && entry.name == name)
}

fn dimension_order(id: &str) -> (u8, &str) {
    match id {
        OVERWORLD => (0, id),
        NETHER => (1, id),
        END => (2, id),
        _ => (3, id),
    }
}

/// The dimensions and player folder of the world folder `folder` with the entries `entries`.
async fn scan<L: Lister>(fs: &L, folder: &str, entries: &[Entry]) -> Result<World, L::Error> {
    let mut dimensions = Vec::new();
    if has_dir(entries, "region") {
        dimensions.push(Dimension {
            id: OVERWORLD.into(),
            folder: folder.to_string(),
        });
    }
    for (name, id) in [("DIM-1", NETHER), ("DIM1", END)] {
        if !has_dir(entries, name) {
            continue;
        }
        let path = join(folder, name);
        if fs
            .list(&path)
            .await?
            .is_some_and(|inner| has_dir(&inner, "region"))
        {
            dimensions.push(Dimension {
                id: id.into(),
                folder: path,
            });
        }
    }
    if has_dir(entries, "dimensions") {
        let root = join(folder, "dimensions");
        for namespace in fs.list(&root).await?.unwrap_or_default() {
            if !namespace.directory {
                continue;
            }
            let namespace_path = join(&root, &namespace.name);
            for dimension in fs.list(&namespace_path).await?.unwrap_or_default() {
                if !dimension.directory {
                    continue;
                }
                let id = format!("{}:{}", namespace.name, dimension.name);
                // a datapack dimension beside vanilla's own folders loses to them
                if dimensions.iter().any(|known: &Dimension| known.id == id) {
                    continue;
                }
                dimensions.push(Dimension {
                    id,
                    folder: join(&namespace_path, &dimension.name),
                });
            }
        }
    }
    dimensions.sort_by(|a, b| dimension_order(&a.id).cmp(&dimension_order(&b.id)));

    let players = if has_dir(entries, "players")
        && fs
            .list(&join(folder, "players"))
            .await?
            .is_some_and(|inner| has_dir(&inner, "data"))
    {
        Some(join(folder, "players/data"))
    } else if has_dir(entries, "playerdata") {
        Some(join(folder, "playerdata"))
    } else {
        None
    };
    let level_modified = entries
        .iter()
        .find(|entry| !entry.directory && entry.name == "level.dat")
        .map_or(0, |entry| entry.modified);
    Ok(World {
        name: folder.to_string(),
        dimensions,
        players,
        folders: vec![folder.to_string()],
        level_modified,
    })
}

/// The worlds of a server whose main world is `level_name`, the main world first.
pub async fn detect<L: Lister>(fs: &L, level_name: &str) -> Result<Vec<World>, L::Error> {
    let root = fs.list("").await?.unwrap_or_default();
    let mut candidates: Vec<String> = vec![level_name.to_string()];
    candidates.extend(
        root.iter()
            .filter(|entry| {
                entry.directory
                    && !entry.name.starts_with('.')
                    && !NOT_WORLDS.contains(&entry.name.as_str())
                    && entry.name != level_name
            })
            .map(|entry| entry.name.clone()),
    );
    candidates.truncate(MAX_CANDIDATES);

    let mut worlds = Vec::new();
    for candidate in &candidates {
        let Some(entries) = fs.list(candidate).await? else {
            continue;
        };
        if entries
            .iter()
            .any(|entry| !entry.directory && entry.name == "level.dat")
        {
            worlds.push(scan(fs, candidate, &entries).await?);
        }
    }

    // fold `<level>_nether` and `<level>_the_end` into the main world
    if let Some(main) = worlds.iter().position(|world| world.name == level_name) {
        let mut main_world = worlds.remove(main);
        let siblings = [format!("{level_name}_nether"), format!("{level_name}_the_end")];
        worlds.retain(|world| {
            if !siblings.contains(&world.name) {
                return true;
            }
            for dimension in &world.dimensions {
                match main_world
                    .dimensions
                    .iter()
                    .position(|known| known.id == dimension.id)
                {
                    None => main_world.dimensions.push(dimension.clone()),
                    Some(index) if world.level_modified >= main_world.level_modified - 3600 => {
                        main_world.dimensions[index] = dimension.clone();
                    }
                    Some(_) => {}
                }
            }
            main_world.folders.push(world.name.clone());
            false
        });
        main_world
            .dimensions
            .sort_by(|a, b| dimension_order(&a.id).cmp(&dimension_order(&b.id)));
        worlds.insert(0, main_world);
    }
    worlds.retain(|world| !world.dimensions.is_empty());
    Ok(worlds)
}

/// `level-name` from `server.properties` (Java properties: `key=value` or `key: value`,
/// backslash escapes); `world` when unset or not a plain relative path.
pub fn level_name(properties: Option<&str>) -> String {
    let value = properties.and_then(|text| {
        text.lines().find_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("level-name")?;
            let rest = rest.trim_start();
            let rest = rest
                .strip_prefix('=')
                .or_else(|| rest.strip_prefix(':'))?;
            let mut value = String::new();
            let mut chars = rest.trim().chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        value.push(next);
                    }
                } else {
                    value.push(c);
                }
            }
            Some(value)
        })
    });
    match value {
        Some(value)
            if !value.is_empty()
                && !value.starts_with('/')
                && value
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..") =>
        {
            value
        }
        _ => "world".into(),
    }
}

/// What `level.dat` says about a world.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LevelInfo {
    pub data_version: Option<i64>,
    pub version: Option<String>,
    pub spawn: Option<[f64; 3]>,
}

/// 17w47a: the first version with block palettes, the oldest the map can draw.
pub const OLDEST_DATA_VERSION: i64 = 1451;

pub fn level_info(root: &Compound) -> LevelInfo {
    let Some(data) = root.compound("Data") else {
        return LevelInfo::default();
    };
    let spawn = match (data.i64("SpawnX"), data.i64("SpawnY"), data.i64("SpawnZ")) {
        (Some(x), Some(y), Some(z)) => Some([x as f64, y as f64, z as f64]),
        // 1.21.9+: `spawn: {pos: [I; x, y, z], dimension, ...}`
        _ => data
            .compound("spawn")
            .and_then(|spawn| spawn.get("pos"))
            .and_then(Tag::as_ints)
            .filter(|pos| pos.len() == 3)
            .map(|pos| [pos[0] as f64, pos[1] as f64, pos[2] as f64]),
    };
    LevelInfo {
        data_version: data.i64("DataVersion"),
        version: data
            .compound("Version")
            .and_then(|version| version.str("Name"))
            .map(str::to_string),
        spawn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A file tree from paths; directories end with `/`, `level.dat@<mtime>` sets a time.
    struct Tree(HashMap<String, Vec<Entry>>);

    impl Tree {
        fn new(paths: &[&str]) -> Self {
            let mut map: HashMap<String, Vec<Entry>> = HashMap::new();
            map.insert(String::new(), Vec::new());
            for path in paths {
                let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
                for depth in 0..parts.len() {
                    let parent = parts[..depth].join("/");
                    let (name, modified) = match parts[depth].split_once('@') {
                        Some((name, time)) => (name, time.parse().unwrap()),
                        None => (parts[depth], 0),
                    };
                    let directory = depth + 1 < parts.len() || path.ends_with('/');
                    let siblings = map.entry(parent).or_default();
                    if !siblings.iter().any(|entry| entry.name == name) {
                        siblings.push(Entry {
                            name: name.to_string(),
                            directory,
                            modified,
                        });
                    }
                    if directory {
                        map.entry(parts[..=depth].join("/").replace(&format!("@{modified}"), ""))
                            .or_default();
                    }
                }
            }
            Self(map)
        }
    }

    #[async_trait::async_trait]
    impl Lister for Tree {
        type Error = ();
        async fn list(&self, path: &str) -> Result<Option<Vec<Entry>>, ()> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn ids(world: &World) -> Vec<(&str, &str)> {
        world
            .dimensions
            .iter()
            .map(|d| (d.id.as_str(), d.folder.as_str()))
            .collect()
    }

    async fn detect_tree(paths: &[&str], level: &str) -> Vec<World> {
        detect(&Tree::new(paths), level).await.unwrap()
    }

    #[tokio::test]
    async fn vanilla_before_26_1() {
        let worlds = detect_tree(
            &[
                "server.properties",
                "world/level.dat",
                "world/region/r.0.0.mca",
                "world/DIM-1/region/r.0.0.mca",
                "world/DIM1/region/",
                "world/playerdata/",
                "logs/latest.log",
            ],
            "world",
        )
        .await;
        assert_eq!(worlds.len(), 1);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "world"),
                (NETHER, "world/DIM-1"),
                (END, "world/DIM1"),
            ]
        );
        assert_eq!(worlds[0].players.as_deref(), Some("world/playerdata"));
    }

    #[tokio::test]
    async fn paper_folders_become_dimensions_of_the_main_world() {
        let worlds = detect_tree(
            &[
                "world/level.dat",
                "world/region/",
                "world/playerdata/",
                "world_nether/level.dat",
                "world_nether/DIM-1/region/",
                "world_the_end/level.dat",
                "world_the_end/DIM1/region/",
                "plugins/Multiverse-Core/",
                "creative/level.dat",
                "creative/region/",
            ],
            "world",
        )
        .await;
        assert_eq!(
            worlds.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            vec!["world", "creative"]
        );
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "world"),
                (NETHER, "world_nether/DIM-1"),
                (END, "world_the_end/DIM1"),
            ]
        );
        assert_eq!(ids(&worlds[1]), vec![(OVERWORLD, "creative")]);
        assert_eq!(worlds[1].players, None);
    }

    #[tokio::test]
    async fn the_26_1_layout_and_datapack_dimensions() {
        let worlds = detect_tree(
            &[
                "survival/level.dat",
                "survival/dimensions/minecraft/overworld/region/",
                "survival/dimensions/minecraft/the_nether/region/",
                "survival/dimensions/minecraft/the_end/region/",
                "survival/dimensions/mypack/mining/region/",
                "survival/players/data/",
            ],
            "survival",
        )
        .await;
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "survival/dimensions/minecraft/overworld"),
                (NETHER, "survival/dimensions/minecraft/the_nether"),
                (END, "survival/dimensions/minecraft/the_end"),
                ("mypack:mining", "survival/dimensions/mypack/mining"),
            ]
        );
        assert_eq!(worlds[0].players.as_deref(), Some("survival/players/data"));
    }

    #[tokio::test]
    async fn the_dimension_saved_last_wins() {
        // switched from Paper to vanilla: world_nether is stale
        let vanilla = detect_tree(
            &[
                "world/level.dat@2000000",
                "world/region/",
                "world/DIM-1/region/",
                "world_nether/level.dat@1000000",
                "world_nether/DIM-1/region/",
            ],
            "world",
        )
        .await;
        assert_eq!(ids(&vanilla[0])[1], (NETHER, "world/DIM-1"));
        // switched from vanilla to Paper: world/DIM-1 is stale
        let paper = detect_tree(
            &[
                "world/level.dat@2000000",
                "world/region/",
                "world/DIM-1/region/",
                "world_nether/level.dat@2000010",
                "world_nether/DIM-1/region/",
            ],
            "world",
        )
        .await;
        assert_eq!(ids(&paper[0])[1], (NETHER, "world_nether/DIM-1"));
    }

    #[tokio::test]
    async fn folders_without_a_level_or_regions_are_not_worlds() {
        let worlds = detect_tree(
            &[
                "world/level.dat",
                "world/region/",
                "backup_world/region/",
                "empty/level.dat",
            ],
            "world",
        )
        .await;
        assert_eq!(worlds.len(), 1);
        assert!(detect_tree(&["plugins/x/"], "world").await.is_empty());
    }

    #[test]
    fn reads_the_level_name() {
        assert_eq!(level_name(None), "world");
        assert_eq!(level_name(Some("motd=x\nlevel-name=survival\n")), "survival");
        assert_eq!(level_name(Some("level-name = my\\ world")), "my world");
        assert_eq!(level_name(Some("level-name=worlds/main")), "worlds/main");
        assert_eq!(level_name(Some("level-name=")), "world");
        assert_eq!(level_name(Some("level-name=../etc")), "world");
        assert_eq!(level_name(Some("level-name=/abs")), "world");
        assert_eq!(level_name(Some("level-name-old=x")), "world");
    }
}
