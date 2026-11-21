//! Finding a server's worlds and their dimensions from its folders:
//! - the main world is `level-name` from `server.properties`, inside Bukkit's world container
//!   (`settings.world-container` in `bukkit.yml`) when one is set;
//! - vanilla before 26.1: `<world>/region`, `<world>/DIM-1/region`, `<world>/DIM1/region`;
//! - Paper and Spigot before 26.1: the Nether and End in `<level>_nether/DIM-1` and
//!   `<level>_the_end/DIM1`, datapack dimensions in `<level>_<path>/dimensions/<ns>/<path>`,
//!   each with its own `level.dat`; they are shown as dimensions of the main world;
//! - 26.1 and newer: `<world>/dimensions/<namespace>/<path>/region` for every dimension;
//! - datapack and mod dimensions in `<world>/dimensions/<namespace>/<path>` before 26.1 too;
//! - any other folder with a `level.dat` (Multiverse and similar plugins) up to two folders
//!   below the server root or the world container, plus folders added by hand, as their own
//!   worlds.
//!
//! When a sibling folder and the main world hold the same dimension (a server that switched
//! between Paper and vanilla), the copy whose `level.dat` the server saved last wins.
use crate::nbt::{Compound, Tag};
use futures_util::{StreamExt, stream};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const OVERWORLD: &str = "minecraft:overworld";
pub const NETHER: &str = "minecraft:the_nether";
pub const END: &str = "minecraft:the_end";

/// Folders never looked into for worlds: server software, plugin and mod data, backups, and the
/// insides of a world (when its `level.dat` is missing).
const IGNORED: &[&str] = &[
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
    "backup",
    "world-backups",
    "debug",
    "journeymap",
    "bluemap",
    "dynmap",
    "squaremap",
    "node_modules",
    "region",
    "entities",
    "poi",
    "data",
    "DIM-1",
    "DIM1",
    "dimensions",
    "datapacks",
    "playerdata",
    "players",
    "stats",
    "advancements",
];
/// How far below the server root (and the world container) worlds are looked for.
const MAX_DEPTH: usize = 2;
/// Folder listings spent looking for worlds, to bound the Wings requests.
const MAX_LISTINGS: usize = 64;
/// Worlds found by looking around (the main world and hand-added folders come on top).
const MAX_FOUND: usize = 32;
/// Datapack or mod dimensions read per world, and how deep their paths go.
const MAX_CUSTOM: usize = 32;
const MAX_CUSTOM_DEPTH: usize = 3;
/// Wings requests made at the same time.
const CONCURRENCY: usize = 8;
/// Hand-added world folders per server.
pub const MAX_EXTRA: usize = 16;
/// Datapacks and datapack files read to find out whether a dimension has a ceiling.
const MAX_PACKS: usize = 16;
const MAX_PACK_READS: usize = 48;
const PACK_FILE: u64 = 256 << 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    /// Unix seconds.
    pub modified: i64,
}

/// Lists directories and reads small files below the server root (`""` is the root); `None`
/// when missing.
#[async_trait::async_trait]
pub trait Files: Sync {
    type Error: Send;
    async fn list(&self, path: &str) -> Result<Option<Vec<Entry>>, Self::Error>;
    async fn read(&self, path: &str, max: u64) -> Result<Option<Vec<u8>>, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dimension {
    /// `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end` or a datapack's.
    pub id: String,
    /// The folder holding `region/`, `entities/` and `poi/`.
    pub folder: String,
    /// The key `execute in` takes for it on the running server; `None` when the server does not
    /// load it (or it cannot be told), so nobody can be teleported there.
    pub key: Option<String>,
    /// Drawn below a roof: the Nether, and datapack dimensions whose type has a ceiling.
    pub ceiling: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct World {
    /// The folder below the server root, which is also the world's id.
    pub name: String,
    /// The world `level-name` points to.
    pub main: bool,
    /// Added by hand in the map's world settings.
    pub extra: bool,
    pub dimensions: Vec<Dimension>,
    /// The folder with the player files (`playerdata`, or `players/data` since 26.1).
    pub players: Option<String>,
    /// What its `level.dat` says.
    pub level: LevelInfo,
    /// The world folders it was read from: its own and the Paper/Spigot dimension folders
    /// folded into it.
    #[serde(skip)]
    pub folders: Vec<String>,
    /// Its `datapacks` folder, when it has one.
    #[serde(skip)]
    pub datapacks: Option<String>,
    #[serde(skip)]
    pub(crate) level_modified: i64,
}

impl World {
    pub fn dimension(&self, id: &str) -> Option<&Dimension> {
        self.dimensions.iter().find(|dimension| dimension.id == id)
    }
}

/// What [`detect`] looks for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options<'a> {
    /// `level-name`, when it is a folder below the server root (see [`relative_path`]).
    pub level_name: Option<&'a str>,
    /// Bukkit's world container (`settings.world-container`).
    pub container: Option<&'a str>,
    /// Folders added by hand, already checked by [`relative_path`].
    pub extra: &'a [String],
    /// A Bukkit server (Paper, Spigot, Purpur...): it loads every world in its container.
    pub bukkit: bool,
}

#[derive(Debug, Default)]
pub struct Detected {
    /// The main world first, then the others by name.
    pub worlds: Vec<World>,
    /// The folder `level-name` points to, when it exists (it may hold no world yet).
    pub main_folder: Option<String>,
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// The folder a path is in (`""` for the root) and its own name.
fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn has_dir(entries: &[Entry], name: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.directory && entry.name == name)
}

fn has_file(entries: &[Entry], name: &str) -> bool {
    entries
        .iter()
        .any(|entry| !entry.directory && entry.name == name)
}

fn browsable(entry: &Entry) -> bool {
    entry.directory && !entry.name.starts_with('.') && !IGNORED.contains(&entry.name.as_str())
}

fn dimension_order(id: &str) -> (u8, &str) {
    match id {
        OVERWORLD => (0, id),
        NETHER => (1, id),
        END => (2, id),
        _ => (3, id),
    }
}

/// A dimension id as commands take it: `namespace:path`, lowercase.
pub fn valid_key(id: &str) -> bool {
    id.split_once(':').is_some_and(|(namespace, path)| {
        !namespace.is_empty()
            && !path.is_empty()
            && namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
            && path
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-./".contains(&b))
    })
}

/// The dimension key Bukkit gives a world it loads by `name` (its folder below the world
/// container): `minecraft:<name in lowercase>`.
pub fn bukkit_key(name: &str) -> Option<String> {
    let key = format!("minecraft:{}", name.to_lowercase());
    valid_key(&key).then_some(key)
}

/// The world folder and dimension id a dimension key points to: the inverse of the keys
/// [`detect`] hands out.
pub fn by_key<'a>(worlds: &'a [World], key: &str) -> Option<(&'a World, &'a Dimension)> {
    worlds.iter().find_map(|world| {
        world
            .dimensions
            .iter()
            .find(|dimension| dimension.key.as_deref() == Some(key))
            .map(|dimension| (world, dimension))
    })
}

/// A folder below the server root as the panel's file access takes it: `/`-separated, not
/// absolute, without `.` or `..` parts. A leading `./` and trailing `/` are dropped.
pub fn relative_path(input: &str) -> Option<String> {
    let path = input.strip_prefix("./").unwrap_or(input).trim_end_matches('/');
    let valid = !path.is_empty()
        && path.len() <= 255
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    valid.then(|| path.to_string())
}

/// Lists `paths` a few at a time; the results in the same order. A folder that cannot be
/// listed (a file, a path Wings refuses) counts as missing: these paths come from settings and
/// guesses, and one bad path must not hide every world.
async fn list_all<L: Files>(fs: &L, paths: Vec<String>) -> Vec<(String, Option<Vec<Entry>>)> {
    stream::iter(paths)
        .map(|path| async move {
            let entries = fs.list(&path).await.ok().flatten();
            (path, entries)
        })
        .buffered(CONCURRENCY)
        .collect()
        .await
}

/// The datapack or mod dimensions below `root` (a world's `dimensions` folder):
/// `<namespace>/<path>/region`, the path up to [`MAX_CUSTOM_DEPTH`] folders deep.
async fn custom_dimensions<L: Files>(fs: &L, root: &str) -> Result<Vec<Dimension>, L::Error> {
    let mut found = Vec::new();
    // (folder, namespace, path below the namespace)
    let mut pending: Vec<(String, String, String)> = fs
        .list(root)
        .await?
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.directory)
        .map(|entry| (join(root, &entry.name), entry.name, String::new()))
        .collect();
    let mut listings = 0;
    while let Some((folder, namespace, path)) = pending.pop() {
        if found.len() >= MAX_CUSTOM || listings >= MAX_CUSTOM * 2 {
            break;
        }
        listings += 1;
        let Some(entries) = fs.list(&folder).await? else {
            continue;
        };
        if !path.is_empty() && has_dir(&entries, "region") {
            found.push(Dimension {
                id: format!("{namespace}:{path}"),
                folder,
                key: None,
                ceiling: false,
            });
            continue;
        }
        if path.split('/').count() >= MAX_CUSTOM_DEPTH {
            continue;
        }
        for entry in entries.iter().filter(|entry| {
            entry.directory && !["region", "entities", "poi", "data"].contains(&entry.name.as_str())
        }) {
            pending.push((
                join(&folder, &entry.name),
                namespace.clone(),
                if path.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{path}/{}", entry.name)
                },
            ));
        }
    }
    Ok(found)
}

/// The dimensions and player folder of the world folder `folder` with the entries `entries`.
async fn scan<L: Files>(fs: &L, folder: &str, entries: &[Entry]) -> Result<World, L::Error> {
    let mut dimensions = Vec::new();
    if has_dir(entries, "region") {
        dimensions.push(Dimension {
            id: OVERWORLD.into(),
            folder: folder.to_string(),
            key: None,
            ceiling: false,
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
                key: None,
                ceiling: false,
            });
        }
    }
    if has_dir(entries, "dimensions") {
        for dimension in custom_dimensions(fs, &join(folder, "dimensions")).await? {
            // a datapack dimension beside vanilla's own folders loses to them
            if !dimensions.iter().any(|known: &Dimension| known.id == dimension.id) {
                dimensions.push(dimension);
            }
        }
    }
    for dimension in &mut dimensions {
        dimension.ceiling = dimension.id == NETHER;
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
        main: false,
        extra: false,
        dimensions,
        players,
        level: LevelInfo::default(),
        folders: vec![folder.to_string()],
        datapacks: has_dir(entries, "datapacks").then(|| join(folder, "datapacks")),
        level_modified,
    })
}

/// Whether the world `sibling`, found beside the main world `main`, is one of the main world's
/// dimensions saved apart by Paper or Spigot: `<main>_nether`, `<main>_the_end` or
/// `<main>_<path>` for a datapack dimension `<namespace>:<path>`, without an Overworld of its
/// own.
fn is_sibling(main: &str, sibling: &World) -> bool {
    let Some(suffix) = sibling
        .name
        .strip_prefix(main)
        .and_then(|rest| rest.strip_prefix('_'))
    else {
        return false;
    };
    !sibling.dimensions.is_empty()
        && sibling.dimension(OVERWORLD).is_none()
        && sibling.dimensions.iter().any(|dimension| {
            let path = dimension.id.split_once(':').map_or("", |(_, path)| path);
            path == suffix || (suffix == "nether" && dimension.id == NETHER)
        })
}

/// The worlds of a server, the main world first.
pub async fn detect<L: Files>(fs: &L, options: &Options<'_>) -> Result<Detected, L::Error> {
    let container = options.container.filter(|container| !container.is_empty());
    let mut listed: HashMap<String, Option<Vec<Entry>>> = HashMap::new();

    // what is looked at in any case: the roots, where the main world may be, the folders
    // beside it (for its Paper siblings) and the folders added by hand
    let mut main_candidates = Vec::new();
    if let Some(level) = options.level_name {
        if let Some(container) = container {
            main_candidates.push(join(container, level));
        }
        main_candidates.push(level.to_string());
    }
    listed.insert(String::new(), fs.list("").await?);
    let mut first: Vec<String> = container.map(str::to_string).into_iter().collect();
    for candidate in &main_candidates {
        first.push(candidate.clone());
        first.push(split_parent(candidate).0.to_string());
    }
    first.extend(options.extra.iter().cloned());
    let mut seen = HashSet::from([String::new()]);
    first.retain(|path| seen.insert(path.clone()));
    listed.extend(list_all(fs, first).await);

    let main_folder = main_candidates
        .iter()
        .find(|candidate| matches!(listed.get(*candidate), Some(Some(_))))
        .cloned();
    let main = main_folder.clone().filter(|folder| {
        listed.get(folder).and_then(Option::as_ref).is_some_and(|entries| {
            has_file(entries, "level.dat")
                || has_dir(entries, "region")
                || has_dir(entries, "dimensions")
        })
    });

    // the main world's siblings, wherever it is
    let mut siblings = Vec::new();
    if let Some(main) = &main {
        let (parent, base) = split_parent(main);
        let prefix = format!("{base}_");
        if let Some(Some(entries)) = listed.get(parent) {
            siblings.extend(
                entries
                    .iter()
                    .filter(|entry| entry.directory && entry.name.starts_with(&prefix))
                    .map(|entry| join(parent, &entry.name))
                    .take(MAX_CUSTOM),
            );
        }
    }
    let unlisted: Vec<String> = siblings
        .iter()
        .filter(|path| !listed.contains_key(*path))
        .cloned()
        .collect();
    listed.extend(list_all(fs, unlisted).await);

    // looking around: up to MAX_DEPTH folders below the roots, not into worlds or ignored folders
    let mut found = Vec::new();
    let mut frontier = Vec::new();
    for root in std::iter::once("").chain(container) {
        if let Some(Some(entries)) = listed.get(root) {
            frontier.extend(
                entries
                    .iter()
                    .filter(|entry| browsable(entry))
                    .map(|entry| join(root, &entry.name)),
            );
        }
    }
    let mut budget = MAX_LISTINGS;
    for depth in 1..=MAX_DEPTH {
        let mut todo = Vec::new();
        for path in &frontier {
            if !listed.contains_key(path) && !todo.contains(path) && todo.len() < budget {
                todo.push(path.clone());
            }
        }
        budget -= todo.len();
        listed.extend(list_all(fs, todo).await);
        let mut next = Vec::new();
        for path in &frontier {
            let Some(Some(entries)) = listed.get(path) else {
                continue;
            };
            if has_file(entries, "level.dat") {
                if !found.contains(path) {
                    found.push(path.clone());
                }
            } else if depth < MAX_DEPTH {
                next.extend(
                    entries
                        .iter()
                        .filter(|entry| browsable(entry))
                        .map(|entry| join(path, &entry.name)),
                );
            }
        }
        frontier = next;
    }
    found.sort();
    found.truncate(MAX_FOUND);

    // (folder, added by hand)
    let mut candidates: Vec<(String, bool)> = Vec::new();
    let mut push = |path: &String, extra: bool| {
        if !candidates.iter().any(|(known, _)| known == path) {
            candidates.push((path.clone(), extra));
        }
    };
    if let Some(main) = &main {
        push(main, false);
    }
    for path in &siblings {
        if listed
            .get(path)
            .and_then(Option::as_ref)
            .is_some_and(|entries| has_file(entries, "level.dat"))
        {
            push(path, false);
        }
    }
    for path in &found {
        push(path, false);
    }
    for path in options.extra {
        push(path, true);
    }
    let candidates: Vec<(String, bool, Vec<Entry>)> = candidates
        .into_iter()
        .filter_map(|(path, extra)| {
            let entries = listed.get(&path)?.clone()?;
            Some((path, extra, entries))
        })
        .collect();
    let scanned: Vec<Result<World, L::Error>> = stream::iter(candidates)
        .map(|(path, extra, entries)| async move {
            let mut world = scan(fs, &path, &entries).await?;
            world.extra = extra;
            Ok::<_, L::Error>(world)
        })
        .buffered(CONCURRENCY)
        .collect()
        .await;
    let mut worlds = scanned.into_iter().collect::<Result<Vec<World>, L::Error>>()?;

    // fold the Paper/Spigot siblings into the main world
    if let Some(main) = &main
        && let Some(index) = worlds.iter().position(|world| world.name == *main)
    {
        let mut main_world = worlds.remove(index);
        main_world.main = true;
        worlds.retain(|world| {
            if world.extra || !is_sibling(main, world) {
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
    let others = usize::from(worlds.first().is_some_and(|world| world.main));
    worlds[others..].sort_by(|a, b| a.name.cmp(&b.name));

    // the keys `execute in` takes: the main world's dimensions by their id; on Bukkit, every
    // world in the container by its name, for the one dimension Bukkit loads from it
    for world in &mut worlds {
        if world.main {
            for dimension in &mut world.dimensions {
                dimension.key = valid_key(&dimension.id).then(|| dimension.id.clone());
            }
        } else if options.bukkit {
            let name = match container {
                Some(container) => world
                    .name
                    .strip_prefix(container)
                    .and_then(|rest| rest.strip_prefix('/')),
                None => Some(world.name.as_str()),
            };
            let key = name.and_then(bukkit_key);
            if let Some(dimension) = world.dimensions.first_mut() {
                dimension.key = key;
            }
        }
    }
    Ok(Detected {
        worlds,
        main_folder,
    })
}

/// Undoes the escapes of a Java properties key or value: `\t`, `\n`, `\r`, `\f`, `\uXXXX`
/// (surrogate pairs joined) and `\<any>` for the character itself.
fn unescape(text: &str) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let mut buffer = [0u16; 2];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let c = if c == '\\' {
            match chars.next() {
                None => break,
                Some('t') => '\t',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('f') => '\x0c',
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() == 4
                        && hex.chars().all(|c| c.is_ascii_hexdigit())
                        && let Ok(unit) = u16::from_str_radix(&hex, 16)
                    {
                        units.push(unit);
                    } else {
                        units.extend(format!("u{hex}").encode_utf16());
                    }
                    continue;
                }
                Some(other) => other,
            }
        } else {
            c
        };
        units.extend_from_slice(c.encode_utf16(&mut buffer));
    }
    String::from_utf16_lossy(&units)
}

const BLANK: [char; 3] = [' ', '\t', '\x0c'];

/// A logical line split into its raw key and value: the key ends at the first unescaped `=`,
/// `:` or blank, then blanks and one `=` or `:` are skipped.
fn split_entry(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut end = 0;
    let mut escaped = false;
    while end < bytes.len() {
        let byte = bytes[end];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if matches!(byte, b'=' | b':' | b' ' | b'\t' | 0x0c) {
            break;
        }
        end += 1;
    }
    let rest = line[end..].trim_start_matches(BLANK);
    let rest = match rest.strip_prefix(['=', ':']) {
        Some(value) => value.trim_start_matches(BLANK),
        None => rest,
    };
    (&line[..end], rest)
}

/// The value of `key` in a Java properties file, as `java.util.Properties` reads it: comment
/// lines (`#`, `!`), lines continued by an odd number of trailing backslashes, escapes, and the
/// last of repeated keys.
pub fn property(text: &str, key: &str) -> Option<String> {
    let mut value = None;
    let mut lines = text.lines();
    while let Some(first) = lines.next() {
        let mut line = first.trim_start_matches(BLANK).to_string();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        while line.bytes().rev().take_while(|&byte| byte == b'\\').count() % 2 == 1 {
            line.pop();
            match lines.next() {
                Some(next) => line.push_str(next.trim_start_matches(BLANK)),
                None => break,
            }
        }
        let (raw_key, raw_value) = split_entry(&line);
        if unescape(raw_key) == key {
            value = Some(unescape(raw_value));
        }
    }
    value
}

/// `level-name` from `server.properties`; `world` when the file or the key is missing or the
/// value empty. Not checked: see [`relative_path`].
pub fn level_name(properties: Option<&str>) -> String {
    properties
        .and_then(|text| property(text, "level-name"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "world".into())
}

/// A YAML scalar as plain text: quotes removed, or a trailing comment cut off.
fn yaml_scalar(raw: &str) -> &str {
    let raw = raw.trim();
    for quote in ['"', '\''] {
        if let Some(rest) = raw.strip_prefix(quote) {
            return rest.split(quote).next().unwrap_or("");
        }
    }
    raw.find(" #").map_or(raw, |end| &raw[..end]).trim_end()
}

/// Bukkit's world container (`settings.world-container` in `bukkit.yml`), when it is a folder
/// below the server root.
pub fn world_container(yaml: &str) -> Option<String> {
    let mut in_settings = false;
    for line in yaml.lines() {
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_settings = yaml_scalar(content) == "settings:";
            continue;
        }
        if in_settings && let Some(value) = content.strip_prefix("world-container:") {
            let value = yaml_scalar(value);
            if value.is_empty() || value == "~" || value == "null" {
                return None;
            }
            return relative_path(value);
        }
    }
    None
}

/// What a dimension file (`data/<namespace>/dimension/<path>.json`) says about its type.
#[derive(Debug, PartialEq, Eq)]
pub enum DimensionType {
    /// Inline or vanilla: whether it has a ceiling.
    Known(bool),
    /// A datapack's type, `data/<namespace>/dimension_type/<path>.json`.
    Named(String),
}

pub fn dimension_type(dimension: &Value) -> Option<DimensionType> {
    match dimension.get("type")? {
        Value::String(id) => {
            let id = if id.contains(':') {
                id.clone()
            } else {
                format!("minecraft:{id}")
            };
            Some(match id.as_str() {
                "minecraft:overworld" | "minecraft:the_end" => DimensionType::Known(false),
                "minecraft:overworld_caves" | "minecraft:the_nether" => DimensionType::Known(true),
                _ => DimensionType::Named(id),
            })
        }
        Value::Object(_) => Some(DimensionType::Known(has_ceiling(dimension.get("type")?))),
        _ => None,
    }
}

/// `has_ceiling` of a dimension type.
pub fn has_ceiling(dimension_type: &Value) -> bool {
    dimension_type
        .get("has_ceiling")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `file` (relative to a datapack) from the first of `packs` that has it, as JSON; files that
/// cannot be read count as missing.
async fn pack_json<L: Files>(fs: &L, packs: &[String], file: &str, reads: &mut usize) -> Option<Value> {
    for pack in packs {
        if *reads == 0 {
            return None;
        }
        *reads -= 1;
        if let Ok(Some(bytes)) = fs.read(&format!("{pack}/{file}"), PACK_FILE).await {
            return serde_json::from_slice(&bytes).ok();
        }
    }
    None
}

/// Whether the datapack dimensions `ids` have a ceiling, from the datapack folders in
/// `datapacks` (zipped datapacks and mods are not read: their dimensions count as open).
pub async fn ceilings<L: Files>(fs: &L, datapacks: &str, ids: &[String]) -> HashMap<String, bool> {
    let mut result = HashMap::new();
    if ids.is_empty() {
        return result;
    }
    let mut packs: Vec<String> = fs
        .list(datapacks)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.directory && !entry.name.starts_with('.'))
        .map(|entry| join(datapacks, &entry.name))
        .collect();
    packs.sort();
    packs.truncate(MAX_PACKS);
    let mut reads = MAX_PACK_READS;
    for id in ids {
        let Some((namespace, path)) = id.split_once(':') else {
            continue;
        };
        let file = format!("data/{namespace}/dimension/{path}.json");
        let kind = pack_json(fs, &packs, &file, &mut reads)
            .await
            .as_ref()
            .and_then(dimension_type);
        let ceiling = match kind {
            Some(DimensionType::Known(ceiling)) => ceiling,
            Some(DimensionType::Named(type_id)) => {
                let (type_namespace, type_path) = type_id
                    .split_once(':')
                    .unwrap_or(("minecraft", type_id.as_str()));
                let file = format!("data/{type_namespace}/dimension_type/{type_path}.json");
                pack_json(fs, &packs, &file, &mut reads)
                    .await
                    .is_some_and(|value| has_ceiling(&value))
            }
            None => false,
        };
        result.insert(id.clone(), ceiling);
    }
    result
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A file tree from paths: directories end with `/`, `name@<mtime>` sets a time.
    struct Tree {
        dirs: HashMap<String, Vec<Entry>>,
        files: HashMap<String, Vec<u8>>,
        listings: AtomicUsize,
    }

    impl Tree {
        fn new(paths: &[&str]) -> Self {
            let mut dirs: HashMap<String, Vec<Entry>> = HashMap::new();
            dirs.insert(String::new(), Vec::new());
            let mut files = HashMap::new();
            for path in paths {
                let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
                let mut parent = String::new();
                for (depth, part) in parts.iter().enumerate() {
                    let (name, modified) = match part.split_once('@') {
                        Some((name, time)) => (name, time.parse().unwrap()),
                        None => (*part, 0),
                    };
                    let directory = depth + 1 < parts.len() || path.ends_with('/');
                    let siblings = dirs.entry(parent.clone()).or_default();
                    if !siblings.iter().any(|entry| entry.name == name) {
                        siblings.push(Entry {
                            name: name.to_string(),
                            directory,
                            modified,
                        });
                    }
                    let here = join(&parent, name);
                    if directory {
                        dirs.entry(here.clone()).or_default();
                    } else {
                        files.insert(here.clone(), Vec::new());
                    }
                    parent = here;
                }
            }
            Self {
                dirs,
                files,
                listings: AtomicUsize::new(0),
            }
        }

        fn with(mut self, path: &str, content: &str) -> Self {
            self.files.insert(path.to_string(), content.as_bytes().to_vec());
            self
        }
    }

    #[async_trait::async_trait]
    impl Files for Tree {
        type Error = ();

        async fn list(&self, path: &str) -> Result<Option<Vec<Entry>>, ()> {
            self.listings.fetch_add(1, Ordering::Relaxed);
            Ok(self.dirs.get(path).cloned())
        }

        async fn read(&self, path: &str, _max: u64) -> Result<Option<Vec<u8>>, ()> {
            Ok(self.files.get(path).cloned())
        }
    }

    fn ids(world: &World) -> Vec<(&str, &str)> {
        world
            .dimensions
            .iter()
            .map(|d| (d.id.as_str(), d.folder.as_str()))
            .collect()
    }

    fn keys(world: &World) -> Vec<Option<&str>> {
        world.dimensions.iter().map(|d| d.key.as_deref()).collect()
    }

    fn names(worlds: &[World]) -> Vec<&str> {
        worlds.iter().map(|world| world.name.as_str()).collect()
    }

    async fn detect_with(tree: &Tree, options: Options<'_>) -> Detected {
        detect(tree, &options).await.unwrap()
    }

    async fn detect_tree(paths: &[&str], level: &str) -> Vec<World> {
        let options = Options {
            level_name: Some(level),
            ..Options::default()
        };
        detect_with(&Tree::new(paths), options).await.worlds
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
        assert_eq!(names(&worlds), vec!["world"]);
        assert!(worlds[0].main);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "world"),
                (NETHER, "world/DIM-1"),
                (END, "world/DIM1"),
            ]
        );
        assert_eq!(keys(&worlds[0]), vec![Some(OVERWORLD), Some(NETHER), Some(END)]);
        assert_eq!(
            worlds[0].dimensions.iter().map(|d| d.ceiling).collect::<Vec<_>>(),
            vec![false, true, false]
        );
        assert_eq!(worlds[0].players.as_deref(), Some("world/playerdata"));
    }

    const PAPER: &[&str] = &[
        "bukkit.yml",
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
        "creative_nether/level.dat",
        "creative_nether/DIM-1/region/",
    ];

    #[tokio::test]
    async fn paper_folders_become_dimensions_of_the_main_world() {
        let tree = Tree::new(PAPER);
        let options = Options {
            level_name: Some("world"),
            bukkit: true,
            ..Options::default()
        };
        let worlds = detect_with(&tree, options).await.worlds;
        assert_eq!(names(&worlds), vec!["world", "creative", "creative_nether"]);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "world"),
                (NETHER, "world_nether/DIM-1"),
                (END, "world_the_end/DIM1"),
            ]
        );
        assert_eq!(worlds[0].folders, vec!["world", "world_nether", "world_the_end"]);
        // Bukkit loads every other world under its own key
        assert_eq!(ids(&worlds[1]), vec![(OVERWORLD, "creative")]);
        assert_eq!(keys(&worlds[1]), vec![Some("minecraft:creative")]);
        assert_eq!(keys(&worlds[2]), vec![Some("minecraft:creative_nether")]);
        assert!(worlds[2].dimensions[0].ceiling);
        assert_eq!(worlds[1].players, None);

        // vanilla does not load other worlds: nothing to teleport to
        let vanilla = detect_with(
            &tree,
            Options {
                bukkit: false,
                ..options
            },
        )
        .await
        .worlds;
        assert_eq!(keys(&vanilla[1]), vec![None]);
    }

    #[tokio::test]
    async fn a_custom_level_name_is_the_main_world() {
        let worlds = detect_tree(
            &[
                "world/level.dat",
                "world/region/",
                "survival/level.dat",
                "survival/region/",
                "survival/playerdata/",
                "survival_nether/level.dat",
                "survival_nether/DIM-1/region/",
                "survival_the_end/level.dat",
                "survival_the_end/DIM1/region/",
            ],
            "survival",
        )
        .await;
        // the old `world` stays a world of its own, after the main one
        assert_eq!(names(&worlds), vec!["survival", "world"]);
        assert!(worlds[0].main && !worlds[1].main);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "survival"),
                (NETHER, "survival_nether/DIM-1"),
                (END, "survival_the_end/DIM1"),
            ]
        );
        assert_eq!(worlds[0].players.as_deref(), Some("survival/playerdata"));
    }

    #[tokio::test]
    async fn a_level_name_with_folders_and_spaces() {
        let worlds = detect_tree(
            &[
                "worlds/my survival/level.dat",
                "worlds/my survival/region/",
                "worlds/my survival_nether/level.dat",
                "worlds/my survival_nether/DIM-1/region/",
            ],
            "worlds/my survival",
        )
        .await;
        assert_eq!(names(&worlds), vec!["worlds/my survival"]);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "worlds/my survival"),
                (NETHER, "worlds/my survival_nether/DIM-1"),
            ]
        );
    }

    #[tokio::test]
    async fn worlds_in_the_bukkit_world_container() {
        let tree = Tree::new(&[
            "bukkit.yml",
            "worlds/world/level.dat",
            "worlds/world/region/",
            "worlds/world/playerdata/",
            "worlds/world_nether/level.dat",
            "worlds/world_nether/DIM-1/region/",
            "worlds/lobby/level.dat",
            "worlds/lobby/region/",
            "oldmap/level.dat",
            "oldmap/region/",
        ]);
        let detected = detect_with(
            &tree,
            Options {
                level_name: Some("world"),
                container: Some("worlds"),
                bukkit: true,
                ..Options::default()
            },
        )
        .await;
        assert_eq!(detected.main_folder.as_deref(), Some("worlds/world"));
        let worlds = detected.worlds;
        assert_eq!(names(&worlds), vec!["worlds/world", "oldmap", "worlds/lobby"]);
        assert_eq!(
            ids(&worlds[0]),
            vec![(OVERWORLD, "worlds/world"), (NETHER, "worlds/world_nether/DIM-1")]
        );
        assert_eq!(keys(&worlds[0]), vec![Some(OVERWORLD), Some(NETHER)]);
        // outside the container Bukkit cannot load it
        assert_eq!(keys(&worlds[1]), vec![None]);
        assert_eq!(keys(&worlds[2]), vec![Some("minecraft:lobby")]);
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
                "survival/dimensions/mypack/caves/deep/region/",
                "survival/dimensions/mypack/unused/data/",
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
                ("mypack:caves/deep", "survival/dimensions/mypack/caves/deep"),
                ("mypack:mining", "survival/dimensions/mypack/mining"),
            ]
        );
        assert_eq!(keys(&worlds[0])[3], Some("mypack:caves/deep"));
        assert_eq!(worlds[0].players.as_deref(), Some("survival/players/data"));
    }

    #[tokio::test]
    async fn paper_datapack_dimension_folders_join_the_main_world() {
        let worlds = detect_tree(
            &[
                "world/level.dat",
                "world/region/",
                "world_mining/level.dat",
                "world_mining/dimensions/mypack/mining/region/",
                "world_backup/level.dat",
                "world_backup/region/",
            ],
            "world",
        )
        .await;
        assert_eq!(names(&worlds), vec!["world", "world_backup"]);
        assert_eq!(
            ids(&worlds[0]),
            vec![
                (OVERWORLD, "world"),
                ("mypack:mining", "world_mining/dimensions/mypack/mining"),
            ]
        );
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
    async fn looks_two_folders_deep_and_skips_what_is_not_a_world() {
        let tree = Tree::new(&[
            "world/level.dat",
            "world/region/",
            "maps/creative/level.dat",
            "maps/creative/region/",
            "a/b/c/level.dat",
            "a/b/c/region/",
            "plugins/Multiverse-Core/world/level.dat",
            "plugins/Multiverse-Core/world/region/",
            "backups/world/level.dat",
            "backups/world/region/",
            ".git/level.dat",
            "backup_world/region/",
            "empty/level.dat",
        ]);
        let options = Options {
            level_name: Some("world"),
            ..Options::default()
        };
        let worlds = detect_with(&tree, options).await.worlds;
        assert_eq!(names(&worlds), vec!["world", "maps/creative"]);

        // folders added by hand are read wherever they are; missing ones and files are skipped
        let extra = [
            "backups/world".to_string(),
            "a/b/c".to_string(),
            "missing".to_string(),
            "world/level.dat".to_string(),
        ];
        let worlds = detect_with(
            &tree,
            Options {
                extra: &extra,
                ..options
            },
        )
        .await
        .worlds;
        assert_eq!(
            names(&worlds),
            vec!["world", "a/b/c", "backups/world", "maps/creative"]
        );
        assert!(worlds[1].extra && worlds[2].extra && !worlds[3].extra);
    }

    #[tokio::test]
    async fn listing_stays_bounded() {
        let paths: Vec<String> = (0..300)
            .flat_map(|i| [format!("dir{i}/a/"), format!("dir{i}/b/")])
            .chain(["world/level.dat".into(), "world/region/".into()])
            .collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let tree = Tree::new(&paths);
        let worlds = detect_with(
            &tree,
            Options {
                level_name: Some("world"),
                ..Options::default()
            },
        )
        .await
        .worlds;
        assert_eq!(names(&worlds), vec!["world"]);
        assert!(tree.listings.load(Ordering::Relaxed) <= MAX_LISTINGS + 8);
    }

    #[tokio::test]
    async fn tells_a_missing_level_from_one_not_generated_yet() {
        let tree = Tree::new(&["world/level.dat", "world/region/", "survival/logs/"]);
        let missing = detect_with(
            &tree,
            Options {
                level_name: Some("lobby"),
                ..Options::default()
            },
        )
        .await;
        assert_eq!(missing.main_folder, None);
        assert_eq!(names(&missing.worlds), vec!["world"]);
        assert!(!missing.worlds[0].main);

        let empty = detect_with(
            &tree,
            Options {
                level_name: Some("survival"),
                ..Options::default()
            },
        )
        .await;
        assert_eq!(empty.main_folder.as_deref(), Some("survival"));
        assert!(empty.worlds.iter().all(|world| !world.main));
    }

    #[tokio::test]
    async fn dimension_keys_map_both_ways() {
        let tree = Tree::new(PAPER);
        let worlds = detect_with(
            &tree,
            Options {
                level_name: Some("world"),
                bukkit: true,
                ..Options::default()
            },
        )
        .await
        .worlds;
        for world in &worlds {
            for dimension in &world.dimensions {
                let key = dimension.key.as_deref().unwrap();
                let (found_world, found) = by_key(&worlds, key).unwrap();
                assert_eq!((found_world.name.as_str(), found.folder.as_str()), (world.name.as_str(), dimension.folder.as_str()));
            }
        }
        let (world, dimension) = by_key(&worlds, NETHER).unwrap();
        assert_eq!((world.name.as_str(), dimension.folder.as_str()), ("world", "world_nether/DIM-1"));
        assert_eq!(by_key(&worlds, "minecraft:creative").unwrap().0.name, "creative");
        assert!(by_key(&worlds, "minecraft:lobby").is_none());

        assert_eq!(bukkit_key("Lobby").as_deref(), Some("minecraft:lobby"));
        assert_eq!(bukkit_key("maps/arena").as_deref(), Some("minecraft:maps/arena"));
        assert_eq!(bukkit_key("my world"), None);
        assert!(valid_key("minecraft:overworld"));
        assert!(valid_key("my_pack:caves/deep"));
        assert!(!valid_key("minecraft"));
        assert!(!valid_key("minecraft:over world"));
        assert!(!valid_key("Minecraft:Overworld"));
        assert!(!valid_key("a:b\nop Steve"));
    }

    #[test]
    fn reads_the_level_name() {
        assert_eq!(level_name(None), "world");
        assert_eq!(level_name(Some("motd=x\n")), "world");
        assert_eq!(level_name(Some("motd=x\nlevel-name=survival\n")), "survival");
        assert_eq!(level_name(Some("level-name = my\\ world")), "my world");
        assert_eq!(level_name(Some("level-name my world ")), "my world ");
        assert_eq!(level_name(Some("level-name:worlds/main")), "worlds/main");
        assert_eq!(level_name(Some("level-name=C\\:\\\\maps")), "C:\\maps");
        assert_eq!(level_name(Some("level-name=caf\\u00e9")), "café");
        assert_eq!(level_name(Some("level-name=\\uD83C\\uDF0D")), "\u{1F30D}");
        assert_eq!(level_name(Some("level-name=bad\\u00zz")), "badu00zz");
        assert_eq!(level_name(Some("level-name=a\\tb")), "a\tb");
        assert_eq!(level_name(Some("level-name=sur\\\n    vival")), "survival");
        assert_eq!(level_name(Some("level-name=x\\\\\nmotd=y")), "x\\");
        assert_eq!(level_name(Some("#level-name=old\n! level-name=old\nlevel-name=new")), "new");
        assert_eq!(level_name(Some("level-name=one\r\nlevel-name=two\r\n")), "two");
        assert_eq!(level_name(Some("level\\-name=escaped key")), "escaped key");
        assert_eq!(level_name(Some("level-name=")), "world");
        assert_eq!(level_name(Some("level-name-old=x")), "world");
    }

    #[test]
    fn folder_paths_stay_inside_the_server() {
        assert_eq!(relative_path("worlds/survival").as_deref(), Some("worlds/survival"));
        assert_eq!(relative_path("./my world/").as_deref(), Some("my world"));
        for bad in ["", ".", "./", "/", "/etc", "..", "../x", "a/../b", "a//b", "a/./b", "a\\b", "a\nb"] {
            assert_eq!(relative_path(bad), None, "{bad:?}");
        }
        assert_eq!(relative_path(&"x".repeat(256)), None);
    }

    #[test]
    fn reads_the_world_container() {
        let yaml = "settings:\n  allow-end: true\n  world-container: worlds # all of them\nspawn-limits:\n  monsters: 70\n";
        assert_eq!(world_container(yaml).as_deref(), Some("worlds"));
        assert_eq!(world_container("settings:\n  world-container: 'my worlds'\n").as_deref(), Some("my worlds"));
        assert_eq!(world_container("settings:\n  allow-end: true\n").as_deref(), None);
        assert_eq!(world_container("other:\n  world-container: x\n").as_deref(), None);
        assert_eq!(world_container("settings:\n  world-container: ../x\n").as_deref(), None);
        assert_eq!(world_container("settings:\n  world-container: ~\n").as_deref(), None);
    }

    #[tokio::test]
    async fn datapack_dimensions_with_a_ceiling() {
        let tree = Tree::new(&[
            "world/datapacks/a/data/mypack/dimension/caves.json",
            "world/datapacks/a/data/mypack/dimension/open.json",
            "world/datapacks/b/data/mypack/dimension/hell.json",
            "world/datapacks/b/data/mypack/dimension_type/cavey.json",
            "world/datapacks/b/data/other/dimension/sky.json",
            "world/datapacks/zipped.zip",
        ])
        .with(
            "world/datapacks/a/data/mypack/dimension/caves.json",
            r#"{"type": "mypack:cavey", "generator": {}}"#,
        )
        .with(
            "world/datapacks/b/data/mypack/dimension_type/cavey.json",
            r#"{"has_ceiling": true, "height": 256}"#,
        )
        .with(
            "world/datapacks/a/data/mypack/dimension/open.json",
            r#"{"type": {"has_ceiling": false}}"#,
        )
        .with(
            "world/datapacks/b/data/mypack/dimension/hell.json",
            r#"{"type": "the_nether"}"#,
        )
        .with("world/datapacks/b/data/other/dimension/sky.json", "not json");
        let ids: Vec<String> = ["mypack:caves", "mypack:open", "mypack:hell", "other:sky", "mod:unknown"]
            .map(String::from)
            .to_vec();
        let found = ceilings(&tree, "world/datapacks", &ids).await;
        assert_eq!(found["mypack:caves"], true);
        assert_eq!(found["mypack:open"], false);
        assert_eq!(found["mypack:hell"], true);
        assert_eq!(found["other:sky"], false);
        assert_eq!(found["mod:unknown"], false);
    }
}
