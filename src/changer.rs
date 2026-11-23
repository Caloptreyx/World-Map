//! MCA Selector's "Change NBT fields": the change syntax (`Status = full, InhabitedTime = 0`),
//! its normalized form, and applying each field to a chunk's region/, entities/ and poi/ NBT.
//!
//! Fields, values and per-version behaviour follow MCA Selector 2.9's `changer` package and its
//! `ChunkFilter_*` implementations: 1.13 to 1.17 chunks (`Level.*`), the 21w37a to 21w42a
//! snapshots (`Level.Sections` with `block_states`) and 1.18+ chunks (root tags), including 26.x
//! palettes of plain strings. Fields that do not apply to a chunk's version leave it untouched,
//! as MCA Selector does. Block, heightmap, biome, status and structure tables are generated from
//! MCA Selector's resources by `scripts/make_changer_data.py`.
//!
//! Where MCA Selector 2.9 has bugs, World Map does what the field is meant to do:
//! - DeleteEntities (and the entity part of DeleteSections) clear 1.17+ entities/ chunks and
//!   1.18+ proto-entities; MCA Selector looks them up under `Level` and deletes nothing there.
//! - FixHeightmaps and ReplaceBlocks write 1.18+ heightmaps (MCA Selector's writer only finds
//!   `Level`); single-valued 1.18+ sections (no `data`) count, LIGHT_BLOCKING in 18w43a to 18w44a
//!   uses the 1.13 block list and 26.3 configs keep the leaves list of 26.2 (MCA Selector's are
//!   empty there). DeleteSections keeps 1.18+ heightmaps as MCA Selector does: the chunk goes
//!   back to `features`, which recomputes them.
//! - `isLightOn` / `LightPopulated` are written as bytes (MCA Selector writes longs before 1.18).
//! - ReplaceBlocks only rewrites sections where a block was replaced (MCA Selector also adds air
//!   to the palette of untouched sections), removes a block entity before placing the
//!   replacement's (MCA Selector adds a second one at the same position), writes 1.18+ palettes
//!   of one entry without `data`, matches `from` by exact name (MCA Selector uses it as a
//!   regular expression) and reads quoted target names up to their closing quote.
//! - Biome changes write single-entry 1.18+ biome palettes without `data`; legacy biome arrays
//!   keep ids above 127 (MCA Selector casts them to a byte).
//! - PreventRetrogen and ForceBlend only touch 1.18+ chunks (the only ones with
//!   `below_zero_retrogen` / blending); DeleteSections on 1.18+ also accepts statuses without
//!   namespace; FixStatus reads 1.18+ root sections.
//! - Timestamp sets the region/ header timestamp only; Compression the region/ chunk only.
//! - InhabitedTime and LastUpdate also take durations ("1 hour 30 minutes", 20 ticks a second).
//! - Chunks saved before 1.13 (numeric block ids) get no ReplaceBlocks, Biome or heightmap
//!   changes (MCA Selector's 1.9 to 1.12 block code is not ported), 1.13 snapshots before 18w19a
//!   no heightmap changes; SNBT numbers in ReplaceBlocks are decimal (no `0x`/`0b` literals).
use crate::{
    chunk::{bits_for, unpack},
    filter::{Needs, ParseError},
    nbt::{Compound, Tag},
};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

/// One chunk position while changes are applied: its NBT in each file (None: no chunk there or
/// the file is not needed) and its region/ header entry.
pub struct ChunkMut<'a> {
    pub x: i32,
    pub z: i32,
    pub region: Option<&'a mut Compound>,
    pub entities: Option<&'a mut Compound>,
    pub poi: Option<&'a mut Compound>,
    /// region/ header timestamp.
    pub timestamp: &'a mut u32,
    /// region/ compression type to write with.
    pub compression: &'a mut u8,
}

/// What applying changes modified. `region` is also set when only the compression type changed
/// (the chunk has to be encoded again); `header` means the region/ timestamp changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub region: bool,
    pub entities: bool,
    pub poi: bool,
    pub header: bool,
}

/// Parsed change fields, in the order they were given (and are applied).
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeSet {
    fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
enum Field {
    DataVersion(i32),
    InhabitedTime(i64),
    Timestamp(i32),
    Compression(u8),
    LastUpdate(i64),
    Status(Status),
    Biome(Biome),
    ReplaceBlocks(Vec<(String, Replacement)>),
    DeleteEntities,
    DeleteSections(Vec<Range>),
    FixStatus,
    FixHeightmaps,
    DeleteStructureReference(Vec<String>),
    PreventRetrogen,
    ForceBlend,
    RemoveLightingInfo,
    LightPopulated(i8),
}

/// MCA Selector field names in its order, the ones World Map applies.
const FIELDS: &[&str] = &[
    "DataVersion",
    "InhabitedTime",
    "Timestamp",
    "Compression",
    "LastUpdate",
    "Status",
    "Biome",
    "ReplaceBlocks",
    "DeleteEntities",
    "DeleteSections",
    "FixStatus",
    "FixHeightmaps",
    "DeleteStructureReference",
    "PreventRetrogen",
    "ForceBlend",
    "RemoveLightingInfo",
    "LightPopulated",
];

/// MCA Selector fields World Map refuses, with the reason.
const UNAVAILABLE: &[(&str, &str)] = &[
    ("Custom", "it runs a Groovy script on the chunk's NBT"),
    (
        "FixStructureReferences",
        "MCA Selector offers it only in its command line mode",
    ),
    ("Script", "it runs a Groovy script"),
];

/// A chunk status; `custom` ones were given quoted and are written as they are.
#[derive(Clone, Debug, PartialEq)]
struct Status {
    name: String,
    custom: bool,
}

impl Status {
    /// The value to store: chunks before 23w12a (DataVersion 3207) store statuses without namespace.
    fn value(&self, data_version: i64) -> String {
        if self.custom || data_version < 3207 {
            self.name.clone()
        } else {
            format!("minecraft:{}", self.name)
        }
    }

    fn display(&self) -> String {
        if self.custom {
            format!("'{}'", self.name)
        } else {
            format!("minecraft:{}", self.name)
        }
    }
}

/// A biome by name (1.18+ palettes) and legacy numeric id (`Biomes` int arrays); either may be
/// unknown, then chunks needing it are skipped.
#[derive(Clone, Debug, PartialEq)]
struct Biome {
    name: Option<String>,
    id: Option<i32>,
}

impl Biome {
    /// MCA Selector's `BiomeIdentifier.toString`.
    fn display(&self) -> String {
        let registry = registry();
        match (&self.name, self.id) {
            (Some(name), _) if registry.biomes.get(name.as_str()).is_some_and(|b| b.1) => {
                name.clone()
            }
            (Some(name), _) => format!("'{name}'"),
            (None, Some(id)) if registry.biome_ids.contains_key(&id) => id.to_string(),
            (None, Some(id)) => format!("'{id}'"),
            (None, None) => String::new(),
        }
    }
}

/// What a block is replaced with, and the block entity placed with it.
#[derive(Clone, Debug, PartialEq)]
struct Replacement {
    block: Block,
    tile: Option<Compound>,
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Name(String),
    /// A full block state as SNBT (`{Name: "minecraft:oak_log", Properties: {axis: "x"}}`).
    State(Compound),
}

impl Replacement {
    /// The palette entry: `{Name}` before 26.3-snapshot-7 (DataVersion 5009), a plain string since.
    fn state(&self, data_version: i64) -> Tag {
        match &self.block {
            Block::State(state) => Tag::Compound(state.clone()),
            Block::Name(name) if data_version >= 5009 => Tag::String(name.clone()),
            Block::Name(name) => named("Name", Tag::String(name.clone())),
        }
    }

    fn display(&self) -> String {
        let mut out = match &self.block {
            Block::Name(name) if name.starts_with("minecraft:") => name.clone(),
            Block::Name(name) => format!("'{name}'"),
            Block::State(state) => snbt::write(state),
        };
        if let Some(tile) = &self.tile {
            out.push(';');
            out.push_str(&snbt::write(tile));
        }
        out
    }
}

/// An inclusive section range; open ends are `i32::MIN` / `i32::MAX`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Range {
    from: i32,
    to: i32,
}

impl Range {
    fn contains(&self, value: i64) -> bool {
        self.from as i64 <= value && value <= self.to as i64
    }

    fn is_all(&self) -> bool {
        self.from == i32::MIN && self.to == i32::MAX
    }

    fn display(&self) -> String {
        if self.from == self.to {
            return self.from.to_string();
        }
        let from = if self.from == i32::MIN {
            String::new()
        } else {
            self.from.to_string()
        };
        let to = if self.to == i32::MAX {
            String::new()
        } else {
            self.to.to_string()
        };
        format!("{from}:{to}")
    }
}

// ---------------------------------------------------------------------------------------------
// Data tables

struct BlockTable {
    /// The DataVersion each heightmap config column applies from, ascending.
    eras: Vec<i64>,
    /// Block name without `minecraft:` -> (heightmap bits per era, in MCA Selector's registry).
    blocks: HashMap<&'static str, (Vec<u8>, bool)>,
}

/// Parsed `data/heightmaps.tsv`.
static BLOCKS: LazyLock<BlockTable> = LazyLock::new(|| {
    let mut eras = Vec::new();
    let mut blocks = HashMap::new();
    for line in include_str!("data/heightmaps.tsv").lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let (Some(name), Some(second)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name == "eras" {
            eras = std::iter::once(second)
                .chain(parts)
                .filter_map(|era| era.parse().ok())
                .collect();
            continue;
        }
        let masks = second
            .chars()
            .map(|digit| digit.to_digit(32).unwrap_or(0) as u8)
            .collect();
        blocks.insert(name, (masks, parts.next() == Some("r")));
    }
    BlockTable { eras, blocks }
});

fn block_table() -> &'static BlockTable {
    &BLOCKS
}

/// Whether MCA Selector's block registry knows a (namespaced) block name.
fn valid_block(name: &str) -> bool {
    name.strip_prefix("minecraft:")
        .and_then(|name| block_table().blocks.get(name))
        .is_some_and(|block| block.1)
}

struct Registry {
    statuses: HashSet<&'static str>,
    /// Name -> (legacy id, valid biome name).
    biomes: HashMap<&'static str, (Option<i32>, bool)>,
    biome_ids: HashMap<i32, &'static str>,
    /// Structure name -> the names deleted with it.
    structures: HashMap<&'static str, Vec<&'static str>>,
}

/// Parsed `data/changer_registry.tsv`.
static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    let mut registry = Registry {
        statuses: HashSet::new(),
        biomes: HashMap::new(),
        biome_ids: HashMap::new(),
        structures: HashMap::new(),
    };
    for line in include_str!("data/changer_registry.tsv").lines() {
        if line.starts_with('#') {
            continue;
        }
        let parts: Vec<&'static str> = line.split('\t').collect();
        match *parts.as_slice() {
            ["status", name] => {
                registry.statuses.insert(name);
            }
            ["biome", name, id, ref rest @ ..] => {
                let id = id.parse().ok();
                if let Some(id) = id {
                    registry.biome_ids.insert(id, name);
                }
                registry
                    .biomes
                    .insert(name, (id, rest.first() == Some(&"v")));
            }
            ["structure", name, alternatives] => {
                registry
                    .structures
                    .insert(name, alternatives.split(',').collect());
            }
            _ => {}
        }
    }
    registry
});

fn registry() -> &'static Registry {
    &REGISTRY
}

// ---------------------------------------------------------------------------------------------
// Parsing

/// Reads change text the way MCA Selector's `ChangeParser` does.
struct Pointer<'a> {
    chars: &'a [char],
    index: usize,
}

impl Pointer<'_> {
    fn current(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.current().is_some_and(char::is_whitespace) {
            self.index += 1;
        }
    }

    fn simple(&mut self) -> String {
        let start = self.index;
        while self
            .current()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
        {
            self.index += 1;
        }
        self.chars[start..self.index].iter().collect()
    }

    /// A `"` quoted value; only `\\` and `\"` are escapes.
    fn quoted(&mut self) -> Result<String, ParseError> {
        self.index += 1;
        let mut value = String::new();
        let mut escape = false;
        while let Some(c) = self.current() {
            self.index += 1;
            if escape {
                if c != '\\' && c != '"' {
                    return Err(self.error(format!("invalid escape of '{c}'")));
                }
                value.push(c);
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                return Ok(value);
            } else {
                value.push(c);
            }
        }
        Err(self.error("missing end quote"))
    }

    fn expect(&mut self, expected: char) -> Result<(), ParseError> {
        self.skip_whitespace();
        match self.current() {
            Some(c) if c == expected => {
                self.index += 1;
                Ok(())
            }
            Some(c) => Err(self.error(format!("expected '{expected}' but got '{c}'"))),
            None => Err(self.error(format!("expected '{expected}' but got EOF"))),
        }
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            position: self.index,
        }
    }
}

/// Parses MCA Selector change syntax: `Field = value, Field = "quoted value", ...`. Positions in
/// errors count characters.
pub fn parse(text: &str) -> Result<ChangeSet, ParseError> {
    let chars: Vec<char> = text.chars().collect();
    let mut pointer = Pointer {
        chars: &chars,
        index: 0,
    };
    let mut fields = Vec::new();
    let mut separated = false;
    loop {
        pointer.skip_whitespace();
        if pointer.current().is_none() {
            if separated {
                return Err(pointer.error("expected a field after ','"));
            }
            break;
        }
        let key = pointer.simple();
        if let Some((_, reason)) = UNAVAILABLE.iter().find(|(name, _)| *name == key) {
            return Err(pointer.error(format!("{key} is not available in World Map: {reason}")));
        }
        if !FIELDS.contains(&key.as_str()) {
            return Err(pointer.error("invalid field"));
        }
        pointer.expect('=')?;
        pointer.skip_whitespace();
        let value = match pointer.current() {
            Some('"') => pointer.quoted()?,
            Some(_) => pointer.simple(),
            None => return Err(pointer.error(format!("missing value for {key}"))),
        };
        let field = parse_value(&key, &value)
            .map_err(|reason| pointer.error(format!("invalid value for {key}: {reason}")))?;
        fields.push(field);
        pointer.skip_whitespace();
        separated = pointer.current().is_some();
        if separated {
            pointer.expect(',')?;
        }
    }
    Ok(ChangeSet { fields })
}

fn parse_value(key: &str, value: &str) -> Result<Field, String> {
    let boolean = || match value {
        "1" | "true" => Ok(()),
        _ => Err("expected true or 1".to_string()),
    };
    Ok(match key {
        "DataVersion" => Field::DataVersion(
            value
                .bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| value.parse().ok())
                .flatten()
                .ok_or("expected a data version (a whole number)")?,
        ),
        "InhabitedTime" => Field::InhabitedTime(ticks(value)?),
        "LastUpdate" => Field::LastUpdate(ticks(value)?),
        "Timestamp" => Field::Timestamp(
            value
                .parse()
                .map_err(|_| "expected seconds since 1970 (a whole number)")?,
        ),
        "Compression" => Field::Compression(compression(value)?),
        "Status" => Field::Status(status(value)?),
        "Biome" => Field::Biome(biome(value)?),
        "ReplaceBlocks" => Field::ReplaceBlocks(replacements(value)?),
        "DeleteEntities" => boolean().map(|()| Field::DeleteEntities)?,
        "DeleteSections" => Field::DeleteSections(ranges(value)?),
        "FixStatus" => boolean().map(|()| Field::FixStatus)?,
        "FixHeightmaps" => boolean().map(|()| Field::FixHeightmaps)?,
        "DeleteStructureReference" => Field::DeleteStructureReference(structures(value)?),
        "PreventRetrogen" => boolean().map(|()| Field::PreventRetrogen)?,
        "ForceBlend" => boolean().map(|()| Field::ForceBlend)?,
        "RemoveLightingInfo" => boolean().map(|()| Field::RemoveLightingInfo)?,
        "LightPopulated" => Field::LightPopulated(match value {
            "0" => 0,
            "1" => 1,
            _ => return Err("expected 0 or 1".into()),
        }),
        _ => return Err("unknown field".into()),
    })
}

/// Game ticks: a whole number (MCA Selector) or a duration such as `1 hour 30 minutes` (as its
/// InhabitedTime filter reads them; 20 ticks per second).
fn ticks(value: &str) -> Result<i64, String> {
    value
        .parse()
        .ok()
        .or_else(|| duration_ticks(value))
        .ok_or_else(|| "expected ticks or a duration such as \"2 hours 5 minutes\"".into())
}

/// MCA Selector's `TextHelper.parseDuration` in ticks: numbers with a unit each (years, months,
/// days, hours, minutes, seconds and their short forms), every unit at most once.
fn duration_ticks(value: &str) -> Option<i64> {
    const UNITS: [(&[&str], i64); 6] = [
        (&["years", "year", "y"], 31_536_000),
        (&["months", "month"], 2_592_000),
        (&["days", "day", "d"], 86_400),
        (&["hours", "hour", "h"], 3600),
        (&["minutes", "minute", "mins", "min"], 60),
        (&["seconds", "second", "secs", "sec", "s"], 1),
    ];
    let text = value.trim();
    let bytes = text.as_bytes();
    let mut seen = [false; UNITS.len()];
    let mut total: i64 = 0;
    let mut index = 0;
    let mut any = false;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let number: i64 = text[start..index].parse().ok()?;
        while index < bytes.len() && !bytes[index].is_ascii_alphanumeric() && bytes[index] != b'_' {
            index += 1;
        }
        let unit_start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        let unit = &text[unit_start..index];
        let kind = UNITS.iter().position(|(names, _)| names.contains(&unit))?;
        if std::mem::replace(&mut seen[kind], true) {
            return None;
        }
        total = total.checked_add(number.checked_mul(UNITS[kind].1)?)?;
        any = true;
    }
    if any { total.checked_mul(20) } else { None }
}

fn compression(value: &str) -> Result<u8, String> {
    match value.to_ascii_uppercase().as_str() {
        "GZIP" | "1" => Ok(1),
        "ZLIB" | "2" => Ok(2),
        "UNCOMPRESSED" | "3" => Ok(3),
        "LZ4" | "4" => Ok(4),
        "NONE" | "0" => Err("NONE marks a missing chunk".into()),
        "NONE_EXT" | "GZIP_EXT" | "ZLIB_EXT" | "UNCOMPRESSED_EXT" | "LZ4_EXT" | "-128" | "-127"
        | "-126" | "-125" | "-124" => {
            Err("external .mcc storage is chosen automatically for chunks over 1 MiB".into())
        }
        _ => Err("expected GZIP, ZLIB, UNCOMPRESSED or LZ4 (or 1 to 4)".into()),
    }
}

fn compression_name(compression: u8) -> &'static str {
    match compression {
        1 => "GZIP",
        2 => "ZLIB",
        3 => "UNCOMPRESSED",
        _ => "LZ4",
    }
}

fn is_quoted(value: &str) -> bool {
    value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'')
}

/// `full`, `minecraft:full` or a custom `'status'`.
fn status(value: &str) -> Result<Status, String> {
    if is_quoted(value) {
        return Ok(Status {
            name: value[1..value.len() - 1].to_string(),
            custom: true,
        });
    }
    let name = value.strip_prefix("minecraft:").unwrap_or(value);
    if registry().statuses.contains(name) {
        Ok(Status {
            name: name.to_string(),
            custom: false,
        })
    } else {
        Err(format!(
            "unknown status {value} (quote custom ones: 'name')"
        ))
    }
}

/// MCA Selector's `BiomeField.parseNewValue`: a name, a legacy numeric id, or either quoted to
/// skip validation.
fn biome(value: &str) -> Result<Biome, String> {
    let registry = registry();
    let mut low = value.to_lowercase();
    let mut quoted = false;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if is_quoted(&low) {
        low = low[1..low.len() - 1].to_string();
        quoted = true;
    } else if !digits(&low) && !low.starts_with("minecraft:") {
        low = format!("minecraft:{low}");
    }
    if digits(&low) {
        if let Ok(id) = low.parse::<i32>()
            && (quoted || registry.biome_ids.contains_key(&id))
        {
            return Ok(Biome {
                name: registry.biome_ids.get(&id).map(|name| name.to_string()),
                id: Some(id),
            });
        }
    } else if quoted || registry.biomes.get(low.as_str()).is_some_and(|b| b.1) {
        let id = registry.biomes.get(low.as_str()).and_then(|b| b.0);
        return Ok(Biome {
            name: Some(low),
            id,
        });
    }
    Err(format!("unknown biome {value} (quote custom ones: 'name')"))
}

fn trim(mut chars: &[char]) -> &[char] {
    while chars.first().is_some_and(|c| c.is_whitespace()) {
        chars = &chars[1..];
    }
    while chars.last().is_some_and(|c| c.is_whitespace()) {
        chars = &chars[..chars.len() - 1];
    }
    chars
}

/// MCA Selector's `ReplaceBlocksField.parseNewValue`: `<from>=<to>[;<tile entity SNBT>]`, comma
/// separated. `from` is a block name (namespace optional) or a quoted custom name; `to` is a
/// block name, a quoted custom name or a block state as SNBT.
fn replacements(value: &str) -> Result<Vec<(String, Replacement)>, String> {
    let chars: Vec<char> = value.chars().collect();
    let mut rest = trim(&chars);
    let mut entries: Vec<(String, Replacement)> = Vec::new();
    while !rest.is_empty() {
        let equals = rest
            .iter()
            .position(|&c| c == '=')
            .ok_or("expected <from>=<to>")?;
        let raw_from: String = trim(&rest[..equals]).iter().collect();
        let from = if raw_from.len() > 2 && is_quoted(&raw_from) {
            raw_from[1..raw_from.len() - 1].to_string()
        } else if raw_from.starts_with("minecraft:") {
            raw_from
        } else {
            let name = format!("minecraft:{raw_from}");
            if !valid_block(&name) {
                return Err(format!(
                    "unknown block {raw_from} (quote custom ones: 'name')"
                ));
            }
            name
        };

        let mut to = trim(&rest[equals + 1..]);
        let (block, read) = match to.first() {
            Some('{') => {
                let (state, read) = snbt::parse_compound(to)?;
                (Block::State(state), read)
            }
            Some('\'') => {
                let close = to[1..]
                    .iter()
                    .position(|&c| c == '\'')
                    .ok_or("missing closing ' of a block name")?
                    + 1;
                let name: String = to[1..close].iter().collect();
                if name.is_empty() {
                    return Err("empty block name".into());
                }
                (Block::Name(name), close + 1)
            }
            _ => {
                let end = to
                    .iter()
                    .position(|&c| c == ',' || c == ';')
                    .unwrap_or(to.len());
                let raw: String = to[..end].iter().collect();
                let name = if raw.starts_with("minecraft:") {
                    raw.clone()
                } else {
                    format!("minecraft:{raw}")
                };
                if !valid_block(&name) {
                    return Err(format!("unknown block {raw} (quote custom ones: 'name')"));
                }
                (Block::Name(name), end)
            }
        };
        to = trim(&to[read..]);
        let mut tile = None;
        if to.first() == Some(&';') {
            to = trim(&to[1..]);
            if to.first() != Some(&'{') {
                return Err("expected a block entity compound after ';'".into());
            }
            let (compound, read) = snbt::parse_compound(to)?;
            tile = Some(compound);
            to = trim(&to[read..]);
        }
        let replacement = Replacement { block, tile };
        match entries.iter_mut().find(|(name, _)| *name == from) {
            Some(entry) => entry.1 = replacement,
            None => entries.push((from, replacement)),
        }
        match to.first() {
            Some(',') => rest = trim(&to[1..]),
            Some(_) => return Err("expected ',' between replacements".into()),
            None => break,
        }
    }
    if entries.is_empty() {
        return Err("expected <from>=<to>".into());
    }
    Ok(entries)
}

/// MCA Selector's `RangeParser.parseRanges(value, ",")`: `true` for all sections, else ranges
/// such as `-4:3, 10, :0, 5:`.
fn ranges(value: &str) -> Result<Vec<Range>, String> {
    if value.trim() == "true" {
        return Ok(vec![Range {
            from: i32::MIN,
            to: i32::MAX,
        }]);
    }
    let mut parts: Vec<&str> = value.split(',').collect();
    // Java's split drops trailing empty strings
    while parts.len() > 1 && parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    parts
        .iter()
        .map(|part| range(part).ok_or_else(|| format!("invalid section range \"{}\"", part.trim())))
        .collect()
}

fn range(text: &str) -> Option<Range> {
    let text = text.trim();
    let number = |s: &str, open: i32| -> Option<i32> {
        if s.is_empty() {
            return Some(open);
        }
        let digits = s.strip_prefix('-').unwrap_or(s);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if text.is_empty() {
        return None;
    }
    match text.split_once(':') {
        Some((from, to)) => Some(Range {
            from: number(from, i32::MIN)?,
            to: number(to, i32::MAX)?,
        }),
        None => {
            let value = number(text, 0)?;
            Some(Range {
                from: value,
                to: value,
            })
        }
    }
}

/// MCA Selector's `DeleteStructureField.parseNewValue`: known structure names or quoted ones.
fn structures(value: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.len() > 2 && is_quoted(part) {
            names.push(part[1..part.len() - 1].to_string());
        } else if registry().structures.contains_key(part) {
            names.push(part.to_string());
        } else {
            return Err(format!(
                "unknown structure {part} (quote custom ones: 'name')"
            ));
        }
    }
    Ok(names)
}

/// A value as MCA Selector's parser reads it back: bare when it is a simple string, else quoted.
fn value_text(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
    {
        value.to_string()
    } else {
        quote(value)
    }
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Field {
    fn name(&self) -> &'static str {
        match self {
            Self::DataVersion(_) => "DataVersion",
            Self::InhabitedTime(_) => "InhabitedTime",
            Self::Timestamp(_) => "Timestamp",
            Self::Compression(_) => "Compression",
            Self::LastUpdate(_) => "LastUpdate",
            Self::Status(_) => "Status",
            Self::Biome(_) => "Biome",
            Self::ReplaceBlocks(_) => "ReplaceBlocks",
            Self::DeleteEntities => "DeleteEntities",
            Self::DeleteSections(_) => "DeleteSections",
            Self::FixStatus => "FixStatus",
            Self::FixHeightmaps => "FixHeightmaps",
            Self::DeleteStructureReference(_) => "DeleteStructureReference",
            Self::PreventRetrogen => "PreventRetrogen",
            Self::ForceBlend => "ForceBlend",
            Self::RemoveLightingInfo => "RemoveLightingInfo",
            Self::LightPopulated(_) => "LightPopulated",
        }
    }

    fn value(&self) -> String {
        match self {
            Self::DataVersion(value) | Self::Timestamp(value) => value.to_string(),
            Self::InhabitedTime(value) | Self::LastUpdate(value) => value.to_string(),
            Self::Compression(value) => compression_name(*value).to_string(),
            Self::Status(status) => value_text(&status.display()),
            Self::Biome(biome) => value_text(&biome.display()),
            Self::ReplaceBlocks(entries) => quote(
                &entries
                    .iter()
                    .map(|(from, to)| {
                        let from = if from.starts_with("minecraft:") {
                            from.clone()
                        } else {
                            format!("'{from}'")
                        };
                        format!("{from}={}", to.display())
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Self::DeleteSections(ranges) if ranges.len() == 1 && ranges[0].is_all() => {
                "true".into()
            }
            Self::DeleteSections(ranges) => quote(
                &ranges
                    .iter()
                    .map(Range::display)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Self::DeleteStructureReference(names) => quote(
                &names
                    .iter()
                    .map(|name| {
                        if registry().structures.contains_key(name.as_str()) {
                            name.clone()
                        } else {
                            format!("'{name}'")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Self::LightPopulated(value) => value.to_string(),
            Self::DeleteEntities
            | Self::FixStatus
            | Self::FixHeightmaps
            | Self::PreventRetrogen
            | Self::ForceBlend
            | Self::RemoveLightingInfo => "true".into(),
        }
    }
}

impl ChangeSet {
    /// The normalized change text (`Field = value, ...`), which [`parse`] reads back unchanged.
    pub fn to_query(&self) -> String {
        self.fields
            .iter()
            .map(|field| format!("{} = {}", field.name(), field.value()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn needs(&self) -> Needs {
        let mut needs = Needs::default();
        for field in &self.fields {
            needs.region = true;
            match field {
                Field::DataVersion(_) => {
                    needs.entities = true;
                    needs.poi = true;
                }
                Field::DeleteEntities | Field::DeleteSections(_) => needs.entities = true,
                _ => {}
            }
        }
        needs
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Applies every field in order. `force` is MCA Selector's "force": fields that are only
    /// changed where a chunk already has them (DataVersion, InhabitedTime, LastUpdate, Status,
    /// Biome, LightPopulated) are set even where missing; the other fields behave the same.
    pub fn apply(&self, chunk: &mut ChunkMut, force: bool) -> Changed {
        let mut changed = Changed::default();
        for field in &self.fields {
            apply(field, chunk, force, &mut changed);
        }
        changed
    }
}

// ---------------------------------------------------------------------------------------------
// Applying

fn apply(field: &Field, chunk: &mut ChunkMut, force: bool, changed: &mut Changed) {
    let region = chunk.region.as_deref_mut();
    match field {
        Field::DataVersion(value) => {
            changed.region |= region.is_some_and(|root| data_version_change(root, *value, force));
            changed.entities |= chunk
                .entities
                .as_deref_mut()
                .is_some_and(|root| data_version_change(root, *value, force));
            changed.poi |= chunk
                .poi
                .as_deref_mut()
                .is_some_and(|root| data_version_change(root, *value, force));
        }
        Field::InhabitedTime(value) => {
            changed.region |=
                region.is_some_and(|root| set_long(root, "InhabitedTime", *value, force));
        }
        Field::LastUpdate(value) => {
            changed.region |=
                region.is_some_and(|root| set_long(root, "LastUpdate", *value, force));
        }
        Field::Timestamp(value) => {
            if region.is_some() && *chunk.timestamp != *value as u32 {
                *chunk.timestamp = *value as u32;
                changed.header = true;
            }
        }
        Field::Compression(value) => {
            if region.is_some() && *chunk.compression != *value {
                *chunk.compression = *value;
                changed.region = true;
            }
        }
        Field::Status(status) => {
            changed.region |= region.is_some_and(|root| set_status(root, status, force));
        }
        Field::Biome(biome) => {
            changed.region |= region.is_some_and(|root| change_biome(root, biome, force));
        }
        Field::ReplaceBlocks(entries) => {
            if let Some(root) = region
                && data_version(root) >= 1451
            {
                let mut modified = replace_blocks(root, entries);
                modified |= fix_heightmaps(root);
                modified |= remove_lighting(root);
                modified |= set_light_populated(root, 0, true);
                changed.region |= modified;
            }
        }
        // MCA Selector 2.9 looks entities up under `Level` and so deletes none in 1.17+ chunks
        // (entities/ files, 1.18+ proto-entities); World Map deletes them, the intended behaviour.
        Field::DeleteEntities => {
            changed.region |= filter_entities(region.and_then(proto_entities), None);
            changed.entities |= filter_entities(
                chunk
                    .entities
                    .as_deref_mut()
                    .and_then(|root| root.list_mut("Entities")),
                None,
            );
        }
        Field::DeleteSections(ranges) => {
            if let Some(root) = region {
                let mut modified = delete_sections(root, ranges);
                modified |= filter_entities(proto_entities(root), Some(ranges.as_slice()));
                // MCA Selector's heightmap writer only finds `Level` chunks; 1.18+ chunks keep
                // their heightmaps and are set back to `features`, which recomputes them
                if matches!(root.get("Level"), Some(Tag::Compound(_))) {
                    modified |= fix_heightmaps(root);
                }
                changed.region |= modified;
            }
            changed.entities |= filter_entities(
                chunk
                    .entities
                    .as_deref_mut()
                    .and_then(|root| root.list_mut("Entities")),
                Some(ranges.as_slice()),
            );
        }
        Field::FixStatus => changed.region |= region.is_some_and(fix_status),
        Field::FixHeightmaps => changed.region |= region.is_some_and(fix_heightmaps),
        Field::DeleteStructureReference(names) => {
            changed.region |= region.is_some_and(|root| delete_structures(root, names));
        }
        Field::PreventRetrogen => changed.region |= region.is_some_and(prevent_retrogen),
        Field::ForceBlend => changed.region |= region.is_some_and(force_blend),
        Field::RemoveLightingInfo => changed.region |= region.is_some_and(remove_lighting),
        Field::LightPopulated(value) => {
            changed.region |= region.is_some_and(|root| set_light_populated(root, *value, force));
        }
    }
}

/// Sets a tag; whether the value differed.
fn set(compound: &mut Compound, key: &str, tag: Tag) -> bool {
    if compound.get(key) == Some(&tag) {
        return false;
    }
    compound.insert(key, tag);
    true
}

/// A compound with one entry.
fn named(key: &str, tag: Tag) -> Tag {
    Tag::Compound(std::iter::once((key.to_string(), tag)).collect())
}

/// The DataVersion as MCA Selector reads it (root, else `Level`; 100 when absent).
fn data_version(root: &Compound) -> i64 {
    if !root.contains("DataVersion")
        && let Some(level) = root.compound("Level")
    {
        return level.i64("DataVersion").unwrap_or(100);
    }
    root.i64("DataVersion").unwrap_or(100)
}

/// Where a region/ chunk keeps its data: `Level` before 21w43a, the root since.
struct Target<'a> {
    c: &'a mut Compound,
    level: bool,
    sections: &'static str,
    dv: i64,
}

fn target(root: &mut Compound) -> Option<Target<'_>> {
    let dv = data_version(root);
    if matches!(root.get("Level"), Some(Tag::Compound(_))) {
        return root.compound_mut("Level").map(|c| Target {
            c,
            level: true,
            sections: "Sections",
            dv,
        });
    }
    (dv >= 2844).then_some(Target {
        c: root,
        level: false,
        sections: "sections",
        dv,
    })
}

fn compounds_mut(list: &mut [Tag]) -> impl Iterator<Item = &mut Compound> {
    list.iter_mut().filter_map(Tag::as_compound_mut)
}

/// `Helper.setDataVersion` where MCA Selector's `DataVersionField` changes or forces it.
fn data_version_change(root: &mut Compound, value: i32, force: bool) -> bool {
    if !force {
        let tag = if root.contains("DataVersion") {
            root.get("DataVersion")
        } else {
            root.compound("Level")
                .and_then(|level| level.get("DataVersion"))
        };
        if !matches!(tag, Some(Tag::Int(_))) {
            return false;
        }
    }
    let mut changed = false;
    // DataVersion was kept in Level before 15w33a
    if value < 111
        && let Some(level) = root.compound_mut("Level")
    {
        changed |= set(level, "DataVersion", Tag::Int(value));
    }
    set(root, "DataVersion", Tag::Int(value)) || changed
}

/// InhabitedTime and LastUpdate.
fn set_long(root: &mut Compound, key: &str, value: i64, force: bool) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if !force && !target.c.contains(key) {
        return false;
    }
    set(target.c, key, Tag::Long(value))
}

/// Chunk statuses exist since 18w06a (1466).
fn set_status(root: &mut Compound, status: &Status, force: bool) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if target.dv < 1466 || (!force && target.c.str("Status").is_none()) {
        return false;
    }
    set(target.c, "Status", Tag::String(status.value(target.dv)))
}

/// `FixStatusField`: an `empty` chunk that has sections becomes `full`.
fn fix_status(root: &mut Compound) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if target.dv < 1466
        || !target
            .c
            .str("Status")
            .is_some_and(|status| status == "empty" || status == "minecraft:empty")
        || !target
            .c
            .list(target.sections)
            .is_some_and(|list| !list.is_empty())
    {
        return false;
    }
    let full = Status {
        name: "full".into(),
        custom: false,
    };
    set(target.c, "Status", Tag::String(full.value(target.dv)))
}

/// `LightPopulated` before 18w06a, nothing until 19w02a (1921), `isLightOn` since.
fn set_light_populated(root: &mut Compound, value: i8, force: bool) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    let key = if !target.level || target.dv >= 1921 {
        "isLightOn"
    } else if target.dv < 1466 {
        "LightPopulated"
    } else {
        return false;
    };
    if !force && !target.c.contains(key) {
        return false;
    }
    set(target.c, key, Tag::Byte(value))
}

/// `Light.removeLightingInfo`: light arrays of every section, and the light flag.
fn remove_lighting(root: &mut Compound) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    let mut changed = false;
    if target.level && target.dv < 1466 {
        changed |= target.c.remove("LightPopulated").is_some();
    } else if !target.level || target.dv >= 1921 {
        changed |= set(target.c, "isLightOn", Tag::Byte(0));
    }
    if let Some(list) = target.c.list_mut(target.sections) {
        for section in compounds_mut(list) {
            changed |= section.remove("BlockLight").is_some();
            changed |= section.remove("SkyLight").is_some();
        }
    }
    changed
}

/// The entities saved in a region/ chunk: all of them before 1.17, proto-entities since.
fn proto_entities(root: &mut Compound) -> Option<&mut Vec<Tag>> {
    let Target { c, level, .. } = target(root)?;
    c.list_mut(if level { "Entities" } else { "entities" })
}

/// Removes all entities, or those whose `Pos` lies in one of the section ranges.
fn filter_entities(list: Option<&mut Vec<Tag>>, ranges: Option<&[Range]>) -> bool {
    let Some(list) = list else {
        return false;
    };
    let before = list.len();
    match ranges {
        None => list.clear(),
        Some(ranges) => list.retain(|entity| {
            let y = entity
                .as_compound()
                .and_then(|entity| entity.list("Pos"))
                .filter(|pos| pos.len() == 3)
                .and_then(|pos| pos[1].as_f64());
            !y.is_some_and(|y| {
                let section = ((y as i32) >> 4) as i64;
                ranges.iter().any(|range| range.contains(section))
            })
        }),
    }
    list.len() != before
}

fn air_entry(data_version: i64) -> Tag {
    if data_version >= 5009 {
        Tag::String("minecraft:air".into())
    } else {
        named("Name", Tag::String("minecraft:air".into()))
    }
}

/// `Sections.deleteSections`: before 21w37a the sections are removed; since, their blocks become
/// air (biomes stay) and only chunks past the light stage are touched, set back to `features`.
fn delete_sections(root: &mut Compound, ranges: &[Range]) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    let modern = !target.level
        || target.c.list(target.sections).is_some_and(|list| {
            list.iter()
                .filter_map(Tag::as_compound)
                .any(|section| section.contains("block_states"))
        });
    if !modern {
        let Some(list) = target.c.list_mut(target.sections) else {
            return false;
        };
        let before = list.len();
        list.retain(|section| {
            let y = section
                .as_compound()
                .and_then(|section| section.i64("Y"))
                .unwrap_or(0);
            !ranges.iter().any(|range| range.contains(y))
        });
        return list.len() != before;
    }
    let status = target
        .c
        .str("Status")
        .map(|status| status.strip_prefix("minecraft:").unwrap_or(status));
    if !matches!(status, Some("light" | "spawn" | "heightmaps" | "full")) {
        return false;
    }
    let features = if target.dv >= 3458 {
        "minecraft:features"
    } else {
        "features"
    };
    let mut changed = set(target.c, "Status", Tag::String(features.into()));
    let air = air_entry(target.dv);
    if let Some(list) = target.c.list_mut(target.sections) {
        for section in compounds_mut(list) {
            let y = section.i64("Y").unwrap_or(0);
            if !ranges.iter().any(|range| range.contains(y)) {
                continue;
            }
            let Some(states) = section.compound_mut("block_states") else {
                continue;
            };
            changed |= states.remove("data").is_some();
            changed |= set(states, "palette", Tag::List(vec![air.clone()]));
            changed |= section.remove("BlockLight").is_some();
        }
    }
    changed
}

/// `DeleteStructureField`: removes the structure (and its alternative names) from the chunk's
/// structure references and starts.
fn delete_structures(root: &mut Compound, names: &[String]) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    let (key, starts) = if target.level {
        ("Structures", "Starts")
    } else {
        ("structures", "starts")
    };
    let Some(structures) = target.c.compound_mut(key) else {
        return false;
    };
    let mut changed = false;
    for name in names {
        let alternatives = registry()
            .structures
            .get(name.as_str())
            .map(|alternatives| alternatives.iter().map(|alt| alt.to_string()).collect())
            .unwrap_or_else(|| vec![name.clone()]);
        for alternative in &alternatives {
            for part in ["References", starts] {
                if let Some(compound) = structures.compound_mut(part) {
                    changed |= compound.remove(alternative).is_some();
                }
            }
        }
    }
    changed
}

/// `PreventRetrogenField` (1.18+ chunks, the only ones with `below_zero_retrogen`).
fn prevent_retrogen(root: &mut Compound) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if target.level {
        return false;
    }
    let mut changed = target.c.remove("below_zero_retrogen").is_some();
    changed |= set(target.c, "Status", Tag::String("full".into()));
    changed
}

/// `Blending.forceBlending` (1.18+): `blending_data` over the chunk's sections, without
/// heightmaps and light flag so the game recomputes them; 25w02a to 1.21.5-rc1 instead reset
/// the DataVersion to 1.21.4's (4189), as MCA Selector does there.
fn force_blend(root: &mut Compound) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if target.level {
        return false;
    }
    if (4317..=4323).contains(&target.dv) {
        return set(target.c, "DataVersion", Tag::Int(4189));
    }
    let Some(list) = target.c.list("sections") else {
        return false;
    };
    let (mut min, mut max) = (0i64, 0i64);
    for section in list.iter().filter_map(Tag::as_compound) {
        let y = section.i64("Y").unwrap_or(0);
        min = min.min(y);
        max = max.max(y);
    }
    let blending: Compound = [
        ("min_section".to_string(), Tag::Int(min.min(-4) as i32)),
        ("max_section".to_string(), Tag::Int(max.max(20) as i32)),
    ]
    .into_iter()
    .collect();
    let mut changed = set(target.c, "blending_data", Tag::Compound(blending));
    changed |= target.c.remove("Heightmaps").is_some();
    changed |= target.c.remove("isLightOn").is_some();
    changed
}

/// `Helper.findSectionRange`: from `yPos` (else the lowest section) to the highest section,
/// ignoring sections that hold nothing but their `Y`.
fn section_range(container: &Compound, sections: &[Tag]) -> Option<(i64, i64)> {
    let (mut low, mut high) = (i64::MAX, i64::MIN);
    for section in sections.iter().filter_map(Tag::as_compound) {
        if section.len() == 1 {
            continue;
        }
        let Some(y) = section.i64("Y") else {
            continue;
        };
        low = low.min(y);
        high = high.max(y);
    }
    if let Some(y) = container.i64("yPos") {
        low = y;
    }
    (low <= high).then_some((low, high))
}

/// `Biomes.changeBiome` / `forceBiome`: palettes per section since 21w37a, a numeric `Biomes`
/// array (256 entries before 19w36a, 1024 since; per section during 21w06a to 21w14a) before.
fn change_biome(root: &mut Compound, biome: &Biome, force: bool) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    if target.dv < 1466 {
        return false;
    }
    let per_section = !target.level
        || target.c.list(target.sections).is_some_and(|list| {
            list.iter()
                .filter_map(Tag::as_compound)
                .any(|section| section.contains("biomes"))
        });
    if per_section {
        let Some(name) = &biome.name else {
            return false;
        };
        let Some(list) = target.c.list_mut(target.sections) else {
            return false;
        };
        let palette = Tag::List(vec![Tag::String(name.clone())]);
        let mut changed = false;
        for section in compounds_mut(list) {
            if force {
                changed |= set(section, "biomes", named("palette", palette.clone()));
            } else if let Some(biomes) = section.compound_mut("biomes") {
                changed |= set(biomes, "palette", palette.clone());
                changed |= biomes.remove("data").is_some();
            }
        }
        return changed;
    }
    let Some(id) = biome.id else {
        return false;
    };
    if !force {
        return match target.c.get_mut("Biomes").and_then(Tag::as_ints_mut) {
            Some(ints) => {
                let changed = ints.iter().any(|&value| value != id);
                ints.fill(id);
                changed
            }
            None => false,
        };
    }
    let length = if target.dv < 2203 {
        256
    } else if (2694..=2708).contains(&target.dv) {
        match target
            .c
            .list(target.sections)
            .and_then(|list| section_range(target.c, list))
        {
            Some((low, high)) if high - low < 256 => ((high - low + 1) * 64) as usize,
            _ => return false,
        }
    } else {
        1024
    };
    set(target.c, "Biomes", Tag::IntArray(vec![id; length]))
}

/// The block name of a palette entry: `{Name, Properties}` before 26.x; a string or
/// `{id, properties}` since (strings wrapped as `{"": name}` when a list mixes both).
fn entry_name(entry: &Tag) -> Option<&str> {
    match entry {
        Tag::String(name) => Some(name),
        Tag::Compound(compound) => compound
            .str("Name")
            .or_else(|| compound.str("id"))
            .or_else(|| compound.str("")),
        _ => None,
    }
}

/// `{"": name}` -> `name`.
fn unwrap_entry(entry: &Tag) -> Tag {
    if let Tag::Compound(compound) = entry
        && compound.len() == 1
        && let Some(name) = compound.str("")
    {
        return Tag::String(name.to_string());
    }
    entry.clone()
}

/// NBT lists hold one type: when strings and compounds mix, strings are wrapped as `{"": name}`.
fn normalize_palette(palette: &mut [Tag]) {
    let strings = palette.iter().any(|entry| matches!(entry, Tag::String(_)));
    let compounds = palette
        .iter()
        .any(|entry| matches!(entry, Tag::Compound(_)));
    if strings && compounds {
        for entry in palette.iter_mut() {
            if let Tag::String(name) = entry {
                *entry = named("", Tag::String(std::mem::take(name)));
            }
        }
    }
}

/// `count` values of `bits` bits in longs, spanning two longs (before 20w17a) or not.
fn pack(values: &[u16], bits: u32, spanning: bool) -> Vec<i64> {
    let mask = (1u64 << bits) - 1;
    let bits = bits as usize;
    let mut out;
    if spanning {
        out = vec![0u64; (values.len() * bits).div_ceil(64)];
        for (i, &value) in values.iter().enumerate() {
            let value = value as u64 & mask;
            let (word, offset) = ((i * bits) / 64, (i * bits) % 64);
            out[word] |= value << offset;
            if offset + bits > 64 {
                out[word + 1] |= value >> (64 - offset);
            }
        }
    } else {
        let per = 64 / bits;
        out = vec![0u64; values.len().div_ceil(per)];
        for (i, &value) in values.iter().enumerate() {
            out[i / per] |= (value as u64 & mask) << ((i % per) * bits);
        }
    }
    out.into_iter().map(|value| value as i64).collect()
}

/// A new air section (`createEmptySection` / `completeSection`).
fn empty_section(y: i64, modern: bool, data_version: i64) -> Compound {
    let mut section: Compound = std::iter::once(("Y".to_string(), Tag::Byte(y as i8))).collect();
    if modern {
        complete_section(&mut section, data_version);
    } else {
        section.insert("BlockStates", Tag::LongArray(vec![0; 256]));
        section.insert("Palette", Tag::List(vec![air_entry(data_version)]));
    }
    section
}

/// Gives a section without blocks an air palette and plains biomes.
fn complete_section(section: &mut Compound, data_version: i64) {
    if !section.contains("block_states") {
        section.insert(
            "block_states",
            named("palette", Tag::List(vec![air_entry(data_version)])),
        );
    }
    if !section.contains("biomes") {
        section.insert(
            "biomes",
            named(
                "palette",
                Tag::List(vec![Tag::String("minecraft:plains".into())]),
            ),
        );
    }
}

/// Replacing air: every section of the chunk's range gets blocks, so air there can be replaced.
fn fill_air_sections(
    container: &mut Compound,
    key: &str,
    (min, max): (i64, i64),
    modern: bool,
    data_version: i64,
) -> bool {
    if max - min > 255 {
        return false;
    }
    let Some(list) = container.list_mut(key) else {
        return false;
    };
    let mut changed = false;
    for y in min..=max {
        let existing = list.iter().position(|section| {
            section
                .as_compound()
                .is_some_and(|section| section.i64("Y").unwrap_or(0) == y)
        });
        match existing.and_then(|index| list[index].as_compound_mut()) {
            None => {
                list.push(Tag::Compound(empty_section(y, modern, data_version)));
                changed = true;
            }
            Some(section) if modern => {
                if !section.contains("block_states") {
                    complete_section(section, data_version);
                    changed = true;
                }
            }
            Some(section) => {
                if !section.contains("BlockStates") || !section.contains("Palette") {
                    *section = empty_section(y, false, data_version);
                    changed = true;
                }
            }
        }
    }
    if changed {
        list.sort_by_key(|section| {
            section
                .as_compound()
                .and_then(|section| section.i64("Y"))
                .unwrap_or(0)
        });
    }
    changed
}

/// Replaces the blocks of one section; block entities at replaced positions are removed, or
/// replaced by the replacement's own.
fn replace_in_section(
    section: &mut Compound,
    y: i64,
    (chunk_x, chunk_z): (i64, i64),
    entries: &[(String, Replacement)],
    data_version: i64,
    tiles: &mut Vec<Tag>,
    tiles_changed: &mut bool,
) -> bool {
    let modern = matches!(section.get("block_states"), Some(Tag::Compound(_)));
    let spanning = data_version < 2529;
    let (palette, data) = if modern {
        let Some(states) = section.compound("block_states") else {
            return false;
        };
        (
            states.list("palette"),
            states.get("data").and_then(Tag::as_longs),
        )
    } else {
        (
            section.list("Palette"),
            section.get("BlockStates").and_then(Tag::as_longs),
        )
    };
    let Some(palette) = palette.filter(|palette| !palette.is_empty()) else {
        return false;
    };
    let rules: Vec<Option<usize>> = palette
        .iter()
        .map(|entry| {
            entry_name(entry).and_then(|name| entries.iter().position(|(from, _)| from == name))
        })
        .collect();
    if rules.iter().all(Option::is_none) {
        return false;
    }
    let indices = match data {
        Some(data) => match unpack(data, bits_for(palette.len()).max(4), 4096, spanning) {
            Some(indices) => indices,
            None => return false,
        },
        None if modern && palette.len() == 1 => vec![0; 4096],
        None => return false,
    };
    let mut new_palette: Vec<Tag> = palette.iter().map(unwrap_entry).collect();

    let mut targets: Vec<Option<u16>> = vec![None; rules.len()];
    let mut new_indices = Vec::with_capacity(4096);
    for (i, &old) in indices.iter().enumerate() {
        let Some(&Some(rule)) = rules.get(old as usize) else {
            new_indices.push(old);
            continue;
        };
        let replacement = &entries[rule].1;
        let target = match targets[old as usize] {
            Some(target) => target,
            None => {
                let state = replacement.state(data_version);
                let target = match new_palette.iter().position(|entry| *entry == state) {
                    Some(position) => position,
                    None => {
                        new_palette.push(state);
                        new_palette.len() - 1
                    }
                } as u16;
                targets[old as usize] = Some(target);
                target
            }
        };
        new_indices.push(target);

        let position = (
            chunk_x * 16 + (i & 15) as i64,
            y * 16 + (i >> 8) as i64,
            chunk_z * 16 + ((i >> 4) & 15) as i64,
        );
        let at = |tile: &Tag| {
            tile.as_compound().is_some_and(|tile| {
                (
                    tile.i64("x").unwrap_or(0),
                    tile.i64("y").unwrap_or(0),
                    tile.i64("z").unwrap_or(0),
                ) == position
            })
        };
        if let Some(index) = tiles.iter().position(at) {
            tiles.remove(index);
            *tiles_changed = true;
        }
        if let Some(tile) = &replacement.tile {
            let mut tile = tile.clone();
            tile.insert("x", Tag::Int(position.0 as i32));
            tile.insert("y", Tag::Int(position.1 as i32));
            tile.insert("z", Tag::Int(position.2 as i32));
            tiles.push(Tag::Compound(tile));
            *tiles_changed = true;
        }
    }

    // drop unused entries, keep air in the palette (`cleanupPalette`)
    let mut used = vec![false; new_palette.len()];
    for &index in &new_indices {
        if let Some(used) = used.get_mut(index as usize) {
            *used = true;
        }
    }
    let mut remap = vec![0u16; new_palette.len()];
    let mut compact = Vec::with_capacity(new_palette.len());
    for (index, entry) in new_palette.into_iter().enumerate() {
        if used[index] {
            remap[index] = compact.len() as u16;
            compact.push(entry);
        }
    }
    for index in &mut new_indices {
        *index = remap.get(*index as usize).copied().unwrap_or(0);
    }
    if !compact
        .iter()
        .any(|entry| entry_name(entry) == Some("minecraft:air"))
    {
        compact.push(air_entry(data_version));
    }
    normalize_palette(&mut compact);
    let bits = bits_for(compact.len()).max(4);
    let single = compact.len() == 1;
    if modern {
        let Some(states) = section.compound_mut("block_states") else {
            return false;
        };
        states.insert("palette", Tag::List(compact));
        if single {
            states.remove("data");
        } else {
            states.insert("data", Tag::LongArray(pack(&new_indices, bits, spanning)));
        }
    } else {
        section.insert("Palette", Tag::List(compact));
        section.insert(
            "BlockStates",
            Tag::LongArray(pack(&new_indices, bits, spanning)),
        );
    }
    true
}

/// `Blocks.replaceBlocks` (palette chunks, 17w47a+).
fn replace_blocks(root: &mut Compound, entries: &[(String, Replacement)]) -> bool {
    let Some(target) = target(root) else {
        return false;
    };
    let (dv, level, key, container) = (target.dv, target.level, target.sections, target.c);
    if dv < 1451 {
        return false;
    }
    let (Some(chunk_x), Some(chunk_z)) = (container.i64("xPos"), container.i64("zPos")) else {
        return false;
    };
    let Some(list) = container.list(key) else {
        return false;
    };
    let modern = !level
        || dv >= 2834
        || list
            .iter()
            .filter_map(Tag::as_compound)
            .any(|section| section.contains("block_states"));
    let range = if level && !modern && !(2694..=2708).contains(&dv) {
        Some((0, 15))
    } else {
        section_range(container, list)
    };
    let Some((min, max)) = range else {
        return false;
    };

    let mut changed = false;
    if entries.iter().any(|(from, _)| from == "minecraft:air") {
        changed |= fill_air_sections(container, key, (min, max), modern, dv);
    }
    let tiles_key = if level {
        "TileEntities"
    } else {
        "block_entities"
    };
    let existing = container.list_mut(tiles_key).map(std::mem::take);
    let had_tiles = existing.is_some();
    let mut tiles = existing.unwrap_or_default();
    let mut tiles_changed = false;
    if let Some(list) = container.list_mut(key) {
        for section in compounds_mut(list) {
            let y = section.i64("Y").unwrap_or(min - 1);
            if y < min || y > max {
                continue;
            }
            changed |= replace_in_section(
                section,
                y,
                (chunk_x, chunk_z),
                entries,
                dv,
                &mut tiles,
                &mut tiles_changed,
            );
        }
    }
    if had_tiles {
        if let Some(list) = container.list_mut(tiles_key) {
            *list = tiles;
        }
    } else if !tiles.is_empty() {
        container.insert(tiles_key, Tag::List(tiles));
    }
    changed || tiles_changed
}

/// Heightmap slots: bit 1 WORLD_SURFACE, 2 OCEAN_FLOOR, 4 MOTION_BLOCKING,
/// 8 MOTION_BLOCKING_NO_LEAVES, 16 LIGHT_BLOCKING.
fn heightmap_mask(name: Option<&str>, era: usize) -> u8 {
    let Some(name) = name else {
        return 0;
    };
    let Some((masks, _)) = block_table()
        .blocks
        .get(name.strip_prefix("minecraft:").unwrap_or(name))
    else {
        return 0;
    };
    let mask = masks.get(era).copied().unwrap_or(0);
    // LIGHT_BLOCKING only existed in 1.13, the first config has it
    let light = masks.first().copied().unwrap_or(0) & 16;
    let no_leaves = if mask & 4 != 0 && mask & 8 == 0 { 8 } else { 0 };
    (mask & 0b0111) | no_leaves | light
}

/// Recomputes a region/ chunk's heightmaps from its blocks, with MCA Selector's per-version block
/// lists and the heightmaps each version has (from 18w19a, DataVersion 1484): heights relative to
/// the bottom of the world in longs of `ceil(log2(height + 1))` bits, spanning two longs before
/// 20w17a. Used by FixHeightmaps, DeleteSections, ReplaceBlocks and chunk imports; whether
/// anything changed.
pub fn fix_heightmaps(root: &mut Compound) -> bool {
    let dv = data_version(root);
    if dv < 1484 {
        return false;
    }
    let Some(target) = target(root) else {
        return false;
    };
    let table = block_table();
    let era = table.eras.iter().rposition(|&era| dv >= era).unwrap_or(0);
    let spanning = dv < 2529;
    let keys: &[(&str, usize)] = if dv < 1503 {
        &[
            ("LIGHT_BLOCKING", 4),
            ("OCEAN_FLOOR", 1),
            ("MOTION_BLOCKING", 2),
            ("MOTION_BLOCKING_NO_LEAVES", 3),
        ]
    } else if dv < 1908 {
        &[
            ("LIGHT_BLOCKING", 4),
            ("WORLD_SURFACE", 0),
            ("OCEAN_FLOOR", 1),
            ("MOTION_BLOCKING", 2),
            ("MOTION_BLOCKING_NO_LEAVES", 3),
        ]
    } else {
        &[
            ("WORLD_SURFACE", 0),
            ("OCEAN_FLOOR", 1),
            ("MOTION_BLOCKING", 2),
            ("MOTION_BLOCKING_NO_LEAVES", 3),
        ]
    };
    let wanted = keys.iter().fold(0u8, |mask, (_, slot)| mask | 1 << slot);

    // (section y, heightmap bits per palette entry, palette indices unless single-valued)
    let mut sections: Vec<(i64, Vec<u8>, Option<Vec<u16>>)> = Vec::new();
    let mut legacy = false;
    let Some(list) = target.c.list(target.sections) else {
        return false;
    };
    for section in list.iter().filter_map(Tag::as_compound) {
        let Some(y) = section.i64("Y") else {
            continue;
        };
        let (palette, data) = match section.compound("block_states") {
            Some(states) => (
                states.list("palette"),
                states.get("data").and_then(Tag::as_longs),
            ),
            None => {
                legacy |= section.contains("Palette");
                (
                    section.list("Palette"),
                    section.get("BlockStates").and_then(Tag::as_longs),
                )
            }
        };
        let Some(palette) = palette.filter(|palette| !palette.is_empty()) else {
            continue;
        };
        let masks = palette
            .iter()
            .map(|entry| heightmap_mask(entry_name(entry), era))
            .collect();
        let indices = if palette.len() > 1 {
            match data.and_then(|data| unpack(data, bits_for(palette.len()).max(4), 4096, spanning))
            {
                Some(indices) => Some(indices),
                None => continue,
            }
        } else {
            None
        };
        sections.push((y, masks, indices));
    }
    let (min, max) = if target.level && legacy && !(2694..=2708).contains(&dv) {
        (0, 15)
    } else {
        let low = target
            .c
            .i64("yPos")
            .or_else(|| sections.iter().map(|section| section.0).min());
        let high = sections.iter().map(|section| section.0).max();
        match (low, high) {
            (Some(low), Some(high)) if low <= high && high - low < 256 => (low, high),
            _ => return false,
        }
    };
    sections.retain(|section| (min..=max).contains(&section.0));
    sections.sort_by_key(|section| std::cmp::Reverse(section.0));

    let mut heights = [[0u16; 256]; 5];
    for column in 0..256 {
        let mut found = 0u8;
        'sections: for (y, masks, indices) in &sections {
            for local in (0..16).rev() {
                let index = indices
                    .as_ref()
                    .map_or(0, |indices| indices[local * 256 + column] as usize);
                let mask = masks.get(index).copied().unwrap_or(0) & wanted & !found;
                if mask == 0 {
                    continue;
                }
                let height = ((y - min) * 16 + local as i64 + 1) as u16;
                for (slot, values) in heights.iter_mut().enumerate() {
                    if mask & 1 << slot != 0 {
                        values[column] = height;
                    }
                }
                found |= mask;
                if found == wanted {
                    break 'sections;
                }
            }
        }
    }

    let bits = bits_for(((max - min + 1) * 16) as usize + 1);
    let container = target.c;
    let mut changed = false;
    if !matches!(container.get("Heightmaps"), Some(Tag::Compound(_))) {
        container.insert("Heightmaps", Tag::Compound(Compound::new()));
        changed = true;
    }
    let Some(heightmaps) = container.compound_mut("Heightmaps") else {
        return changed;
    };
    for (name, slot) in keys {
        changed |= set(
            heightmaps,
            name,
            Tag::LongArray(pack(&heights[*slot], bits, spanning)),
        );
    }
    changed
}

/// The SNBT MCA Selector's ReplaceBlocks values use (Querz NBT's `SNBTParser` and
/// `SNBTWriter`): reading block states and block entities, writing them back normalized.
mod snbt {
    use crate::nbt::{Compound, Tag};

    /// Parses a compound at the start of `chars`; the compound and the characters it took.
    pub fn parse_compound(chars: &[char]) -> Result<(Compound, usize), String> {
        let mut parser = Parser { chars, index: 0 };
        parser.skip_whitespace();
        match parser.value()? {
            Tag::Compound(compound) => Ok((compound, parser.index)),
            _ => Err("expected an SNBT compound".into()),
        }
    }

    struct Parser<'a> {
        chars: &'a [char],
        index: usize,
    }

    fn simple_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.')
    }

    /// `^[a-zA-Z_][\w.+-]*$`
    fn unquoted(s: &str) -> bool {
        let mut chars = s.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<char> {
            self.chars.get(self.index).copied()
        }

        fn skip_whitespace(&mut self) {
            while self.peek().is_some_and(char::is_whitespace) {
                self.index += 1;
            }
        }

        fn error(&self, message: &str) -> String {
            format!("{message} in SNBT at character {}", self.index)
        }

        fn expect(&mut self, expected: char) -> Result<(), String> {
            self.skip_whitespace();
            if self.peek() == Some(expected) {
                self.index += 1;
                Ok(())
            } else {
                Err(self.error(&format!("expected '{expected}'")))
            }
        }

        fn separator(&mut self) -> bool {
            self.skip_whitespace();
            if self.peek() == Some(',') {
                self.index += 1;
                self.skip_whitespace();
                true
            } else {
                false
            }
        }

        fn simple(&mut self) -> String {
            let start = self.index;
            while self.peek().is_some_and(simple_char) {
                self.index += 1;
            }
            self.chars[start..self.index].iter().collect()
        }

        fn hex(&mut self, length: usize) -> Result<char, String> {
            let digits: String = self
                .chars
                .get(self.index..self.index + length)
                .ok_or_else(|| self.error("incomplete escape"))?
                .iter()
                .collect();
            self.index += length;
            u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| self.error(&format!("invalid escape {digits}")))
        }

        /// After the opening quote.
        fn quoted(&mut self, quote: char) -> Result<String, String> {
            let mut value = String::new();
            while let Some(c) = self.peek() {
                self.index += 1;
                if c == quote {
                    return Ok(value);
                }
                if c != '\\' {
                    value.push(c);
                    continue;
                }
                let escaped = self.peek().ok_or_else(|| self.error("missing end quote"))?;
                self.index += 1;
                value.push(match escaped {
                    '\\' => '\\',
                    c if c == quote => c,
                    'b' => '\u{8}',
                    's' => ' ',
                    't' => '\t',
                    'n' => '\n',
                    'f' => '\u{c}',
                    'r' => '\r',
                    'x' => self.hex(2)?,
                    'u' => self.hex(4)?,
                    'U' => self.hex(8)?,
                    other => return Err(self.error(&format!("invalid escape of '{other}'"))),
                });
            }
            Err(self.error("missing end quote"))
        }

        fn value(&mut self) -> Result<Tag, String> {
            self.skip_whitespace();
            match self.peek() {
                Some('{') => self.compound().map(Tag::Compound),
                Some('[') => self.collection(),
                Some(quote @ ('"' | '\'')) => {
                    self.index += 1;
                    self.quoted(quote).map(Tag::String)
                }
                Some(_) => {
                    let text = self.simple();
                    literal(&text).ok_or_else(|| {
                        self.error(&format!("invalid number or unquoted string {text}"))
                    })
                }
                None => Err(self.error("unexpected end")),
            }
        }

        fn compound(&mut self) -> Result<Compound, String> {
            self.expect('{')?;
            let mut compound = Compound::new();
            self.skip_whitespace();
            while self.peek().is_some_and(|c| c != '}') {
                if self.separator() {
                    return Err(self.error("unexpected separator"));
                }
                self.skip_whitespace();
                let key = match self.peek() {
                    Some(quote @ ('"' | '\'')) => {
                        self.index += 1;
                        self.quoted(quote)?
                    }
                    _ => {
                        let key = self.simple();
                        if !unquoted(&key) {
                            return Err(self.error(&format!("invalid unquoted key {key}")));
                        }
                        key
                    }
                };
                self.expect(':')?;
                let value = self.value()?;
                compound.insert(key, value);
                if !self.separator() {
                    break;
                }
            }
            self.expect('}')?;
            Ok(compound)
        }

        fn collection(&mut self) -> Result<Tag, String> {
            let at = |offset: usize| self.chars.get(self.index + offset).copied();
            if self.index + 3 < self.chars.len()
                && !matches!(at(1), Some('"' | '\''))
                && at(2) == Some(';')
            {
                self.array()
            } else {
                self.list()
            }
        }

        fn array(&mut self) -> Result<Tag, String> {
            self.expect('[')?;
            let kind = self.peek();
            self.index += 2;
            self.skip_whitespace();
            let mut values: Vec<i64> = Vec::new();
            let (bits, suffixes): (u32, &[char]) = match kind {
                Some('B') => (8, &['b'][..]),
                Some('I') => (32, &['i', 's', 'b'][..]),
                Some('L') => (64, &['l', 'i', 's', 'b'][..]),
                _ => return Err(self.error("invalid array type")),
            };
            while self.peek().is_some_and(|c| c != ']') {
                let text = self.simple();
                let value = integer(&text)
                    .and_then(|(value, suffix, unsigned)| match suffix {
                        None => fit(value, bits, unsigned),
                        Some(suffix) if suffixes.contains(&suffix) => {
                            fit(value, suffix_bits(suffix), unsigned)
                        }
                        Some(_) => None,
                    })
                    .ok_or_else(|| self.error(&format!("invalid array value {text}")))?;
                values.push(value);
                if !self.separator() {
                    break;
                }
            }
            self.expect(']')?;
            Ok(match kind {
                Some('B') => Tag::ByteArray(values.into_iter().map(|v| v as i8).collect()),
                Some('I') => Tag::IntArray(values.into_iter().map(|v| v as i32).collect()),
                _ => Tag::LongArray(values),
            })
        }

        fn list(&mut self) -> Result<Tag, String> {
            self.expect('[')?;
            self.skip_whitespace();
            let mut items: Vec<Tag> = Vec::new();
            while self.peek().is_some_and(|c| c != ']') {
                if self.separator() {
                    return Err(self.error("unexpected list separator"));
                }
                items.push(self.value()?);
                if !self.separator() {
                    break;
                }
            }
            self.expect(']')?;
            let mixed = items.windows(2).any(|pair| pair[0].id() != pair[1].id());
            if mixed {
                for item in items.iter_mut() {
                    if !matches!(item, Tag::Compound(_)) {
                        let value = std::mem::replace(item, Tag::Byte(0));
                        *item = Tag::Compound(std::iter::once((String::new(), value)).collect());
                    }
                }
            }
            Ok(Tag::List(items))
        }
    }

    fn suffix_bits(suffix: char) -> u32 {
        match suffix {
            'b' => 8,
            's' => 16,
            'l' => 64,
            _ => 32,
        }
    }

    /// A decimal integer literal: digits (underscores between them allowed), then optionally
    /// `s`/`u` (signed/unsigned) and a type suffix `b`, `s`, `i` or `l`.
    fn integer(text: &str) -> Option<(i128, Option<char>, bool)> {
        let mut body = text;
        let mut suffix = None;
        let mut unsigned = false;
        if let Some(last) = body.chars().last()
            && matches!(last.to_ascii_lowercase(), 'b' | 's' | 'i' | 'l')
        {
            suffix = Some(last.to_ascii_lowercase());
            body = &body[..body.len() - 1];
            if let Some(sign @ ('s' | 'u')) = body.chars().last() {
                unsigned = sign == 'u';
                body = &body[..body.len() - 1];
            }
        }
        let digits = body.strip_prefix(['-', '+']).unwrap_or(body);
        if !digits.starts_with(|c: char| c.is_ascii_digit())
            || !digits.ends_with(|c: char| c.is_ascii_digit())
            || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        let cleaned: String = body.chars().filter(|&c| c != '_').collect();
        cleaned.parse().ok().map(|value| (value, suffix, unsigned))
    }

    /// The value in range of a `bits` wide integer (unsigned ones wrap like Java's casts).
    fn fit(value: i128, bits: u32, unsigned: bool) -> Option<i64> {
        if unsigned {
            if value < 0 || value >= 1i128 << bits {
                return None;
            }
            let shift = 64 - bits;
            Some(((value as u64) << shift) as i64 >> shift)
        } else {
            let limit = 1i128 << (bits - 1);
            (-limit..limit).contains(&value).then_some(value as i64)
        }
    }

    fn float_body(text: &str) -> bool {
        text.chars().any(|c| c.is_ascii_digit())
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    }

    /// An unquoted value: a number with its type, else a string.
    fn literal(text: &str) -> Option<Tag> {
        if let Some((value, suffix, unsigned)) = integer(text) {
            return match suffix {
                Some('b') => fit(value, 8, unsigned).map(|v| Tag::Byte(v as i8)),
                Some('s') => fit(value, 16, unsigned).map(|v| Tag::Short(v as i16)),
                Some('l') => fit(value, 64, unsigned).map(Tag::Long),
                _ => fit(value, 32, unsigned).map(|v| Tag::Int(v as i32)),
            };
        }
        if let Some(body) = text.strip_suffix(['f', 'F'])
            && float_body(body)
            && let Ok(value) = body.parse::<f32>()
        {
            return Some(Tag::Float(value));
        }
        let body = text.strip_suffix(['d', 'D']).unwrap_or(text);
        if float_body(body)
            && let Ok(value) = body.parse::<f64>()
        {
            return Some(Tag::Double(value));
        }
        (text.is_empty() || unquoted(text)).then(|| Tag::String(text.to_string()))
    }

    /// Querz NBT's `SNBTWriter` without indentation: `{ Name: "minecraft:stone" }`.
    pub fn write(compound: &Compound) -> String {
        let mut out = String::new();
        write_compound(compound, &mut out);
        out
    }

    /// Java's `Float/Double.toString` closely enough to read back the same value.
    fn number(value: f64, shortest: String) -> String {
        let magnitude = value.abs();
        if value == 0.0 || (1e-3..1e7).contains(&magnitude) {
            if shortest.contains(['.', 'e', 'E']) || !value.is_finite() {
                shortest
            } else {
                format!("{shortest}.0")
            }
        } else {
            let scientific = format!("{value:e}");
            match scientific.split_once('e') {
                Some((mantissa, exponent)) if !mantissa.contains('.') => {
                    format!("{mantissa}.0E{exponent}")
                }
                Some((mantissa, exponent)) => format!("{mantissa}E{exponent}"),
                None => scientific,
            }
        }
    }

    fn write_string(value: &str, out: &mut String) {
        let mut quote = None;
        let mut body = String::new();
        for c in value.chars() {
            match c {
                '\\' => body.push_str("\\\\"),
                '"' | '\'' => {
                    let chosen = *quote.get_or_insert(if c == '"' { '\'' } else { '"' });
                    if chosen == c {
                        body.push('\\');
                    }
                    body.push(c);
                }
                '\u{8}' => body.push_str("\\b"),
                '\t' => body.push_str("\\t"),
                '\n' => body.push_str("\\n"),
                '\u{c}' => body.push_str("\\f"),
                '\r' => body.push_str("\\r"),
                c => body.push(c),
            }
        }
        let quote = quote.unwrap_or('"');
        out.push(quote);
        out.push_str(&body);
        out.push(quote);
    }

    fn write_tag(tag: &Tag, out: &mut String) {
        match tag {
            Tag::Byte(value) => out.push_str(&format!("{value}b")),
            Tag::Short(value) => out.push_str(&format!("{value}s")),
            Tag::Int(value) => out.push_str(&value.to_string()),
            Tag::Long(value) => out.push_str(&format!("{value}L")),
            Tag::Float(value) => {
                out.push_str(&number(*value as f64, value.to_string()));
                out.push('f');
            }
            Tag::Double(value) => {
                out.push_str(&number(*value, value.to_string()));
                out.push('d');
            }
            Tag::String(value) => write_string(value, out),
            Tag::ByteArray(values) => write_array("B", values.iter().map(|v| format!("{v}b")), out),
            Tag::IntArray(values) => write_array("I", values.iter().map(|v| v.to_string()), out),
            Tag::LongArray(values) => write_array("L", values.iter().map(|v| format!("{v}L")), out),
            Tag::List(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[ ");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_tag(item, out);
                }
                out.push_str(" ]");
            }
            Tag::Compound(compound) => write_compound(compound, out),
        }
    }

    fn write_compound(compound: &Compound, out: &mut String) {
        if compound.is_empty() {
            out.push_str("{}");
            return;
        }
        out.push_str("{ ");
        for (i, (key, value)) in compound.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if !key.is_empty() && key.chars().all(simple_char) {
                out.push_str(key);
            } else {
                write_string(key, out);
            }
            out.push_str(": ");
            write_tag(value, out);
        }
        out.push_str(" }");
    }

    fn write_array(prefix: &str, values: impl Iterator<Item = String>, out: &mut String) {
        out.push('[');
        out.push_str(prefix);
        out.push(';');
        let values: Vec<String> = values.collect();
        for (i, value) in values.iter().enumerate() {
            out.push(' ');
            out.push_str(value);
            if i + 1 < values.len() {
                out.push(',');
            }
        }
        out.push(']');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{anvil, chunk};

    const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/legacy.mca");
    const V26: &[u8] = include_bytes!("../tests/fixtures/v26.mca");
    const ENTITIES: &[u8] = include_bytes!("../tests/fixtures/entities.mca");

    fn chunks(region: &[u8]) -> Vec<(usize, Compound)> {
        anvil::slots(region)
            .unwrap()
            .into_iter()
            .map(|slot| (slot.index, anvil::chunk(region, slot).unwrap().unwrap()))
            .collect()
    }

    fn chunk_at(region: &[u8], index: usize) -> Compound {
        chunks(region)
            .into_iter()
            .find(|(i, _)| *i == index)
            .unwrap()
            .1
    }

    fn run(text: &str, root: &mut Compound, force: bool) -> Changed {
        let (mut timestamp, mut compression) = (0u32, 2u8);
        let mut chunk = ChunkMut {
            x: 0,
            z: 0,
            region: Some(root),
            entities: None,
            poi: None,
            timestamp: &mut timestamp,
            compression: &mut compression,
        };
        parse(text).unwrap().apply(&mut chunk, force)
    }

    /// Top-level keys whose values differ.
    fn changed_keys(before: &Compound, after: &Compound) -> Vec<String> {
        let mut keys: Vec<String> = before
            .iter()
            .chain(after.iter())
            .map(|(key, _)| key.to_string())
            .collect();
        keys.sort();
        keys.dedup();
        keys.retain(|key| before.get(key) != after.get(key));
        keys
    }

    fn level(root: &Compound) -> &Compound {
        root.compound("Level").unwrap_or(root)
    }

    fn level_mut(root: &mut Compound) -> &mut Compound {
        if root.contains("Level") {
            root.compound_mut("Level").unwrap()
        } else {
            root
        }
    }

    fn sections(root: &Compound) -> Vec<&Compound> {
        let container = level(root);
        container
            .list("sections")
            .or_else(|| container.list("Sections"))
            .unwrap()
            .iter()
            .filter_map(Tag::as_compound)
            .collect()
    }

    fn section_blocks(root: &Compound, section: &Compound) -> Option<(Vec<Tag>, Vec<u16>)> {
        let (palette, data) = match section.compound("block_states") {
            Some(states) => (
                states.list("palette")?,
                states.get("data").and_then(Tag::as_longs),
            ),
            None => (
                section.list("Palette")?,
                section.get("BlockStates").and_then(Tag::as_longs),
            ),
        };
        let indices = match data {
            Some(data) if palette.len() > 1 => unpack(
                data,
                bits_for(palette.len()).max(4),
                4096,
                data_version(root) < 2529,
            )
            .unwrap(),
            _ => vec![0; 4096],
        };
        Some((palette.to_vec(), indices))
    }

    /// How many blocks of a kind a chunk holds.
    fn count(root: &Compound, name: &str) -> usize {
        sections(root)
            .into_iter()
            .filter_map(|section| section_blocks(root, section))
            .map(|(palette, indices)| {
                indices
                    .iter()
                    .filter(|&&index| entry_name(&palette[index as usize]) == Some(name))
                    .count()
            })
            .sum()
    }

    fn heights(root: &Compound, key: &str) -> Vec<u16> {
        let longs = level(root)
            .compound("Heightmaps")
            .unwrap()
            .get(key)
            .and_then(Tag::as_longs)
            .unwrap();
        unpack(longs, 9, 256, data_version(root) < 2529).unwrap()
    }

    fn canonical(tag: &Tag) -> Tag {
        match tag {
            Tag::Compound(compound) => {
                let mut entries: Vec<(String, Tag)> = compound
                    .iter()
                    .map(|(key, value)| (key.to_string(), canonical(value)))
                    .collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Tag::Compound(entries.into_iter().collect())
            }
            Tag::List(items) => Tag::List(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }

    fn string(value: &str) -> Tag {
        Tag::String(value.into())
    }

    #[test]
    fn queries_round_trip() {
        for (input, normalized) in [
            ("Status=full", "Status = \"minecraft:full\""),
            ("Status = \"'custom'\"", "Status = \"'custom'\""),
            ("InhabitedTime = \"1 hour\"", "InhabitedTime = 72000"),
            (
                "LastUpdate = -5, Timestamp = +7",
                "LastUpdate = -5, Timestamp = 7",
            ),
            ("DataVersion = 3465", "DataVersion = 3465"),
            ("Compression = lz4", "Compression = LZ4"),
            ("Compression = 1", "Compression = GZIP"),
            ("Biome = plains", "Biome = \"minecraft:plains\""),
            ("Biome = 2", "Biome = \"minecraft:desert\""),
            ("Biome = \"'Custom:Thing'\"", "Biome = \"'custom:thing'\""),
            ("DeleteSections = true", "DeleteSections = true"),
            (
                "DeleteSections = \"4:, -2,:0, 1:3\"",
                "DeleteSections = \"4:, -2, :0, 1:3\"",
            ),
            ("DeleteSections = \":\"", "DeleteSections = true"),
            (
                "DeleteStructureReference = \"village, 'custom:thing'\"",
                "DeleteStructureReference = \"village, 'custom:thing'\"",
            ),
            (
                "DeleteEntities = 1, FixStatus = true, FixHeightmaps = 1, PreventRetrogen = 1, \
                 ForceBlend = 1, RemoveLightingInfo = 1, LightPopulated = 0",
                "DeleteEntities = true, FixStatus = true, FixHeightmaps = true, \
                 PreventRetrogen = true, ForceBlend = true, RemoveLightingInfo = true, \
                 LightPopulated = 0",
            ),
            (
                "ReplaceBlocks = \"stone=dirt, 'mod:x'={Name:'minecraft:oak_log',Properties:{axis:'x'}};{id:'minecraft:chest'}\"",
                "ReplaceBlocks = \"minecraft:stone=minecraft:dirt, 'mod:x'={ Name: \\\"minecraft:oak_log\\\", Properties: { axis: \\\"x\\\" } };{ id: \\\"minecraft:chest\\\" }\"",
            ),
            (
                "ReplaceBlocks = \"minecraft:air = 'mod:glow' , dirt=chest;{Items: [], Lock: \\\"\\\"}\"",
                "ReplaceBlocks = \"minecraft:air='mod:glow', minecraft:dirt=minecraft:chest;{ Items: [], Lock: \\\"\\\" }\"",
            ),
            ("", ""),
        ] {
            let set = parse(input).unwrap();
            assert_eq!(set.to_query(), normalized, "{input}");
            let again = parse(&set.to_query()).unwrap();
            assert_eq!(again, set, "{input}");
            assert_eq!(again.to_query(), normalized);
        }
        assert!(parse("  ").unwrap().is_empty());
        assert!(!parse("FixHeightmaps = true").unwrap().is_empty());
    }

    #[test]
    fn parse_errors() {
        for (input, message, position) in [
            ("Foo = 1", "invalid field", 3),
            ("Status full", "expected '=' but got 'f'", 7),
            ("Status = nope", "invalid value for Status", 13),
            ("Status =", "missing value for Status", 8),
            ("Status = full,", "expected a field after ','", 14),
            (
                "Status = full Biome = plains",
                "expected ',' but got 'B'",
                14,
            ),
            ("Status = \"full", "missing end quote", 14),
            ("Status = \"a\\b\"", "invalid escape of 'b'", 13),
            ("Script = x", "Script is not available in World Map", 6),
            ("Custom = x", "Custom is not available in World Map", 6),
            (
                "FixStructureReferences = 1",
                "FixStructureReferences is not available",
                22,
            ),
            ("Compression = NONE", "NONE marks a missing chunk", 18),
            ("Compression = \"LZ4_EXT\"", "external .mcc storage", 23),
            ("DataVersion = -1", "invalid value for DataVersion", 16),
            ("LightPopulated = 2", "expected 0 or 1", 18),
            ("FixHeightmaps = false", "expected true or 1", 21),
            (
                "InhabitedTime = \"5 minutes 3 minutes\"",
                "invalid value for InhabitedTime",
                37,
            ),
            ("DeleteSections = \"a\"", "invalid section range", 20),
            ("DeleteSections = \"1,,2\"", "invalid section range", 23),
            ("Biome = nowhere", "unknown biome", 15),
            (
                "DeleteStructureReference = castle",
                "unknown structure castle",
                33,
            ),
            (
                "ReplaceBlocks = \"notablock=stone\"",
                "unknown block notablock",
                33,
            ),
            (
                "ReplaceBlocks = \"stone=notablock\"",
                "unknown block notablock",
                33,
            ),
            ("ReplaceBlocks = \"stone\"", "expected <from>=<to>", 23),
            (
                "ReplaceBlocks = \"stone=dirt;\"",
                "expected a block entity compound",
                29,
            ),
            (
                "ReplaceBlocks = \"stone={Name:\"",
                "invalid value for ReplaceBlocks",
                30,
            ),
            ("ReplaceBlocks = \"stone='x\"", "missing closing '", 26),
        ] {
            let error = parse(input).unwrap_err();
            assert!(
                error.message.contains(message),
                "{input}: got {:?}",
                error.message
            );
            assert_eq!(error.position, position, "{input}: {}", error.message);
        }
    }

    #[test]
    fn durations() {
        assert_eq!(duration_ticks("1 hour 30 minutes"), Some(5400 * 20));
        assert_eq!(duration_ticks("2d"), Some(2 * 86_400 * 20));
        assert_eq!(duration_ticks("1 year, 2 months"), None);
        assert_eq!(
            duration_ticks("1year 2months 3s"),
            Some((31_536_000 + 2 * 2_592_000 + 3) * 20)
        );
        assert_eq!(duration_ticks("5 m"), None);
        assert_eq!(duration_ticks("1 hour 1 hour"), None);
        assert_eq!(duration_ticks(""), None);
        assert_eq!(ticks("-5"), Ok(-5));
        assert_eq!(ticks("10 secs"), Ok(200));
    }

    #[test]
    fn snbt_reads_and_writes_querz_style() {
        let text: Vec<char> =
            "{a:1b, b:[I; 1, 2], c:[1, \"x\"], d:'q\"uote', e:1.5f, f:-3L, g:2.0, h:7, i:200ub, j:x.y} rest"
                .chars()
                .collect();
        let (compound, read) = snbt::parse_compound(&text).unwrap();
        assert_eq!(text[read..].iter().collect::<String>(), " rest");
        assert_eq!(compound.get("a"), Some(&Tag::Byte(1)));
        assert_eq!(compound.get("b"), Some(&Tag::IntArray(vec![1, 2])));
        assert_eq!(compound.get("e"), Some(&Tag::Float(1.5)));
        assert_eq!(compound.get("f"), Some(&Tag::Long(-3)));
        assert_eq!(compound.get("g"), Some(&Tag::Double(2.0)));
        assert_eq!(compound.get("h"), Some(&Tag::Int(7)));
        assert_eq!(compound.get("i"), Some(&Tag::Byte(-56)));
        assert_eq!(compound.get("j"), Some(&string("x.y")));
        assert_eq!(
            snbt::write(&compound),
            "{ a: 1b, b: [I; 1, 2], c: [ { \"\": 1 }, { \"\": \"x\" } ], d: 'q\"uote', e: 1.5f, \
             f: -3L, g: 2.0d, h: 7, i: -56b, j: \"x.y\" }"
        );
        let written: Vec<char> = snbt::write(&compound).chars().collect();
        assert_eq!(snbt::parse_compound(&written).unwrap().0, compound);
        for bad in [
            "{a:300b}",
            "{1a:1}",
            "{a:1,,b:2}",
            "{a:[B; 1, 2s]}",
            "{a:'\\q'}",
            "{a:1",
        ] {
            let chars: Vec<char> = bad.chars().collect();
            assert!(snbt::parse_compound(&chars).is_err(), "{bad}");
        }
    }

    #[test]
    fn needs_follow_the_fields() {
        let needs = |text: &str| parse(text).unwrap().needs();
        assert_eq!(needs(""), Needs::default());
        assert_eq!(
            needs("Status = full"),
            Needs {
                region: true,
                entities: false,
                poi: false
            }
        );
        assert_eq!(
            needs("DeleteEntities = true"),
            Needs {
                region: true,
                entities: true,
                poi: false
            }
        );
        assert_eq!(
            needs("DeleteSections = \"1\""),
            Needs {
                region: true,
                entities: true,
                poi: false
            }
        );
        assert_eq!(
            needs("DataVersion = 1"),
            Needs {
                region: true,
                entities: true,
                poi: true
            }
        );
    }

    #[test]
    fn data_version_changes_existing_tags_or_forces_them() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        let mut entities = chunk_at(ENTITIES, 247);
        let mut poi = Compound::new();
        let (mut timestamp, mut compression) = (0u32, 2u8);
        let set = parse("DataVersion = 4325").unwrap();
        let mut chunk = ChunkMut {
            x: 0,
            z: 0,
            region: Some(&mut root),
            entities: Some(&mut entities),
            poi: Some(&mut poi),
            timestamp: &mut timestamp,
            compression: &mut compression,
        };
        let changed = set.apply(&mut chunk, false);
        assert_eq!(
            changed,
            Changed {
                region: true,
                entities: true,
                poi: false,
                header: false
            }
        );
        let changed = set.apply(&mut chunk, true);
        assert_eq!(
            changed,
            Changed {
                region: false,
                entities: false,
                poi: true,
                header: false
            }
        );
        assert_eq!(changed_keys(&before, &root), ["DataVersion"]);
        assert_eq!(root.get("DataVersion"), Some(&Tag::Int(4325)));
        assert_eq!(entities.get("DataVersion"), Some(&Tag::Int(4325)));
        assert_eq!(poi.get("DataVersion"), Some(&Tag::Int(4325)));
    }

    #[test]
    fn inhabited_time_and_last_update() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        let changed = run(
            "InhabitedTime = \"1 minute\", LastUpdate = 0",
            &mut root,
            false,
        );
        assert!(changed.region && !changed.header && !changed.entities);
        assert_eq!(
            changed_keys(&before, &root),
            ["InhabitedTime", "LastUpdate"]
        );
        assert_eq!(root.get("InhabitedTime"), Some(&Tag::Long(1200)));
        assert_eq!(root.get("LastUpdate"), Some(&Tag::Long(0)));
        // the same values again change nothing
        assert_eq!(
            run("InhabitedTime = 1200", &mut root, false),
            Changed::default()
        );

        // 1.16: in Level, and absent there, so only forcing sets it
        let mut legacy = chunk_at(LEGACY, 0);
        assert_eq!(
            run("InhabitedTime = 5", &mut legacy, false),
            Changed::default()
        );
        let before = level(&legacy).clone();
        assert!(run("InhabitedTime = 5", &mut legacy, true).region);
        assert_eq!(changed_keys(&before, level(&legacy)), ["InhabitedTime"]);
        assert_eq!(level(&legacy).get("InhabitedTime"), Some(&Tag::Long(5)));
        assert!(!legacy.contains("InhabitedTime"));
    }

    #[test]
    fn status_and_fix_status() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        assert!(run("Status = empty", &mut root, false).region);
        assert_eq!(changed_keys(&before, &root), ["Status"]);
        assert_eq!(root.str("Status"), Some("minecraft:empty"));
        assert!(run("FixStatus = true", &mut root, false).region);
        assert_eq!(root.str("Status"), Some("minecraft:full"));
        assert_eq!(
            run("FixStatus = true", &mut root, false),
            Changed::default()
        );
        run("Status = \"'weird'\"", &mut root, false);
        assert_eq!(root.str("Status"), Some("weird"));

        // statuses before 23w12a have no namespace
        let mut legacy = chunk_at(LEGACY, 0);
        run("Status = \"minecraft:empty\"", &mut legacy, false);
        assert_eq!(level(&legacy).str("Status"), Some("empty"));
        assert!(run("FixStatus = 1", &mut legacy, false).region);
        assert_eq!(level(&legacy).str("Status"), Some("full"));

        // missing: only forced
        root.remove("Status");
        assert_eq!(run("Status = full", &mut root, false), Changed::default());
        assert!(run("Status = full", &mut root, true).region);
        assert_eq!(root.str("Status"), Some("minecraft:full"));
    }

    #[test]
    fn timestamp_and_compression_change_the_header_entry() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        let (mut timestamp, mut compression) = (5u32, 2u8);
        let set = parse("Timestamp = 1700000000, Compression = GZIP").unwrap();
        let mut chunk = ChunkMut {
            x: -12,
            z: 1,
            region: Some(&mut root),
            entities: None,
            poi: None,
            timestamp: &mut timestamp,
            compression: &mut compression,
        };
        assert_eq!(
            set.apply(&mut chunk, false),
            Changed {
                region: true,
                entities: false,
                poi: false,
                header: true
            }
        );
        assert_eq!(set.apply(&mut chunk, false), Changed::default());
        assert_eq!((timestamp, compression), (1_700_000_000, 1));
        assert_eq!(root, before);

        // no chunk there: nothing to stamp
        let (mut timestamp, mut compression) = (5u32, 2u8);
        let mut empty = ChunkMut {
            x: 0,
            z: 0,
            region: None,
            entities: None,
            poi: None,
            timestamp: &mut timestamp,
            compression: &mut compression,
        };
        assert_eq!(set.apply(&mut empty, true), Changed::default());
        assert_eq!((timestamp, compression), (5, 2));
    }

    #[test]
    fn biomes_per_section_and_legacy_arrays() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        assert!(run("Biome = plains", &mut root, false).region);
        assert_eq!(changed_keys(&before, &root), ["sections"]);
        for (old, new) in sections(&before).into_iter().zip(sections(&root)) {
            match old.compound("biomes") {
                Some(_) => {
                    let biomes = new.compound("biomes").unwrap();
                    assert_eq!(
                        biomes.list("palette").unwrap(),
                        [string("minecraft:plains")]
                    );
                    assert!(!biomes.contains("data"));
                    assert!(changed_keys(old, new).iter().all(|key| key == "biomes"));
                }
                None => assert_eq!(old, new),
            }
        }
        // forcing gives every section (also the light-only one) biomes
        run("Biome = plains", &mut root, true);
        assert!(
            sections(&root)
                .iter()
                .all(|section| section.contains("biomes"))
        );

        let mut legacy = chunk_at(LEGACY, 0);
        let before = level(&legacy).clone();
        assert!(run("Biome = desert", &mut legacy, false).region);
        assert_eq!(changed_keys(&before, level(&legacy)), ["Biomes"]);
        assert_eq!(
            level(&legacy).get("Biomes"),
            Some(&Tag::IntArray(vec![2; 1024]))
        );
        level_mut(&mut legacy).remove("Biomes");
        assert_eq!(
            run("Biome = desert", &mut legacy, false),
            Changed::default()
        );
        run("Biome = desert", &mut legacy, true);
        assert_eq!(
            level(&legacy).get("Biomes"),
            Some(&Tag::IntArray(vec![2; 1024]))
        );
        // 1.13 (before 19w36a): 256 columns
        let mut old = chunk_at(LEGACY, 1);
        run("Biome = 1", &mut old, true);
        assert_eq!(
            level(&old).get("Biomes"),
            Some(&Tag::IntArray(vec![1; 256]))
        );
        // a custom name has no legacy id: 1.16 chunks are skipped
        assert_eq!(
            run("Biome = \"'mod:thing'\"", &mut legacy, true),
            Changed::default()
        );
    }

    #[test]
    fn replace_blocks_in_modern_chunks() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        let bedrock = count(&root, "minecraft:bedrock");
        let stone = count(&root, "minecraft:stone");
        assert!(bedrock > 0);
        assert!(
            run(
                "ReplaceBlocks = \"bedrock=diamond_block\"",
                &mut root,
                false
            )
            .region
        );
        assert_eq!(count(&root, "minecraft:bedrock"), 0);
        assert_eq!(count(&root, "minecraft:diamond_block"), bedrock);
        assert_eq!(count(&root, "minecraft:stone"), stone);
        // lighting is dropped for the game to redo, heightmaps stay right (bedrock is at the bottom)
        assert_eq!(changed_keys(&before, &root), ["sections"]);
        assert_eq!(root.get("isLightOn"), Some(&Tag::Byte(0)));
        assert!(
            sections(&root)
                .iter()
                .all(|section| !section.contains("BlockLight"))
        );
        let palette = |root: &Compound, y: i64| {
            sections(root)
                .into_iter()
                .find(|section| section.i64("Y") == Some(y))
                .and_then(|section| section.compound("block_states")?.list("palette"))
                .unwrap()
                .to_vec()
        };
        let bottom = palette(&root, -4);
        assert!(bottom.contains(&named("Name", string("minecraft:diamond_block"))));
        assert!(
            !bottom
                .iter()
                .any(|entry| entry_name(entry) == Some("minecraft:bedrock"))
        );
        // sections without bedrock keep their palette
        assert_eq!(palette(&root, 2), palette(&before, 2));
        assert!(matches!(
            chunk::surface(&root, None).unwrap(),
            chunk::Parsed::Surface(_)
        ));
    }

    #[test]
    fn replace_blocks_handles_block_entities() {
        let mut root = chunk_at(OVERWORLD, 52);
        let bottom = sections(&root)
            .into_iter()
            .find(|section| section.i64("Y") == Some(-4))
            .unwrap()
            .clone();
        let (palette, indices) = section_blocks(&root, &bottom).unwrap();
        let first = indices
            .iter()
            .position(|&index| entry_name(&palette[index as usize]) == Some("minecraft:bedrock"))
            .unwrap();
        let position = (
            -12 * 16 + (first & 15) as i32,
            -64 + (first >> 8) as i32,
            16 + ((first >> 4) & 15) as i32,
        );
        let chest: Compound = [
            ("id".to_string(), string("minecraft:chest")),
            ("x".to_string(), Tag::Int(position.0)),
            ("y".to_string(), Tag::Int(position.1)),
            ("z".to_string(), Tag::Int(position.2)),
        ]
        .into_iter()
        .collect();
        let with_chest = |root: &mut Compound| {
            root.insert(
                "block_entities",
                Tag::List(vec![Tag::Compound(chest.clone())]),
            );
        };
        with_chest(&mut root);
        let mut replaced = root.clone();
        run("ReplaceBlocks = \"bedrock=stone\"", &mut replaced, false);
        assert_eq!(replaced.list("block_entities").unwrap(), []);

        let bedrock = count(&root, "minecraft:bedrock");
        run(
            "ReplaceBlocks = \"bedrock=chest;{id:'minecraft:chest',Items:[]}\"",
            &mut root,
            false,
        );
        let tiles = root.list("block_entities").unwrap();
        assert_eq!(tiles.len(), bedrock);
        let at_chest: Vec<&Compound> = tiles
            .iter()
            .filter_map(Tag::as_compound)
            .filter(|tile| {
                (tile.i64("x"), tile.i64("y"), tile.i64("z"))
                    == (
                        Some(position.0 as i64),
                        Some(position.1 as i64),
                        Some(position.2 as i64),
                    )
            })
            .collect();
        assert_eq!(at_chest.len(), 1);
        assert_eq!(at_chest[0].list("Items").unwrap(), []);
        assert_eq!(count(&root, "minecraft:chest"), bedrock);
    }

    #[test]
    fn replace_blocks_in_26_palettes() {
        let mut root = chunk_at(V26, 97);
        let deepslate = count(&root, "minecraft:deepslate");
        run("ReplaceBlocks = \"deepslate=stone\"", &mut root, false);
        assert_eq!(count(&root, "minecraft:stone") >= deepslate, true);
        assert_eq!(count(&root, "minecraft:deepslate"), 0);
        let bottom = sections(&root)
            .into_iter()
            .find(|section| section.i64("Y") == Some(-4))
            .unwrap();
        let palette = bottom
            .compound("block_states")
            .unwrap()
            .list("palette")
            .unwrap();
        assert!(palette.iter().all(|entry| matches!(entry, Tag::String(_))));
        assert!(palette.contains(&string("minecraft:stone")));

        let tuff = count(&root, "minecraft:tuff");
        run(
            "ReplaceBlocks = \"tuff={id:'minecraft:oak_log',properties:{axis:'x'}}\"",
            &mut root,
            false,
        );
        assert_eq!(count(&root, "minecraft:oak_log"), tuff);
        let bottom = sections(&root)
            .into_iter()
            .find(|section| section.i64("Y") == Some(-4))
            .unwrap();
        let palette = bottom
            .compound("block_states")
            .unwrap()
            .list("palette")
            .unwrap();
        // mixed entries: strings are wrapped as {"": name}
        assert!(
            palette
                .iter()
                .all(|entry| matches!(entry, Tag::Compound(_)))
        );
        assert!(palette.contains(&named("", string("minecraft:stone"))));
        assert!(matches!(
            chunk::surface(&root, None).unwrap(),
            chunk::Parsed::Surface(_)
        ));
    }

    #[test]
    fn replace_blocks_in_legacy_chunks() {
        for index in [0, 1] {
            let mut root = chunk_at(LEGACY, index);
            let stone = count(&root, "minecraft:stone");
            let grass = count(&root, "minecraft:grass_block");
            assert!(run("ReplaceBlocks = \"stone=dirt\"", &mut root, false).region);
            assert_eq!(count(&root, "minecraft:dirt"), stone, "chunk {index}");
            assert_eq!(count(&root, "minecraft:stone"), 0);
            assert_eq!(count(&root, "minecraft:grass_block"), grass);
            assert!(matches!(
                chunk::surface(&root, None).unwrap(),
                chunk::Parsed::Surface(_)
            ));
            assert!(level(&root).contains("Heightmaps"));
            assert!(!level(&root).contains("TileEntities"));
        }
        // isLightOn exists since 19w02a
        let mut root = chunk_at(LEGACY, 0);
        run("ReplaceBlocks = \"stone=dirt\"", &mut root, false);
        assert_eq!(level(&root).get("isLightOn"), Some(&Tag::Byte(0)));
        let mut root = chunk_at(LEGACY, 1);
        run("ReplaceBlocks = \"stone=dirt\"", &mut root, false);
        assert!(!level(&root).contains("isLightOn"));
        // before 1.13 there are no palettes
        let mut ancient = chunk_at(LEGACY, 2);
        let before = ancient.clone();
        run("ReplaceBlocks = \"stone=dirt\"", &mut ancient, false);
        assert_eq!(ancient, before);
    }

    #[test]
    fn replacing_air_fills_the_world_height() {
        let mut root = chunk_at(LEGACY, 0);
        let air = count(&root, "minecraft:air");
        run("ReplaceBlocks = \"air=glass\"", &mut root, false);
        let ys: Vec<i64> = sections(&root)
            .iter()
            .map(|s| s.i64("Y").unwrap())
            .collect();
        assert_eq!(ys, (0..16).collect::<Vec<_>>());
        assert_eq!(count(&root, "minecraft:glass"), air + 12 * 4096);
        assert_eq!(heights(&root, "WORLD_SURFACE"), vec![256; 256]);
    }

    #[test]
    fn delete_entities() {
        let mut entities = chunk_at(ENTITIES, 247);
        assert_eq!(entities.list("Entities").unwrap().len(), 3);
        let (mut timestamp, mut compression) = (0u32, 2u8);
        let mut chunk = ChunkMut {
            x: -9,
            z: 7,
            region: None,
            entities: Some(&mut entities),
            poi: None,
            timestamp: &mut timestamp,
            compression: &mut compression,
        };
        let set = parse("DeleteEntities = true").unwrap();
        assert_eq!(
            set.apply(&mut chunk, false),
            Changed {
                region: false,
                entities: true,
                poi: false,
                header: false
            }
        );
        assert_eq!(set.apply(&mut chunk, false), Changed::default());
        assert_eq!(entities.list("Entities").unwrap(), []);

        // proto-entities in the region/ chunk (1.18+) and Level.Entities (before 1.17)
        let pig = named("id", string("minecraft:pig"));
        let mut root = chunk_at(OVERWORLD, 9);
        assert_eq!(
            run("DeleteEntities = true", &mut root, false),
            Changed::default()
        );
        root.insert("entities", Tag::List(vec![pig.clone()]));
        assert!(run("DeleteEntities = true", &mut root, false).region);
        assert_eq!(root.list("entities").unwrap(), []);
        let mut legacy = chunk_at(LEGACY, 0);
        level_mut(&mut legacy).insert("Entities", Tag::List(vec![pig]));
        assert!(run("DeleteEntities = true", &mut legacy, false).region);
        assert_eq!(level(&legacy).list("Entities").unwrap(), []);
    }

    #[test]
    fn delete_sections_in_modern_chunks() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        assert!(run("DeleteSections = \"0:3\"", &mut root, false).region);
        assert_eq!(changed_keys(&before, &root), ["Status", "sections"]);
        assert_eq!(root.str("Status"), Some("minecraft:features"));
        for (old, new) in sections(&before).into_iter().zip(sections(&root)) {
            let y = old.i64("Y").unwrap();
            if (0..=3).contains(&y) {
                let states = new.compound("block_states").unwrap();
                assert_eq!(
                    states.list("palette").unwrap(),
                    [named("Name", string("minecraft:air"))]
                );
                assert!(!states.contains("data") && !new.contains("BlockLight"));
                assert_eq!(new.get("biomes"), old.get("biomes"));
            } else {
                assert_eq!(old, new);
            }
        }
        // chunks still being generated are left alone
        let mut proto = chunk_at(OVERWORLD, 9);
        let before = proto.clone();
        assert_eq!(
            run("DeleteSections = true", &mut proto, false),
            Changed::default()
        );
        assert_eq!(proto, before);

        // entities in the deleted sections go too (the rabbits stand at y 64, section 4)
        for (ranges, left) in [("\"0:3\"", 3), ("\"4\"", 0)] {
            let mut entities = chunk_at(ENTITIES, 247);
            let (mut timestamp, mut compression) = (0u32, 2u8);
            let mut chunk = ChunkMut {
                x: -9,
                z: 7,
                region: None,
                entities: Some(&mut entities),
                poi: None,
                timestamp: &mut timestamp,
                compression: &mut compression,
            };
            let changed = parse(&format!("DeleteSections = {ranges}"))
                .unwrap()
                .apply(&mut chunk, false);
            assert_eq!(changed.entities, left == 0);
            assert_eq!(entities.list("Entities").unwrap().len(), left);
        }
    }

    #[test]
    fn delete_sections_in_legacy_chunks() {
        let mut root = chunk_at(LEGACY, 0);
        let before = level(&root).clone();
        assert!(run("DeleteSections = \"1\"", &mut root, false).region);
        assert_eq!(
            changed_keys(&before, level(&root)),
            ["Heightmaps", "Sections"]
        );
        let ys: Vec<i64> = sections(&root)
            .iter()
            .map(|s| s.i64("Y").unwrap())
            .collect();
        assert_eq!(ys, [0, 2, 3]);
        assert_eq!(heights(&root, "WORLD_SURFACE"), vec![61; 256]);
    }

    #[test]
    fn fixed_heightmaps_match_the_game() {
        for (region, index) in [
            (OVERWORLD, 52),
            (OVERWORLD, 53),
            (OVERWORLD, 84),
            (OVERWORLD, 85),
            (V26, 97),
            (V26, 207),
            (V26, 239),
        ] {
            let mut root = chunk_at(region, index);
            let original = root.compound("Heightmaps").unwrap().clone();
            assert!(!fix_heightmaps(&mut root), "chunk {index} already right");
            root.remove("Heightmaps");
            assert!(run("FixHeightmaps = true", &mut root, false).region);
            let fixed = root.compound("Heightmaps").unwrap();
            for (key, value) in original.iter() {
                assert_eq!(fixed.get(key), Some(value), "{key} of chunk {index}");
            }
            assert_eq!(fixed.len(), 4);

            // the world surface is the topmost visible block World Map draws
            let chunk::Parsed::Surface(surface) = chunk::surface(&root, None).unwrap() else {
                panic!("no surface");
            };
            let world_surface = heights(&root, "WORLD_SURFACE");
            for (column, top) in surface.columns.iter().enumerate() {
                if let Some(top) = top {
                    assert_eq!(
                        world_surface[column] as i32 - 1 - 64,
                        top.y,
                        "column {column}"
                    );
                }
            }
        }
    }

    #[test]
    fn fixed_heightmaps_of_legacy_chunks() {
        // 1.16: 37 longs of 9 bits, values above the bottom of the world (y 0)
        let mut root = chunk_at(LEGACY, 0);
        assert!(fix_heightmaps(&mut root));
        let heightmaps = level(&root).compound("Heightmaps").unwrap();
        let keys: Vec<&str> = heightmaps.iter().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            [
                "WORLD_SURFACE",
                "OCEAN_FLOOR",
                "MOTION_BLOCKING",
                "MOTION_BLOCKING_NO_LEAVES"
            ]
        );
        assert_eq!(
            heightmaps
                .get("WORLD_SURFACE")
                .and_then(Tag::as_longs)
                .unwrap()
                .len(),
            37
        );
        let pool =
            |column: usize| (4..8).contains(&(column % 16)) && (4..8).contains(&(column / 16));
        let ocean_floor = heights(&root, "OCEAN_FLOOR");
        for column in 0..256 {
            assert_eq!(ocean_floor[column], if pool(column) { 60 } else { 61 });
        }
        assert_eq!(heights(&root, "MOTION_BLOCKING"), vec![61; 256]);
        assert_eq!(heights(&root, "WORLD_SURFACE"), vec![61; 256]);
        assert!(!fix_heightmaps(&mut root));

        // 1.13: spanning longs (36) and the LIGHT_BLOCKING heightmap of the time
        let mut old = chunk_at(LEGACY, 1);
        assert!(fix_heightmaps(&mut old));
        let heightmaps = level(&old).compound("Heightmaps").unwrap();
        assert_eq!(heightmaps.len(), 5);
        assert!(heightmaps.contains("LIGHT_BLOCKING"));
        assert_eq!(
            heightmaps
                .get("WORLD_SURFACE")
                .and_then(Tag::as_longs)
                .unwrap()
                .len(),
            36
        );
        assert_eq!(heights(&old, "WORLD_SURFACE"), vec![61; 256]);

        // pre-1.13 chunks are left alone
        let mut ancient = chunk_at(LEGACY, 2);
        assert!(!fix_heightmaps(&mut ancient));
    }

    #[test]
    fn delete_structure_references() {
        let references: Compound = [
            ("minecraft:mineshaft".to_string(), Tag::LongArray(vec![1])),
            ("minecraft:village".to_string(), Tag::LongArray(vec![2])),
        ]
        .into_iter()
        .collect();
        let starts: Compound = std::iter::once((
            "minecraft:mineshaft".to_string(),
            named("id", string("minecraft:mineshaft")),
        ))
        .collect();

        let mut root = chunk_at(OVERWORLD, 52);
        let structures = root.compound_mut("structures").unwrap();
        structures.insert("References", Tag::Compound(references.clone()));
        structures.insert("starts", Tag::Compound(starts.clone()));
        let before = root.clone();
        assert!(run("DeleteStructureReference = mineshaft", &mut root, false).region);
        assert_eq!(changed_keys(&before, &root), ["structures"]);
        let structures = root.compound("structures").unwrap();
        let keys = |c: &Compound| c.iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>();
        assert_eq!(
            keys(structures.compound("References").unwrap()),
            ["minecraft:village"]
        );
        assert!(structures.compound("starts").unwrap().is_empty());
        assert_eq!(
            run("DeleteStructureReference = mineshaft", &mut root, false),
            Changed::default()
        );

        // before 1.18: Level.Structures with References and Starts, older names too
        let mut legacy = chunk_at(LEGACY, 0);
        let old: Compound = [
            (
                "References".to_string(),
                named("Mineshaft", Tag::LongArray(vec![1])),
            ),
            (
                "Starts".to_string(),
                named("Mineshaft", named("id", string("Mineshaft"))),
            ),
        ]
        .into_iter()
        .collect();
        level_mut(&mut legacy).insert("Structures", Tag::Compound(old));
        assert!(
            run(
                "DeleteStructureReference = \"minecraft:mineshaft\"",
                &mut legacy,
                false
            )
            .region
        );
        let structures = level(&legacy).compound("Structures").unwrap();
        assert!(structures.compound("References").unwrap().is_empty());
        assert!(structures.compound("Starts").unwrap().is_empty());
    }

    #[test]
    fn prevent_retrogen_and_force_blend() {
        let mut root = chunk_at(OVERWORLD, 52);
        root.insert(
            "below_zero_retrogen",
            named("target_status", string("minecraft:spawn")),
        );
        let before = root.clone();
        assert!(run("PreventRetrogen = true", &mut root, false).region);
        assert_eq!(
            changed_keys(&before, &root),
            ["Status", "below_zero_retrogen"]
        );
        assert_eq!(root.str("Status"), Some("full"));

        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        assert!(run("ForceBlend = true", &mut root, false).region);
        assert_eq!(
            changed_keys(&before, &root),
            ["Heightmaps", "blending_data", "isLightOn"]
        );
        let blending = root.compound("blending_data").unwrap();
        assert_eq!(blending.get("min_section"), Some(&Tag::Int(-5)));
        assert_eq!(blending.get("max_section"), Some(&Tag::Int(20)));
        assert!(!root.contains("Heightmaps") && !root.contains("isLightOn"));

        // 25w02a to 1.21.5-rc1: back to 1.21.4's DataVersion
        let mut root = chunk_at(OVERWORLD, 52);
        root.insert("DataVersion", Tag::Int(4320));
        let before = root.clone();
        run("ForceBlend = true", &mut root, false);
        assert_eq!(changed_keys(&before, &root), ["DataVersion"]);
        assert_eq!(root.get("DataVersion"), Some(&Tag::Int(4189)));

        // nothing before 1.18
        let mut legacy = chunk_at(LEGACY, 0);
        let before = legacy.clone();
        assert_eq!(
            run("ForceBlend = 1, PreventRetrogen = 1", &mut legacy, false),
            Changed::default()
        );
        assert_eq!(legacy, before);
    }

    #[test]
    fn lighting() {
        let mut root = chunk_at(OVERWORLD, 52);
        let before = root.clone();
        assert!(
            sections(&root)
                .iter()
                .any(|section| section.contains("BlockLight"))
        );
        assert!(run("RemoveLightingInfo = true", &mut root, false).region);
        assert_eq!(changed_keys(&before, &root), ["sections"]);
        assert!(
            sections(&root)
                .iter()
                .all(|section| !section.contains("BlockLight") && !section.contains("SkyLight"))
        );
        assert!(run("LightPopulated = 1", &mut root, false).region);
        assert_eq!(root.get("isLightOn"), Some(&Tag::Byte(1)));

        let mut legacy = chunk_at(LEGACY, 0);
        assert_eq!(
            run("LightPopulated = 1", &mut legacy, false),
            Changed::default()
        );
        assert!(run("LightPopulated = 1", &mut legacy, true).region);
        assert_eq!(level(&legacy).get("isLightOn"), Some(&Tag::Byte(1)));
        assert!(run("RemoveLightingInfo = 1", &mut legacy, false).region);
        assert_eq!(level(&legacy).get("isLightOn"), Some(&Tag::Byte(0)));
        // 18w06a to 19w01a had no light flag
        let mut old = chunk_at(LEGACY, 1);
        assert_eq!(
            run("LightPopulated = 1", &mut old, true),
            Changed::default()
        );
    }

    const MCA_BEFORE_REGION: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/change/before/region/r.-1.0.mca");
    const MCA_BEFORE_ENTITIES: &[u8] =
        include_bytes!("../tests/fixtures/mcaselector/change/before/entities/r.-1.0.mca");
    const MCA_CHANGES: &str = include_str!("../tests/fixtures/mcaselector/inputs/changes.tsv");
    const MCA_AFTER: &[(&str, &[u8], &[u8])] = &[
        (
            "inhabited-lastupdate",
            include_bytes!(
                "../tests/fixtures/mcaselector/change/inhabited-lastupdate/region/r.-1.0.mca"
            )
            .as_slice(),
            include_bytes!(
                "../tests/fixtures/mcaselector/change/inhabited-lastupdate/entities/r.-1.0.mca"
            )
            .as_slice(),
        ),
        (
            "status",
            include_bytes!("../tests/fixtures/mcaselector/change/status/region/r.-1.0.mca")
                .as_slice(),
            include_bytes!("../tests/fixtures/mcaselector/change/status/entities/r.-1.0.mca")
                .as_slice(),
        ),
        (
            "delete-sections",
            include_bytes!(
                "../tests/fixtures/mcaselector/change/delete-sections/region/r.-1.0.mca"
            )
            .as_slice(),
            include_bytes!(
                "../tests/fixtures/mcaselector/change/delete-sections/entities/r.-1.0.mca"
            )
            .as_slice(),
        ),
        (
            "delete-entities",
            include_bytes!(
                "../tests/fixtures/mcaselector/change/delete-entities/region/r.-1.0.mca"
            )
            .as_slice(),
            include_bytes!(
                "../tests/fixtures/mcaselector/change/delete-entities/entities/r.-1.0.mca"
            )
            .as_slice(),
        ),
    ];

    /// MCA Selector 2.9's own output for the same changes (tests/fixtures/mcaselector/README.md).
    #[test]
    fn matches_mca_selector() {
        let before_entities = chunks(MCA_BEFORE_ENTITIES);
        for (folder, region, entities) in MCA_AFTER {
            let changes = MCA_CHANGES
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{folder}\t")))
                .unwrap();
            let set = parse(changes).unwrap();
            assert_eq!(parse(&set.to_query()).unwrap(), set);
            let expected_region = chunks(region);
            let expected_entities = chunks(entities);
            for (index, mut root) in chunks(MCA_BEFORE_REGION) {
                let mut entity_chunk = before_entities
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map(|(_, compound)| compound.clone());
                let (mut timestamp, mut compression) = (0u32, 2u8);
                let mut chunk = ChunkMut {
                    x: 0,
                    z: 0,
                    region: Some(&mut root),
                    entities: entity_chunk.as_mut(),
                    poi: None,
                    timestamp: &mut timestamp,
                    compression: &mut compression,
                };
                let changed = set.apply(&mut chunk, false);
                let (_, want) = expected_region.iter().find(|(i, _)| *i == index).unwrap();
                assert_eq!(
                    canonical(&Tag::Compound(root.clone())),
                    canonical(&Tag::Compound(want.clone())),
                    "{folder}: region chunk {index}"
                );
                let original = chunks(MCA_BEFORE_REGION)
                    .into_iter()
                    .find(|(i, _)| *i == index)
                    .unwrap()
                    .1;
                assert_eq!(changed.region, root != original, "{folder}: chunk {index}");
                let Some(entity_chunk) = entity_chunk else {
                    continue;
                };
                let (_, want) = expected_entities.iter().find(|(i, _)| *i == index).unwrap();
                match *folder {
                    // MCA Selector 2.9 finds no entities in 1.17+ chunks (it looks under
                    // `Level`); World Map deletes them: the rabbits stand in section 4
                    "delete-entities" | "delete-sections" => {
                        assert_eq!(want.list("Entities").unwrap().len(), 3);
                        assert_eq!(entity_chunk.list("Entities").unwrap(), []);
                        assert!(changed.entities);
                    }
                    _ => {
                        assert_eq!(
                            canonical(&Tag::Compound(entity_chunk)),
                            canonical(&Tag::Compound(want.clone())),
                            "{folder}: entities chunk {index}"
                        );
                        assert!(!changed.entities);
                    }
                }
            }
        }
    }
}
