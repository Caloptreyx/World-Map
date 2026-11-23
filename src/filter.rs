//! MCA Selector's chunk filter queries (`FilterParser`), evaluated on one chunk position at a time.
//!
//! Syntax: filters `<type> <comparator> <value>` joined by `AND` / `OR`, grouped with `( … )`,
//! groups negated with `!( … )`. `AND` binds tighter than `OR` (a group is the OR of its AND runs,
//! as MCA Selector's `GroupFilter` evaluates it). Values are a bare token of `[A-Za-z0-9,+:-]` or
//! a `"quoted"` string with `\"` and `\\` escapes.
//!
//! Filter types (value syntax; comparators):
//! - `DataVersion` (int ≥ 0), `xPos`, `yPos`, `zPos` (int), `#Entities`, `#ProtoEntities`,
//!   `#TileEntities` (int ≥ 0): `=`, `!=`, `<`, `>`, `<=`, `>=`.
//! - `InhabitedTime`, `LastUpdate`: ticks, or a duration such as `"1 hour 30 minutes"` (years,
//!   months, days, hours, minutes, seconds; converted to ticks at 20 per second); number
//!   comparators.
//! - `Timestamp`: the region header timestamp, unix seconds or `"YYYY-MM-DD[ hh[:mm[:ss]]]"`
//!   (read as UTC; MCA Selector uses the computer's time zone); number comparators.
//! - `Compression`: `GZIP`, `ZLIB`, `UNCOMPRESSED`, `LZ4`, their `_EXT` forms (stored in a `.mcc`
//!   file) or the type byte; `=`, `!=`. Compares the region/ chunk only.
//! - `Palette`: block names (`stone,minecraft:dirt,'custom:block'`); `contains`, `!contains`,
//!   `intersects`, `!intersects`, `=` (exactly these blocks), `!=`.
//! - `Biome`: biome names or legacy numeric ids, `'quoted'` for custom ones; `contains`,
//!   `!contains`, `intersects`.
//! - `Entities`: entity ids; `Structures`: structure names (also old names such as `Village`);
//!   `contains`, `!contains`, `intersects`.
//! - `Status`: a chunk status (`full`, `minecraft:features`, `'custom'`); `=`, `!=`.
//! - `LightPopulated`: `0` or `1` (`LightPopulated`/`isLightOn`); `=`, `!=`.
//! - `Circle`: `"<x>;<z>;<radius>[,…]"` in chunks; `contains`, `!contains`.
//! - `Border`: 0 to 4: how many of the 8 neighbouring chunks are missing from region/ files, for
//!   chunks with status `full` (others count 9); number comparators.
//! - World Map addition `Size`: the compressed size of the region/ chunk in bytes; number
//!   comparators.
//!
//! `PlayerLocation`, `PlayerSpawn`, `Selection` (they read files from the user's computer) and
//! `Custom` / `Script` (Groovy scripts) are parse errors.
//!
//! Where World Map differs from MCA Selector: `to_query` always re-parses to the same filter
//! (Timestamp keeps its name, values are escaped, empty groups are kept as `()`, Status values and
//! `_EXT` compressions are quoted); a `)` without an open group is an error instead of ending the
//! query; Border reads `minecraft:full` as full; block palettes of a single entry (1.18+ sections
//! without `data`) count as that block for the overlays; pre-1.13 chunks (numeric block ids) never
//! match Palette filters.
use crate::{
    chunk,
    nbt::{Compound, Tag},
};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

/// 17w47a (1.13): block palettes.
pub(crate) const PALETTES: i64 = 1451;
/// 18w06a: chunk status strings.
const STATUS: i64 = 1466;
/// 19w02a: `isLightOn` replaced `LightPopulated`.
const LIGHT_ON: i64 = 1921;
/// 20w17a (1.16): block states stopped spanning two longs.
pub(crate) const NO_SPANNING: i64 = 2529;
/// 20w45a (1.17): entities moved to entities/ files.
pub(crate) const ENTITIES_FILE: i64 = 2681;
/// 21w37a: `Level.Sections` with `block_states` and `biomes` palettes.
pub(crate) const SECTION_PALETTES: i64 = 2834;
/// 21w43a (1.18): no `Level` compound, `sections`, `block_entities`, `structures`.
pub(crate) const MODERN: i64 = 2844;

/// Everything known about one chunk position while a region is processed.
#[derive(Clone, Copy)]
pub struct ChunkContext<'a> {
    /// Absolute chunk coordinates.
    pub x: i32,
    pub z: i32,
    /// region/ chunk NBT (None: no chunk or not needed).
    pub region: Option<&'a Compound>,
    pub entities: Option<&'a Compound>,
    pub poi: Option<&'a Compound>,
    /// region/ header timestamp (0 when absent).
    pub timestamp: u32,
    /// region/ chunk compression type, 0x80 flag stripped.
    pub compression: Option<u8>,
    /// Stored in a .mcc.
    pub external: bool,
    /// Compressed bytes of the region/ chunk.
    pub size: Option<usize>,
    /// Whether a region/ chunk exists at absolute chunk coordinates (for `Border`, see
    /// [`Filter::needs_neighbours`]); `None`: unknown, Border never matches.
    pub neighbours: Option<&'a dyn Fn(i32, i32) -> bool>,
}

/// Which files a filter / overlay / change needs, so unneeded files are never read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    pub region: bool,
    pub entities: bool,
    pub poi: bool,
}

impl Needs {
    fn union(self, other: Self) -> Self {
        Self {
            region: self.region || other.region,
            entities: self.entities || other.entities,
            poi: self.poi || other.poi,
        }
    }
}

/// A query that does not parse: what is wrong and where (UTF-16 offset into the query, as
/// MCA Selector and the browser count).
#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    And,
    Or,
}

impl Operator {
    fn query(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comparator {
    Equal,
    NotEqual,
    Smaller,
    Larger,
    LargerEqual,
    SmallerEqual,
    Contains,
    ContainsNot,
    Intersects,
    IntersectsNot,
}

impl Comparator {
    const ALL: [Self; 10] = [
        Self::Equal,
        Self::NotEqual,
        Self::Smaller,
        Self::Larger,
        Self::LargerEqual,
        Self::SmallerEqual,
        Self::Contains,
        Self::ContainsNot,
        Self::Intersects,
        Self::IntersectsNot,
    ];

    fn query(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Smaller => "<",
            Self::Larger => ">",
            Self::LargerEqual => ">=",
            Self::SmallerEqual => "<=",
            Self::Contains => "contains",
            Self::ContainsNot => "!contains",
            Self::Intersects => "intersects",
            Self::IntersectsNot => "!intersects",
        }
    }

    fn from_query(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.query() == token)
    }

    /// NumberFilter's comparison; a missing chunk value never matches (IntFilter's null checks).
    fn numbers(self, data: Option<i64>, value: i64) -> bool {
        let Some(data) = data else {
            return false;
        };
        match self {
            Self::Equal => data == value,
            Self::NotEqual => data != value,
            Self::Larger => data > value,
            Self::Smaller => data < value,
            Self::LargerEqual => data >= value,
            Self::SmallerEqual => data <= value,
            _ => false,
        }
    }
}

const NUMBER: &[Comparator] = &[
    Comparator::Equal,
    Comparator::NotEqual,
    Comparator::Larger,
    Comparator::Smaller,
    Comparator::LargerEqual,
    Comparator::SmallerEqual,
];
const EQUALITY: &[Comparator] = &[Comparator::Equal, Comparator::NotEqual];
/// TextFilter's comparators (MCA Selector offers no `!intersects` there).
const TEXT: &[Comparator] = &[
    Comparator::Contains,
    Comparator::ContainsNot,
    Comparator::Intersects,
];
const PALETTE: &[Comparator] = &[
    Comparator::Contains,
    Comparator::ContainsNot,
    Comparator::Intersects,
    Comparator::IntersectsNot,
    Comparator::Equal,
    Comparator::NotEqual,
];
const CIRCLE: &[Comparator] = &[Comparator::Contains, Comparator::ContainsNot];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    DataVersion,
    InhabitedTime,
    XPos,
    YPos,
    ZPos,
    Timestamp,
    Compression,
    LastUpdate,
    Palette,
    Biome,
    Status,
    LightPopulated,
    Entities,
    Structures,
    EntityAmount,
    ProtoEntityAmount,
    TileEntityAmount,
    Circle,
    Border,
    Size,
}

impl Type {
    const ALL: [Self; 20] = [
        Self::DataVersion,
        Self::InhabitedTime,
        Self::XPos,
        Self::YPos,
        Self::ZPos,
        Self::Timestamp,
        Self::Compression,
        Self::LastUpdate,
        Self::Palette,
        Self::Biome,
        Self::Status,
        Self::LightPopulated,
        Self::Entities,
        Self::Structures,
        Self::EntityAmount,
        Self::ProtoEntityAmount,
        Self::TileEntityAmount,
        Self::Circle,
        Self::Border,
        Self::Size,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::DataVersion => "DataVersion",
            Self::InhabitedTime => "InhabitedTime",
            Self::XPos => "xPos",
            Self::YPos => "yPos",
            Self::ZPos => "zPos",
            Self::Timestamp => "Timestamp",
            Self::Compression => "Compression",
            Self::LastUpdate => "LastUpdate",
            Self::Palette => "Palette",
            Self::Biome => "Biome",
            Self::Status => "Status",
            Self::LightPopulated => "LightPopulated",
            Self::Entities => "Entities",
            Self::Structures => "Structures",
            Self::EntityAmount => "#Entities",
            Self::ProtoEntityAmount => "#ProtoEntities",
            Self::TileEntityAmount => "#TileEntities",
            Self::Circle => "Circle",
            Self::Border => "Border",
            Self::Size => "Size",
        }
    }

    fn comparators(self) -> &'static [Comparator] {
        match self {
            Self::Compression | Self::Status | Self::LightPopulated => EQUALITY,
            Self::Palette => PALETTE,
            Self::Biome | Self::Entities | Self::Structures => TEXT,
            Self::Circle => CIRCLE,
            _ => NUMBER,
        }
    }

    fn needs(self) -> Needs {
        match self {
            Self::Circle => Needs::default(),
            // entities live in region/ chunks before 1.17; the version comes from the region chunk
            Self::Entities | Self::EntityAmount => Needs {
                region: true,
                entities: true,
                poi: false,
            },
            _ => Needs {
                region: true,
                entities: false,
                poi: false,
            },
        }
    }
}

/// Why a MCA Selector filter type is not offered.
fn unavailable(name: &str) -> Option<&'static str> {
    Some(match name {
        "PlayerLocation" => {
            "PlayerLocation is not available in World Map: it reads player files chosen on your computer"
        }
        "PlayerSpawn" => {
            "PlayerSpawn is not available in World Map: it reads player files chosen on your computer"
        }
        "Selection" => {
            "Selection is not available in World Map: it reads a selection file chosen on your computer; use the map selection instead"
        }
        "Custom" => "Custom is not available in World Map: it runs a Groovy script",
        "Script" => "Script is not available in World Map: it runs a Groovy script",
        _ => return None,
    })
}

/// MCA Selector's `CompressionType` names and bytes.
const COMPRESSIONS: [(&str, i8); 10] = [
    ("NONE", 0),
    ("GZIP", 1),
    ("ZLIB", 2),
    ("UNCOMPRESSED", 3),
    ("LZ4", 4),
    ("NONE_EXT", -128),
    ("GZIP_EXT", -127),
    ("ZLIB_EXT", -126),
    ("UNCOMPRESSED_EXT", -125),
    ("LZ4_EXT", -124),
];

/// A biome of a `Biome` filter: by name (new chunks) and numeric id (chunks before 1.18).
#[derive(Clone, Debug, PartialEq)]
struct Biome {
    name: Option<String>,
    id: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Known { name: String, namespaced: String },
    Custom(String),
}

impl Status {
    /// `StatusIdentifier.equals(String)`.
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Custom(name) => name == value,
            Self::Known { name, namespaced } => {
                (value.starts_with("minecraft:") && value == namespaced) || value == name
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Circle {
    x: i32,
    z: i32,
    radius: i32,
}

impl Circle {
    /// `CircleFilterDefinition.matches`: within the radius plus 0.3 chunks.
    fn matches(&self, x: i32, z: i32) -> bool {
        if x == self.x && z == self.z {
            return true;
        }
        let dx = f64::from(x) - f64::from(self.x);
        let dz = f64::from(z) - f64::from(self.z);
        let reach = f64::from(self.radius) + 0.3;
        dx * dx + dz * dz <= reach * reach
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Number(i64),
    Compression(i8),
    Blocks(Vec<String>),
    Biomes(Vec<Biome>),
    Status(Status),
    Names(Vec<String>),
    Circles(Vec<Circle>),
}

#[derive(Clone, Debug, PartialEq)]
struct Leaf {
    operator: Operator,
    kind: Type,
    comparator: Comparator,
    /// The value as typed (quotes removed, escapes resolved).
    raw: String,
    value: Value,
}

#[derive(Clone, Debug, PartialEq)]
struct Group {
    operator: Operator,
    negated: bool,
    children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Group(Group),
    Leaf(Leaf),
}

impl Node {
    fn operator(&self) -> Operator {
        match self {
            Self::Group(group) => group.operator,
            Self::Leaf(leaf) => leaf.operator,
        }
    }
}

/// A parsed query: the root group of MCA Selector's filter tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    root: Group,
}

/// Most groups nested in each other, so a hostile query cannot exhaust the stack.
const MAX_DEPTH: usize = 100;

/// Parses a query in MCA Selector's `FilterParser` syntax. An empty query matches every chunk.
pub fn parse(query: &str) -> Result<Filter, ParseError> {
    let mut pointer = Pointer {
        chars: query.chars().collect(),
        index: 0,
    };
    Ok(Filter {
        root: pointer.group(0)?,
    })
}

impl Filter {
    /// The query in canonical form; it parses back to the same filter.
    pub fn to_query(&self) -> String {
        group_query(&self.root, 0)
    }

    pub fn needs(&self) -> Needs {
        fn walk(group: &Group) -> Needs {
            group
                .children
                .iter()
                .fold(Needs::default(), |needs, child| {
                    needs.union(match child {
                        Node::Group(group) => walk(group),
                        Node::Leaf(leaf) => leaf.kind.needs(),
                    })
                })
        }
        walk(&self.root)
    }

    /// Whether a `Border` filter needs [`ChunkContext::neighbours`].
    pub fn needs_neighbours(&self) -> bool {
        fn walk(group: &Group) -> bool {
            group.children.iter().any(|child| match child {
                Node::Group(group) => walk(group),
                Node::Leaf(leaf) => leaf.kind == Type::Border,
            })
        }
        walk(&self.root)
    }

    /// false when no chunk of region (rx, rz) can match (xPos/zPos/Circle bounds), so the region
    /// is skipped without reading it (MCA Selector's RegionMatcher); true when unsure.
    pub fn region_may_match(&self, rx: i32, rz: i32) -> bool {
        group_region(&self.root, rx, rz) != Match::None
    }

    pub fn matches(&self, chunk: &ChunkContext) -> bool {
        group_matches(&self.root, chunk)
    }
}

fn escape(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn group_query(group: &Group, depth: usize) -> String {
    let show = group.negated || depth != 0;
    let mut query = String::new();
    if show {
        query.push_str(if group.negated { "!(" } else { "(" });
    }
    for (i, child) in group.children.iter().enumerate() {
        if i != 0 {
            query.push(' ');
            query.push_str(child.operator().query());
            query.push(' ');
        }
        match child {
            Node::Group(child) => query.push_str(&group_query(child, depth + 1)),
            Node::Leaf(leaf) => query.push_str(&leaf_query(leaf)),
        }
    }
    if show {
        query.push(')');
    }
    query
}

fn leaf_query(leaf: &Leaf) -> String {
    let value = match (&leaf.value, leaf.kind) {
        (Value::Number(number), kind)
            if !matches!(
                kind,
                Type::InhabitedTime | Type::LastUpdate | Type::Timestamp
            ) =>
        {
            number.to_string()
        }
        (Value::Compression(byte), _) => {
            let name = COMPRESSIONS
                .iter()
                .find(|&&(_, b)| b == *byte)
                .map_or("NONE", |&(name, _)| name);
            if name.contains('_') {
                escape(name)
            } else {
                name.to_string()
            }
        }
        (Value::Status(Status::Known { namespaced, .. }), _) => escape(namespaced),
        _ => escape(&leaf.raw),
    };
    format!("{} {} {}", leaf.kind.name(), leaf.comparator.query(), value)
}

/// RegionMatcher's answer for a whole region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Match {
    Full,
    Partial,
    None,
}

impl Match {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, _) | (_, Self::None) => Self::None,
            (Self::Full, Self::Full) => Self::Full,
            _ => Self::Partial,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::None, Self::None) => Self::None,
            _ => Self::Partial,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::Full => Self::None,
            Self::None => Self::Full,
            Self::Partial => Self::Partial,
        }
    }
}

/// A group as GroupFilter evaluates it: the OR of its runs of AND-joined children.
fn group_region(group: &Group, rx: i32, rz: i32) -> Match {
    let mut any = Match::None;
    let mut run = Match::Full;
    for (i, child) in group.children.iter().enumerate() {
        if i != 0 && child.operator() == Operator::Or {
            any = any.or(run);
            run = Match::Full;
        }
        let child = match child {
            Node::Group(child) => group_region(child, rx, rz),
            Node::Leaf(leaf) => leaf_region(leaf, rx, rz),
        };
        run = run.and(child);
    }
    let result = any.or(run);
    if group.negated { result.not() } else { result }
}

fn count_match(matched: usize) -> Match {
    match matched {
        0 => Match::None,
        32 => Match::Full,
        _ => Match::Partial,
    }
}

fn leaf_region(leaf: &Leaf, rx: i32, rz: i32) -> Match {
    match (leaf.kind, &leaf.value) {
        (Type::XPos | Type::ZPos, Value::Number(value)) => {
            let start = i64::from(if leaf.kind == Type::XPos { rx } else { rz }) * 32;
            count_match(
                (0..32)
                    .filter(|i| leaf.comparator.numbers(Some(start + i), *value))
                    .count(),
            )
        }
        (Type::Circle, Value::Circles(circles)) => {
            let (x0, z0) = (rx.saturating_mul(32), rz.saturating_mul(32));
            let (x1, z1) = (x0.saturating_add(31), z0.saturating_add(31));
            let corners = [(x0, z0), (x0, z1), (x1, z0), (x1, z1)];
            let full = circles
                .iter()
                .any(|c| corners.iter().all(|&(x, z)| c.matches(x, z)));
            let touched = full
                || circles
                    .iter()
                    .any(|c| c.matches(c.x.clamp(x0, x1), c.z.clamp(z0, z1)));
            let inside = if full {
                Match::Full
            } else if touched {
                Match::Partial
            } else {
                Match::None
            };
            if leaf.comparator == Comparator::ContainsNot {
                inside.not()
            } else {
                inside
            }
        }
        _ => Match::Partial,
    }
}

fn group_matches(group: &Group, chunk: &ChunkContext) -> bool {
    let mut current = true;
    for (i, child) in group.children.iter().enumerate() {
        let operator = child.operator();
        if (operator == Operator::And || i == 0) && current {
            current = node_matches(child, chunk);
        } else if operator == Operator::Or {
            if current {
                return !group.negated;
            }
            current = node_matches(child, chunk);
        }
    }
    group.negated != current
}

fn node_matches(node: &Node, chunk: &ChunkContext) -> bool {
    match node {
        Node::Group(group) => group_matches(group, chunk),
        Node::Leaf(leaf) => leaf_matches(leaf, chunk),
    }
}

fn leaf_matches(leaf: &Leaf, chunk: &ChunkContext) -> bool {
    let version = version(chunk);
    let comparator = leaf.comparator;
    match &leaf.value {
        Value::Number(value) => {
            let data = match leaf.kind {
                Type::DataVersion => chunk.region.and_then(data_version_tag),
                Type::InhabitedTime => Some(
                    level(chunk, version)
                        .and_then(|level| level.i64("InhabitedTime"))
                        .unwrap_or(0),
                ),
                Type::LastUpdate => Some(
                    level(chunk, version)
                        .and_then(|level| level.i64("LastUpdate"))
                        .unwrap_or(0),
                ),
                Type::XPos => level(chunk, version).and_then(|level| level.i64("xPos")),
                Type::ZPos => level(chunk, version).and_then(|level| level.i64("zPos")),
                Type::YPos if version >= MODERN => chunk.region.and_then(|root| root.i64("yPos")),
                Type::YPos => None,
                Type::Timestamp => Some(i64::from(chunk.timestamp)),
                Type::LightPopulated => Some(light_populated(chunk, version).unwrap_or(0)),
                Type::EntityAmount => Some(entities(chunk, version).map_or(0, <[Tag]>::len) as i64),
                Type::ProtoEntityAmount => Some(
                    chunk
                        .region
                        .and_then(|root| root.list("entities"))
                        .map_or(0, <[Tag]>::len) as i64,
                ),
                Type::TileEntityAmount => {
                    Some(tile_entities(chunk, version).map_or(0, <[Tag]>::len) as i64)
                }
                Type::Border => border(chunk, version),
                Type::Size => chunk.size.map(|size| size as i64),
                _ => None,
            };
            comparator.numbers(data, *value)
        }
        Value::Compression(byte) => {
            let equal = chunk.compression.is_some_and(|compression| {
                (compression | if chunk.external { 0x80 } else { 0 }) as i8 == *byte
            });
            match comparator {
                Comparator::Equal => equal,
                Comparator::NotEqual => !equal,
                _ => false,
            }
        }
        Value::Status(status) => {
            let equal = status_tag(chunk, version).is_some_and(|value| status.matches(value));
            match comparator {
                Comparator::Equal => equal,
                Comparator::NotEqual => !equal,
                _ => false,
            }
        }
        Value::Blocks(blocks) => match comparator {
            Comparator::Contains => palette_contains(chunk, version, blocks, true),
            Comparator::ContainsNot => !palette_contains(chunk, version, blocks, true),
            Comparator::Intersects => palette_contains(chunk, version, blocks, false),
            Comparator::IntersectsNot => !palette_contains(chunk, version, blocks, false),
            Comparator::Equal => palette_equals(chunk, version, blocks),
            Comparator::NotEqual => !palette_equals(chunk, version, blocks),
            _ => false,
        },
        Value::Biomes(biomes) => text(comparator, |all| {
            biomes_contain(chunk, version, biomes, all)
        }),
        Value::Names(names) if leaf.kind == Type::Entities => text(comparator, |all| {
            entities_contain(chunk, version, names, all)
        }),
        Value::Names(names) => text(comparator, |all| {
            structures_contain(chunk, version, names, all)
        }),
        Value::Circles(circles) => {
            let inside = circles.iter().any(|c| c.matches(chunk.x, chunk.z));
            match comparator {
                Comparator::Contains => inside,
                Comparator::ContainsNot => !inside,
                _ => false,
            }
        }
    }
}

/// TextFilter's comparators on a `contains(all)` check (`all` false: intersects).
fn text(comparator: Comparator, contains: impl Fn(bool) -> bool) -> bool {
    match comparator {
        Comparator::Contains => contains(true),
        Comparator::ContainsNot => !contains(true),
        Comparator::Intersects => contains(false),
        Comparator::IntersectsNot => !contains(false),
        _ => false,
    }
}

/// The DataVersion of a chunk root (`Level.DataVersion` when the root has none; 100, MCA
/// Selector's oldest version, without any).
pub(crate) fn data_version(root: &Compound) -> i64 {
    if root.get("DataVersion").is_none()
        && let Some(level) = root.compound("Level")
    {
        return level.i64("DataVersion").unwrap_or(100);
    }
    root.i64("DataVersion").unwrap_or(100)
}

/// The DataVersion tag the DataVersion filter compares: an int tag only.
fn data_version_tag(root: &Compound) -> Option<i64> {
    let tag = if root.get("DataVersion").is_none() {
        root.compound("Level")
            .and_then(|level| level.get("DataVersion"))
    } else {
        root.get("DataVersion")
    };
    match tag {
        Some(Tag::Int(value)) => Some(i64::from(*value)),
        _ => None,
    }
}

/// The version MCA Selector picks its reading code by: the region/ chunk's, else the entities/
/// chunk's, else the poi/ chunk's; 0 without any chunk.
pub(crate) fn version(chunk: &ChunkContext) -> i64 {
    chunk
        .region
        .or(chunk.entities)
        .or(chunk.poi)
        .map_or(0, data_version)
}

/// Where most chunk values live: `Level` before 1.18, the root since.
pub(crate) fn level<'a>(chunk: &ChunkContext<'a>, version: i64) -> Option<&'a Compound> {
    let root = chunk.region?;
    if version < MODERN {
        root.compound("Level")
    } else {
        Some(root)
    }
}

/// The entities of a chunk: region/ `Level.Entities` before 1.17, the entities/ chunk since.
pub(crate) fn entities<'a>(chunk: &ChunkContext<'a>, version: i64) -> Option<&'a [Tag]> {
    if version >= ENTITIES_FILE {
        chunk.entities?.list("Entities")
    } else {
        chunk.region?.compound("Level")?.list("Entities")
    }
}

/// The block entities: `Level.TileEntities` before 1.18, `block_entities` since.
pub(crate) fn tile_entities<'a>(chunk: &ChunkContext<'a>, version: i64) -> Option<&'a [Tag]> {
    let level = level(chunk, version)?;
    level.list(if version < MODERN {
        "TileEntities"
    } else {
        "block_entities"
    })
}

fn status_tag<'a>(chunk: &ChunkContext<'a>, version: i64) -> Option<&'a str> {
    if version < STATUS {
        return None;
    }
    level(chunk, version)?.str("Status")
}

fn light_populated(chunk: &ChunkContext, version: i64) -> Option<i64> {
    let level = level(chunk, version)?;
    if version < STATUS {
        level.i64("LightPopulated")
    } else if version < LIGHT_ON {
        None
    } else {
        level.i64("isLightOn")
    }
}

/// The 8 neighbours `Border` checks.
const NEIGHBOURS: [(i32, i32); 8] = [
    (0, -1),
    (1, 0),
    (0, 1),
    (-1, 0),
    (-1, -1),
    (1, -1),
    (1, 1),
    (-1, 1),
];

/// BorderFilter's number: missing neighbours of a full chunk, 9 for chunks that are not full.
fn border(chunk: &ChunkContext, version: i64) -> Option<i64> {
    if chunk.region.is_none() {
        return Some(9);
    }
    let status = status_tag(chunk, version).map(|s| s.strip_prefix("minecraft:").unwrap_or(s));
    if status != Some("full") {
        return Some(9);
    }
    let exists = chunk.neighbours?;
    Some(
        NEIGHBOURS
            .iter()
            .filter(|&&(dx, dz)| !exists(chunk.x.wrapping_add(dx), chunk.z.wrapping_add(dz)))
            .count() as i64,
    )
}

/// The block sections of a chunk root: `Level.Sections` before 1.18, `sections` since.
pub(crate) fn sections(root: &Compound, version: i64) -> Option<&[Tag]> {
    if version >= MODERN {
        root.list("sections")
    } else {
        root.compound("Level")?.list("Sections")
    }
}

/// The block palette entries of a section (1.13+): `Palette` before 21w37a, then
/// `block_states.palette`.
pub(crate) fn block_palette(section: &Compound, version: i64) -> Option<&[Tag]> {
    if version >= SECTION_PALETTES {
        section.compound("block_states")?.list("palette")
    } else if version >= PALETTES {
        section.list("Palette")
    } else {
        None
    }
}

/// The block names in every section palette; `None` without sections or before 1.13.
fn palette_names<'a>(chunk: &ChunkContext<'a>, version: i64) -> Option<Vec<&'a str>> {
    if version < PALETTES {
        return None;
    }
    let sections = sections(chunk.region?, version)?;
    let mut names = Vec::new();
    for section in sections.iter().filter_map(Tag::as_compound) {
        if let Some(entries) = block_palette(section, version)
            && let Some(section_names) = chunk::palette_names(entries)
        {
            names.extend(section_names);
        }
    }
    Some(names)
}

/// `matchBlockNames` (`all`) / `matchAnyBlockName`.
fn palette_contains(chunk: &ChunkContext, version: i64, blocks: &[String], all: bool) -> bool {
    let Some(names) = palette_names(chunk, version) else {
        return false;
    };
    let found = |block: &String| names.iter().any(|name| *name == block.as_str());
    if all {
        blocks.iter().all(found)
    } else {
        blocks.iter().any(found)
    }
}

/// `paletteEquals`: the chunk's blocks are exactly the filter's.
fn palette_equals(chunk: &ChunkContext, version: i64, blocks: &[String]) -> bool {
    let Some(names) = palette_names(chunk, version) else {
        return false;
    };
    let mut present = HashSet::new();
    for name in names {
        if !blocks.iter().any(|block| block == name) {
            return false;
        }
        present.insert(name);
    }
    present.len() == blocks.len() && blocks.iter().all(|block| present.contains(block.as_str()))
}

/// `matchBiomes` (`all`) / `matchAnyBiome`.
fn biomes_contain(chunk: &ChunkContext, version: i64, biomes: &[Biome], all: bool) -> bool {
    let Some(level) = level(chunk, version) else {
        return false;
    };
    if version < SECTION_PALETTES {
        let ids: Vec<i32> = if version < STATUS {
            match level.get("Biomes") {
                Some(Tag::ByteArray(ids)) => ids.iter().map(|&id| i32::from(id)).collect(),
                _ => return false,
            }
        } else {
            match level.get("Biomes") {
                Some(Tag::IntArray(ids)) => ids.clone(),
                _ => return false,
            }
        };
        let found = |biome: &Biome| biome.id.is_some_and(|id| ids.contains(&id));
        return if all {
            biomes.iter().all(found)
        } else {
            biomes.iter().any(found)
        };
    }
    let Some(sections) = level.list(if version < MODERN {
        "Sections"
    } else {
        "sections"
    }) else {
        return false;
    };
    let names: Vec<&str> = sections
        .iter()
        .filter_map(Tag::as_compound)
        .filter_map(|section| section.compound("biomes")?.list("palette"))
        .flatten()
        .filter_map(Tag::as_str)
        .collect();
    let found = |biome: &Biome| {
        biome
            .name
            .as_deref()
            .is_some_and(|name| names.contains(&name))
    };
    if all {
        // distinct biome names found, one per filter biome, as MCA Selector counts them
        let mut matched = HashSet::new();
        for biome in biomes {
            if let Some(name) = biome.name.as_deref()
                && names.contains(&name)
            {
                matched.insert(name);
            }
        }
        matched.len() == biomes.len()
    } else {
        biomes.iter().any(found)
    }
}

fn entities_contain(chunk: &ChunkContext, version: i64, names: &[String], all: bool) -> bool {
    let Some(list) = entities(chunk, version) else {
        return false;
    };
    let ids: Vec<&str> = list
        .iter()
        .filter_map(Tag::as_compound)
        .map(|entity| entity.str("id").unwrap_or(""))
        .collect();
    let found = |name: &String| ids.contains(&name.as_str());
    if all {
        names.iter().all(found)
    } else {
        names.iter().any(found)
    }
}

fn structures_contain(chunk: &ChunkContext, version: i64, names: &[String], all: bool) -> bool {
    let Some(level) = level(chunk, version) else {
        return false;
    };
    let references = if version < MODERN {
        level.compound("Structures")
    } else {
        level.compound("structures")
    }
    .and_then(|structures| structures.compound("References"));
    let Some(references) = references else {
        return false;
    };
    let found = |name: &String| {
        REGISTRIES.structure_alts(name).iter().any(
            |alt| matches!(references.get(alt), Some(Tag::LongArray(starts)) if !starts.is_empty()),
        )
    };
    if all {
        names.iter().all(found)
    } else {
        names.iter().any(found)
    }
}

/// MCA Selector's name registries (src/data/mcaselector/, MIT), for validating filter values.
struct Registries {
    /// Block names with and without `minecraft:`, mapped to the namespaced name.
    blocks: HashMap<String, String>,
    biomes: HashSet<String>,
    biome_ids: HashMap<String, i32>,
    biome_names: HashMap<i32, String>,
    /// Entity ids with and without `minecraft:`.
    entities: HashSet<String>,
    /// Status names with and without `minecraft:`, mapped to (name, namespaced name).
    statuses: HashMap<String, (String, String)>,
    structures: HashSet<String>,
    structure_alts: HashMap<String, Vec<String>>,
}

static REGISTRIES: LazyLock<Registries> = LazyLock::new(Registries::load);

#[derive(serde::Deserialize)]
struct StructureData {
    name: String,
    #[serde(default)]
    alt: Option<Vec<String>>,
}

impl Registries {
    fn load() -> Self {
        fn names(json: &str) -> Vec<String> {
            serde_json::from_str(json).expect("embedded MCA Selector registry")
        }
        let mut blocks = HashMap::new();
        for name in names(include_str!("data/mcaselector/blocks.json")) {
            blocks.insert(format!("minecraft:{name}"), format!("minecraft:{name}"));
            blocks.insert(name.clone(), format!("minecraft:{name}"));
        }
        let biomes = names(include_str!("data/mcaselector/biomes.json"))
            .into_iter()
            .collect();
        // pairs in MCA Selector's file order: a later name wins an id, as its map iteration does
        let pairs: Vec<(String, i32)> =
            serde_json::from_str(include_str!("data/mcaselector/biome_name_to_id.json"))
                .expect("embedded MCA Selector registry");
        let biome_ids = pairs.iter().cloned().collect();
        let biome_names = pairs.into_iter().map(|(name, id)| (id, name)).collect();
        let mut entities = HashSet::new();
        for name in names(include_str!("data/mcaselector/entities.json")) {
            entities.insert(format!("minecraft:{name}"));
            entities.insert(name);
        }
        let mut statuses = HashMap::new();
        for name in names(include_str!("data/mcaselector/status.json")) {
            let pair = (name.clone(), format!("minecraft:{name}"));
            statuses.insert(format!("minecraft:{name}"), pair.clone());
            statuses.insert(name, pair);
        }

        // StructureRegistry: every name and alt is valid, lower-case ones also with `minecraft:`;
        // a name's alts are its other spellings
        let data: Vec<StructureData> =
            serde_json::from_str(include_str!("data/mcaselector/structures.json"))
                .expect("embedded MCA Selector registry");
        let mut structures = HashSet::new();
        for entry in &data {
            for name in entry.alt.iter().flatten().chain([&entry.name]) {
                structures.insert(name.clone());
                if !name.chars().any(char::is_uppercase) {
                    structures.insert(format!("minecraft:{name}"));
                }
            }
        }
        let mut alts: HashMap<String, HashSet<String>> = HashMap::new();
        for name in &structures {
            alts.entry(name.clone()).or_default().insert(name.clone());
            if name.chars().any(char::is_uppercase) {
                let lower = name.to_lowercase();
                for other in [lower.clone(), format!("minecraft:{lower}")] {
                    if structures.contains(&other) {
                        alts.entry(name.clone()).or_default().insert(other.clone());
                        alts.entry(other).or_default().insert(name.clone());
                    }
                }
            } else if let Some(bare) = name.strip_prefix("minecraft:") {
                alts.entry(name.clone())
                    .or_default()
                    .insert(bare.to_string());
            } else {
                alts.entry(name.clone())
                    .or_default()
                    .insert(format!("minecraft:{name}"));
            }
        }
        let structure_alts = alts
            .into_iter()
            .map(|(name, set)| {
                let mut list: Vec<String> = set.into_iter().collect();
                list.sort();
                (name, list)
            })
            .collect();

        Self {
            blocks,
            biomes,
            biome_ids,
            biome_names,
            entities,
            statuses,
            structures,
            structure_alts,
        }
    }

    fn structure_alts(&self, name: &str) -> Vec<String> {
        self.structure_alts
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![name.to_string()])
    }

    /// `isValidName` of the block, entity and status registries: known, or `'quoted'`.
    fn quoted(name: &str) -> bool {
        name.starts_with('\'') && name.ends_with('\'')
    }
}

/// Java's `String.split` on a character: trailing empty parts dropped.
fn java_split(text: &str, separator: char) -> Vec<&str> {
    let mut parts: Vec<&str> = text.split(separator).collect();
    while parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.len() == 1 && parts[0].is_empty() && !text.is_empty() {
        parts.clear();
    }
    parts
}

/// Java's `String.trim`: strips characters up to U+0020.
fn java_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c <= ' ')
}

/// `Integer.parseInt` / `Long.parseLong` of a filter number (spaces removed first, as
/// MCA Selector's number filters do).
fn parse_number<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.replace(' ', "").parse().ok()
}

/// MCA Selector's `TextHelper.parseDuration`: seconds of a text such as `1 hour 30 minutes`.
/// Each unit counts once (its first occurrence); anything else left over is an error.
fn parse_duration(text: &str) -> Option<i64> {
    const UNITS: [(&[&str], i64); 6] = [
        (&["years", "year", "y"], 31_536_000),
        (&["months", "month"], 2_592_000),
        (&["days", "day", "d"], 86_400),
        (&["hours", "hour", "h"], 3_600),
        (&["minutes", "minute", "mins", "min"], 60),
        (&["seconds", "second", "secs", "sec", "s"], 1),
    ];
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut total: i64 = 0;
    let mut elements = Vec::new();
    for (alternatives, seconds) in UNITS {
        // `(?<data>\d+)\W*(?:alternatives)`, first match
        let found = (0..chars.len()).find_map(|start| {
            if !chars[start].is_ascii_digit() || (start > 0 && chars[start - 1].is_ascii_digit()) {
                return None;
            }
            let digits_end = (start..chars.len())
                .find(|&i| !chars[i].is_ascii_digit())
                .unwrap_or(chars.len());
            let unit_start = (digits_end..chars.len())
                .find(|&i| word(chars[i]))
                .unwrap_or(chars.len());
            let rest: String = chars[unit_start..].iter().collect();
            let unit = alternatives.iter().find(|alt| rest.starts_with(**alt))?;
            Some((start, digits_end, unit_start + unit.chars().count()))
        });
        if let Some((start, digits_end, end)) = found {
            let digits: String = chars[start..digits_end].iter().collect();
            let amount: i64 = digits.parse().ok()?;
            total = total.checked_add(amount.checked_mul(seconds)?)?;
            elements.push(chars[start..end].iter().collect::<String>());
        }
    }
    if elements.is_empty() {
        return None;
    }
    let mut remains = text.to_string();
    for element in elements {
        remains = remains.replacen(&element, "", 1);
    }
    java_trim(&remains).is_empty().then_some(total)
}

/// MCA Selector's `TextHelper.parseTimestamp` (`yyyy-MM-dd[ [HH][:mm][:ss]]`), read as UTC.
fn parse_timestamp(text: &str) -> Option<i64> {
    let chars: Vec<char> = java_trim(text).chars().collect();
    let mut i = 0;
    let digits = |i: &mut usize, min: usize, max: usize| -> Option<u32> {
        let start = *i;
        while *i < chars.len() && *i - start < max && chars[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i - start < min {
            *i = start;
            return None;
        }
        chars[start..*i].iter().collect::<String>().parse().ok()
    };
    let expect = |i: &mut usize, c: char| -> bool {
        if chars.get(*i) == Some(&c) {
            *i += 1;
            true
        } else {
            false
        }
    };
    // `yyyy` parses strictly: four digits (longer years would need a sign)
    let year = digits(&mut i, 4, 4)?;
    if !expect(&mut i, '-') {
        return None;
    }
    let month = digits(&mut i, 2, 2)?;
    if !expect(&mut i, '-') {
        return None;
    }
    let day = digits(&mut i, 2, 2)?;
    let (mut hour, mut minute, mut second) = (0, 0, 0);
    if expect(&mut i, ' ') {
        if let Some(h) = digits(&mut i, 2, 2) {
            hour = h;
        }
        let save = i;
        if expect(&mut i, ':') {
            match digits(&mut i, 2, 2) {
                Some(m) => minute = m,
                None => i = save,
            }
        }
        let save = i;
        if expect(&mut i, ':') {
            match digits(&mut i, 2, 2) {
                Some(s) => second = s,
                None => i = save,
            }
        }
    }
    if i != chars.len() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if minute > 59 || second > 59 || hour > 24 || (hour == 24 && (minute, second) != (0, 0)) {
        return None;
    }
    let year = i32::try_from(year).ok()?;
    // SMART resolving: days past the month's end become its last day
    let mut date = None;
    for d in (28..=day.max(28)).rev() {
        if d <= day
            && let Some(valid) = chrono::NaiveDate::from_ymd_opt(year, month, d)
        {
            date = Some(valid);
            break;
        }
    }
    let date = match date {
        Some(date) => date,
        None => chrono::NaiveDate::from_ymd_opt(year, month, day)?,
    };
    // 24:00:00 is the start of the next day
    let (date, hour) = if hour == 24 {
        (date.succ_opt()?, 0)
    } else {
        (date, hour)
    };
    Some(
        date.and_hms_opt(hour, minute, second)?
            .and_utc()
            .timestamp(),
    )
}

/// `BlockRegistry.parseBlockNames`: known names (namespaced) and `'quoted'` custom ones;
/// unknown names are skipped; `None` when nothing is left.
pub(crate) fn parse_block_names(raw: &str) -> Option<Vec<String>> {
    let mut blocks = Vec::new();
    for name in java_split(raw, ',') {
        let name = java_trim(name);
        if name.len() >= 2 && name.starts_with('\'') && name.ends_with('\'') {
            blocks.push(name[1..name.len() - 1].to_string());
        }
        if let Some(namespaced) = REGISTRIES.blocks.get(name) {
            blocks.push(namespaced.clone());
        }
    }
    (!blocks.is_empty()).then_some(blocks)
}

fn parse_biomes(raw: &str) -> Option<Vec<Biome>> {
    let joined = raw.replace(' ', "");
    let names = java_split(&joined, ',');
    if raw.is_empty() || names.is_empty() {
        return None;
    }
    let registries = &*REGISTRIES;
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let mut biomes = Vec::new();
    for name in names {
        let mut name = name.to_string();
        let mut quoted = false;
        if name.starts_with('\'') && name.ends_with('\'') && name.chars().count() > 2 {
            name = name[1..name.len() - 1].to_string();
            quoted = true;
        } else if !numeric(&name) && !name.starts_with("minecraft:") {
            name = format!("minecraft:{name}");
        }
        if numeric(&name) {
            let id: i32 = name.parse().ok()?;
            if !quoted && !registries.biome_names.contains_key(&id) {
                return None;
            }
            biomes.push(Biome {
                name: registries.biome_names.get(&id).cloned(),
                id: Some(id),
            });
        } else if quoted || registries.biomes.contains(&name) {
            biomes.push(Biome {
                id: registries.biome_ids.get(&name).copied(),
                name: Some(name),
            });
        } else {
            return None;
        }
    }
    Some(biomes)
}

fn parse_entities(raw: &str) -> Option<Vec<String>> {
    let joined = raw.replace(' ', "");
    let mut names: Vec<String> = java_split(&joined, ',')
        .into_iter()
        .map(str::to_string)
        .collect();
    if raw.is_empty() || names.is_empty() {
        return None;
    }
    let lower = |c: char| c.is_ascii_lowercase() || c == '_';
    for name in &mut names {
        // `^(?<space>[a-z_]*):?(?<id>[a-z_]*)$` with an empty id: a name without namespace
        let space_end = name.find(|c: char| !lower(c)).unwrap_or(name.len());
        let rest = &name[space_end..];
        let id = rest.strip_prefix(':').unwrap_or(rest);
        if id.chars().all(lower) && id.is_empty() {
            *name = format!("minecraft:{}", &name[..space_end]);
        }
        if name.len() >= 2 && name.starts_with('\'') && name.ends_with('\'') && !name.contains('"')
        {
            *name = name[1..name.len() - 1].to_string();
            continue;
        }
        if !REGISTRIES.entities.contains(name.as_str()) && !Registries::quoted(name) {
            return None;
        }
    }
    Some(names)
}

fn parse_structures(raw: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for name in java_split(raw, ',') {
        let name = java_trim(name);
        if name.len() >= 2 && name.starts_with('\'') && name.ends_with('\'') {
            names.push(name[1..name.len() - 1].to_string());
            continue;
        }
        if !REGISTRIES.structures.contains(name) {
            return None;
        }
        names.push(name.to_string());
    }
    (!names.is_empty()).then_some(names)
}

fn parse_status(raw: &str) -> Option<Status> {
    if let Some((name, namespaced)) = REGISTRIES.statuses.get(raw) {
        return Some(Status::Known {
            name: name.clone(),
            namespaced: namespaced.clone(),
        });
    }
    (raw.len() >= 2 && Registries::quoted(raw))
        .then(|| Status::Custom(raw[1..raw.len() - 1].to_string()))
}

fn parse_circles(raw: &str) -> Option<Vec<Circle>> {
    let mut circles = Vec::new();
    for circle in java_split(raw, ',') {
        let parts = java_split(circle, ';');
        let [x, z, radius] = parts.as_slice() else {
            return None;
        };
        let (x, z, radius) = (x.parse().ok()?, z.parse().ok()?, radius.parse().ok()?);
        if radius < 0 {
            return None;
        }
        circles.push(Circle { x, z, radius });
    }
    (!circles.is_empty()).then_some(circles)
}

/// The value of a filter type from its raw text, as the type's `setFilterValue` reads it.
fn parse_value(kind: Type, raw: &str) -> Option<Value> {
    let non_negative = |value: i64| (value >= 0).then_some(Value::Number(value));
    match kind {
        Type::DataVersion
        | Type::EntityAmount
        | Type::ProtoEntityAmount
        | Type::TileEntityAmount => {
            parse_number::<i32>(raw).and_then(|value| non_negative(value.into()))
        }
        Type::XPos | Type::YPos | Type::ZPos => {
            parse_number::<i32>(raw).map(|value| Value::Number(value.into()))
        }
        Type::Border => parse_number::<i32>(raw)
            .filter(|value| (0..=4).contains(value))
            .map(|value| Value::Number(value.into())),
        Type::LightPopulated => parse_number::<i8>(raw)
            .filter(|value| *value == 0 || *value == 1)
            .map(|value| Value::Number(value.into())),
        Type::Size => parse_number::<i64>(raw).and_then(non_negative),
        Type::InhabitedTime | Type::LastUpdate => parse_number::<i64>(raw)
            .or_else(|| parse_duration(raw)?.checked_mul(20))
            .map(Value::Number),
        Type::Timestamp => parse_number::<i32>(raw)
            .map(i64::from)
            .or_else(|| parse_timestamp(raw))
            .map(Value::Number),
        Type::Compression => COMPRESSIONS
            .iter()
            .find(|(name, byte)| name.eq_ignore_ascii_case(raw) || byte.to_string() == raw)
            .map(|&(_, byte)| Value::Compression(byte)),
        Type::Palette => parse_block_names(raw).map(Value::Blocks),
        Type::Biome => parse_biomes(raw).map(Value::Biomes),
        Type::Status => parse_status(raw).map(Value::Status),
        Type::Entities => parse_entities(raw).map(Value::Names),
        Type::Structures => parse_structures(raw).map(Value::Names),
        Type::Circle => parse_circles(raw).map(Value::Circles),
    }
}

/// Java's `Character.isWhitespace`.
fn java_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{1C}'..='\u{1F}'
    ) || (c.is_whitespace() && !matches!(c, '\u{85}' | '\u{A0}' | '\u{2007}' | '\u{202F}'))
}

/// Characters of an unquoted value.
fn value_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ',' | '-' | '+' | ':')
}

/// MCA Selector's `StringPointer` over a query.
struct Pointer {
    chars: Vec<char>,
    index: usize,
}

impl Pointer {
    fn current(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            position: self.chars[..self.index.min(self.chars.len())]
                .iter()
                .map(|c| c.len_utf16())
                .sum(),
        }
    }

    fn skip_whitespace(&mut self) {
        while self.current().is_some_and(java_whitespace) {
            self.index += 1;
        }
    }

    fn simple_string(&mut self) -> String {
        self.take_while(|c| !java_whitespace(c))
    }

    fn take_while(&mut self, valid: impl Fn(char) -> bool) -> String {
        let start = self.index;
        while self.current().is_some_and(&valid) {
            self.index += 1;
        }
        self.chars[start..self.index].iter().collect()
    }

    /// A `"quoted"` string with `\"` and `\\` escapes; the pointer is at the opening quote.
    fn quoted_string(&mut self) -> Result<String, ParseError> {
        self.index += 1;
        let mut value = String::new();
        let mut escape = false;
        while let Some(c) = self.current() {
            self.index += 1;
            if escape {
                if c != '\\' && c != '"' {
                    return Err(self.error(format!("invalid escape of '{c}'")));
                }
                escape = false;
            } else if c == '\\' {
                escape = true;
                continue;
            } else if c == '"' {
                return Ok(value);
            }
            value.push(c);
        }
        Err(self.error("missing end quote"))
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        self.skip_whitespace();
        match self.current() {
            Some(current) if current == c => {
                self.index += 1;
                Ok(())
            }
            Some(current) => Err(self.error(format!("expected '{c}' but got '{current}'"))),
            None => Err(self.error(format!("expected '{c}' but got EOF"))),
        }
    }

    /// `FilterParser.parse`: children until the end or a `)`; `depth` counts open groups.
    fn group(&mut self, depth: usize) -> Result<Group, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("groups nested too deeply"));
        }
        let mut group = Group {
            operator: Operator::And,
            negated: false,
            children: Vec::new(),
        };
        self.skip_whitespace();
        let mut first = true;
        while let Some(c) = self.current() {
            if c == ')' {
                if depth == 0 {
                    return Err(self.error("unexpected ')'"));
                }
                break;
            }
            let operator = if first {
                first = false;
                Operator::And
            } else {
                let token = self.simple_string();
                match token.as_str() {
                    "AND" => Operator::And,
                    "OR" => Operator::Or,
                    _ => return Err(self.error(format!("invalid operator {token}"))),
                }
            };
            self.skip_whitespace();
            let mut negated = false;
            if self.current() == Some('!') {
                negated = true;
                self.index += 1;
                self.skip_whitespace();
            }
            match self.current() {
                None => return Err(self.error("unexpected end of query")),
                Some('(') => {
                    self.index += 1;
                    let mut child = self.group(depth + 1)?;
                    child.operator = operator;
                    child.negated = negated;
                    group.children.push(Node::Group(child));
                    self.skip_whitespace();
                    self.expect(')')?;
                    self.skip_whitespace();
                    continue;
                }
                Some(_) if negated => return Err(self.error("only groups can be negated")),
                Some(_) => {}
            }
            group.children.push(Node::Leaf(self.filter(operator)?));
            self.skip_whitespace();
        }
        Ok(group)
    }

    /// `parseFilterType` + `parseFilterValue`.
    fn filter(&mut self, operator: Operator) -> Result<Leaf, ParseError> {
        let name = self.simple_string();
        let Some(kind) = Type::ALL.into_iter().find(|kind| kind.name() == name) else {
            return Err(self.error(match unavailable(&name) {
                Some(reason) => reason.to_string(),
                None => format!("invalid filter type \"{name}\""),
            }));
        };
        self.skip_whitespace();
        let token = self.simple_string();
        let comparator =
            Comparator::from_query(&token).ok_or_else(|| self.error("invalid comparator"))?;
        if !kind.comparators().contains(&comparator) {
            return Err(self.error(format!(
                "comparator {} not allowed for filter type {}",
                comparator.query(),
                kind.name()
            )));
        }
        self.skip_whitespace();
        let raw = if self.current() == Some('"') {
            self.quoted_string()?
        } else {
            self.take_while(value_char)
        };
        let value = parse_value(kind, &raw).ok_or_else(|| self.error("invalid value"))?;
        Ok(Leaf {
            operator,
            kind,
            comparator,
            raw,
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil;

    #[derive(serde::Deserialize)]
    struct Case {
        query: String,
        ok: bool,
        normalized: Option<String>,
        error_position: Option<usize>,
    }

    #[test]
    fn shared_query_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/queries.json")).unwrap();
        assert!(cases.len() >= 25);
        for case in cases {
            match parse(&case.query) {
                Ok(filter) => {
                    assert!(case.ok, "{:?} parsed but should fail", case.query);
                    let normalized = filter.to_query();
                    if let Some(expected) = &case.normalized {
                        assert_eq!(&normalized, expected, "{:?}", case.query);
                    }
                    // the canonical form parses back to itself
                    let again =
                        parse(&normalized).unwrap_or_else(|err| panic!("{normalized:?}: {err:?}"));
                    assert_eq!(again.to_query(), normalized);
                }
                Err(err) => {
                    assert!(!case.ok, "{:?} failed: {err:?}", case.query);
                    if let Some(position) = case.error_position {
                        assert_eq!(err.position, position, "{:?}: {err:?}", case.query);
                    }
                }
            }
        }
    }

    #[test]
    fn error_messages() {
        let message = |query: &str| parse(query).unwrap_err().message;
        assert_eq!(message("Foo = 1"), "invalid filter type \"Foo\"");
        assert_eq!(message("xPos == 1"), "invalid comparator");
        assert_eq!(
            message("xPos contains 1"),
            "comparator contains not allowed for filter type xPos"
        );
        assert_eq!(message("xPos = abc"), "invalid value");
        assert_eq!(message("xPos = 1 XOR zPos = 1"), "invalid operator XOR");
        assert_eq!(message("!xPos = 1"), "only groups can be negated");
        assert_eq!(message("(xPos = 1"), "expected ')' but got EOF");
        assert_eq!(
            message("Palette contains \"a\\b\""),
            "invalid escape of 'b'"
        );
        assert_eq!(message("Palette contains \"stone"), "missing end quote");
        assert!(message("Custom = \"return true\"").contains("Groovy"));
        assert!(message("PlayerLocation contains x").contains("not available in World Map"));
        assert!(message("Selection contains x").contains("not available in World Map"));
    }

    #[test]
    fn registry_values() {
        // names are validated against MCA Selector's registries
        assert!(parse("Palette contains \"notablock\"").is_err());
        assert!(parse("Palette contains \"stone,notablock\"").is_ok());
        assert!(parse("Biome contains notabiome").is_err());
        assert!(parse("Biome contains \"'custom:biome'\"").is_ok());
        assert!(parse("Biome contains 999").is_err());
        assert!(parse("Biome contains \"'999'\"").is_ok());
        assert!(parse("Entities contains notanentity").is_err());
        assert!(parse("Structures contains notastructure").is_err());
        assert!(parse("Structures contains Village").is_ok());
        assert!(parse("Status = notastatus").is_err());
        assert_eq!(
            REGISTRIES.structure_alts("Village"),
            vec!["Village", "minecraft:village", "village"]
        );
        assert_eq!(
            REGISTRIES.biome_names.get(&8).map(String::as_str),
            Some("minecraft:nether_wastes")
        );
        assert_eq!(REGISTRIES.biome_ids.get("minecraft:plains"), Some(&1));
    }

    #[test]
    fn durations_and_timestamps() {
        assert_eq!(parse_duration("5 minutes"), Some(300));
        assert_eq!(parse_duration("1 hour 30 minutes"), Some(5400));
        assert_eq!(parse_duration("2d 3h"), Some(2 * 86_400 + 3 * 3_600));
        assert_eq!(
            parse_duration("1 year 1 month"),
            Some(31_536_000 + 2_592_000)
        );
        // MCA Selector leaves separators other than spaces over: an error
        assert_eq!(parse_duration("1 year, 1 month"), None);
        assert_eq!(parse_duration("5 sec"), Some(5));
        assert_eq!(parse_duration("5 minutes and"), None);
        assert_eq!(parse_duration("minutes"), None);
        assert_eq!(parse_timestamp("2023-11-14"), Some(1_699_920_000));
        assert_eq!(parse_timestamp("2023-11-14 22:13:20"), Some(1_700_000_000));
        assert_eq!(parse_timestamp("2023-11-14 22"), Some(1_699_999_200));
        assert_eq!(parse_timestamp("2023-02-31"), parse_timestamp("2023-02-28"));
        assert_eq!(
            parse_timestamp("2023-11-14 24:00"),
            parse_timestamp("2023-11-15")
        );
        assert_eq!(parse_timestamp("2023-13-01"), None);
        assert_eq!(parse_timestamp("2023-1-01"), None);
        assert_eq!(parse_timestamp("yesterday"), None);
        let ticks = |query: &str| match &parse(query).unwrap().root.children[0] {
            Node::Leaf(leaf) => leaf.value.clone(),
            Node::Group(_) => unreachable!(),
        };
        assert_eq!(ticks("InhabitedTime > \"1 hour\""), Value::Number(72_000));
        assert_eq!(ticks("InhabitedTime > 100"), Value::Number(100));
        assert_eq!(ticks("LastUpdate < 5minutes"), Value::Number(6_000));
        assert_eq!(
            ticks("Timestamp > \"2023-11-14 22:13:20\""),
            Value::Number(1_700_000_000)
        );
    }

    /// A fixture region file's chunk at `index`: its NBT, header timestamp, compression and size.
    struct Stored {
        root: Compound,
        timestamp: u32,
        compression: u8,
        size: usize,
    }

    fn stored(data: &[u8], index: usize) -> Stored {
        let slot = anvil::slots(data)
            .unwrap()
            .into_iter()
            .find(|slot| slot.index == index)
            .expect("slot");
        let (compression, payload) = anvil::payload(data, slot).unwrap().unwrap();
        let start = anvil::SECTOR + index * 4;
        Stored {
            root: anvil::chunk(data, slot).unwrap().unwrap(),
            timestamp: u32::from_be_bytes(data[start..start + 4].try_into().unwrap()),
            compression,
            size: payload.len(),
        }
    }

    /// The chunk context of a stored chunk at region (rx, rz).
    fn context<'a>(
        rx: i32,
        rz: i32,
        index: usize,
        region: Option<&'a Stored>,
        entities: Option<&'a Compound>,
    ) -> ChunkContext<'a> {
        ChunkContext {
            x: rx * 32 + (index % 32) as i32,
            z: rz * 32 + (index / 32) as i32,
            region: region.map(|stored| &stored.root),
            entities,
            poi: None,
            timestamp: region.map_or(0, |stored| stored.timestamp),
            compression: region.map(|stored| stored.compression),
            external: false,
            size: region.map(|stored| stored.size),
            neighbours: None,
        }
    }

    const MODERN_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/filter_modern.mca");
    const ENTITIES_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/filter_entities.mca");
    const LEGACY_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/filter_legacy.mca");

    fn check(query: &str, chunk: &ChunkContext) -> bool {
        parse(query)
            .unwrap_or_else(|err| panic!("{query}: {err:?}"))
            .matches(chunk)
    }

    #[test]
    fn modern_chunks() {
        // r.-1.0.mca slots 89 (chest), 121 (spawner + chest, mineshaft, 1 minute inhabited), 18 (proto-chunk)
        let chest = stored(MODERN_FIXTURE, 89);
        let spawner = stored(MODERN_FIXTURE, 121);
        let proto = stored(MODERN_FIXTURE, 18);
        let entities = stored(ENTITIES_FIXTURE, 121);
        let chest = context(-1, 0, 89, Some(&chest), None);
        let spawner = context(-1, 0, 121, Some(&spawner), Some(&entities.root));
        let proto = context(-1, 0, 18, Some(&proto), None);

        assert!(check("DataVersion = 4189", &spawner));
        assert!(check(
            "DataVersion >= 3000 AND DataVersion < 5000",
            &spawner
        ));
        assert!(check("xPos = -7 AND zPos = 3 AND yPos = -4", &spawner));
        assert!(!check("xPos = -6", &spawner));
        assert!(check("InhabitedTime = 1200", &spawner));
        assert!(check("InhabitedTime = \"1 minute\"", &spawner));
        assert!(check("InhabitedTime < \"1 minute\"", &chest));
        assert!(check("LastUpdate = 99389128", &spawner));
        assert!(check("LastUpdate > \"1 day\"", &spawner));
        assert!(check("Status = full", &spawner));
        assert!(check("Status = \"minecraft:full\"", &spawner));
        assert!(check("Status = carvers AND Status != full", &proto));
        assert!(check("LightPopulated = 0", &spawner));
        assert!(check("Timestamp = 1710000000", &spawner));
        assert!(check("Timestamp > \"2024-01-01\"", &spawner));
        assert!(check("Timestamp < \"2021-01-01\"", &proto));
        assert!(check(
            "Compression = ZLIB AND Compression != GZIP",
            &spawner
        ));
        assert!(check("Compression = GZIP AND Compression = 1", &proto));
        assert!(!check("Compression = ZLIB_EXT", &spawner));
        assert!(check("Size = 5347", &spawner));
        assert!(check("Size > 3000 AND Size < 4000", &proto));
        assert!(check("#TileEntities = 2", &spawner));
        assert!(check("#TileEntities = 1", &chest));
        assert!(check("#TileEntities = 0", &proto));
        assert!(check("#ProtoEntities = 2", &proto));
        assert!(check("#ProtoEntities = 0", &spawner));
        assert!(check("Biome contains \"desert,river\"", &spawner));
        assert!(check("Biome contains desert", &proto));
        assert!(!check("Biome contains \"desert,river\"", &proto));
        assert!(check("Biome intersects \"river,plains\"", &spawner));
        assert!(check("Biome !contains plains", &spawner));
        assert!(check("Palette contains \"spawner,chest\"", &spawner));
        assert!(!check("Palette contains spawner", &chest));
        assert!(check("Palette intersects \"spawner,chest\"", &chest));
        assert!(check(
            "Palette !intersects \"spawner,diamond_block\"",
            &chest
        ));
        assert!(check(
            "Palette = \"bedrock,deepslate,deepslate_redstone_ore,lava,air,gravel,tuff,andesite,granite,copper_ore,stone,diorite,sandstone,sand\"",
            &proto
        ));
        assert!(check("Palette != \"bedrock,deepslate\"", &proto));
        assert!(check("Structures contains mineshaft", &spawner));
        assert!(check(
            "Structures contains \"minecraft:mineshaft\"",
            &spawner
        ));
        assert!(check("Structures contains Mineshaft", &spawner));
        // an empty reference list is no reference
        assert!(check("Structures !contains village_plains", &spawner));
        assert!(!check(
            "Structures intersects \"village_plains,igloo\"",
            &spawner
        ));
        assert!(!check("Structures contains mineshaft", &chest));
        // entities come from the entities/ chunk since 1.17
        assert!(check("#Entities = 3", &spawner));
        assert!(check("#Entities = 0", &chest));
        assert!(check("Entities contains \"cow,chicken\"", &spawner));
        assert!(check("Entities contains \"minecraft:cow\"", &spawner));
        assert!(!check("Entities contains \"cow,pig\"", &spawner));
        assert!(check("Entities intersects \"cow,pig\"", &spawner));
        assert!(!check("Entities intersects \"sheep,pig\"", &proto));
        assert!(check("Circle contains \"-7;3;0\"", &spawner));
        assert!(!check("Circle contains \"-7;3;0\"", &chest));
        assert!(check("Circle contains \"-7;3;1\"", &chest));
        assert!(check("Circle !contains \"0;0;5,100;100;5\"", &chest));
    }

    #[test]
    fn group_logic() {
        let stored = stored(MODERN_FIXTURE, 121);
        let chunk = context(-1, 0, 121, Some(&stored), None);
        // AND binds tighter than OR
        assert!(check("xPos = 0 AND zPos = 0 OR xPos = -7", &chunk));
        assert!(check("xPos = -7 OR xPos = 0 AND zPos = 0", &chunk));
        assert!(!check("xPos = 0 OR xPos = -7 AND zPos = 0", &chunk));
        assert!(check("(xPos = 0 OR xPos = -7) AND zPos = 3", &chunk));
        assert!(!check("(xPos = 0 OR xPos = -7) AND zPos = 0", &chunk));
        assert!(check("!(xPos = 0) AND !(zPos = 0 OR zPos = 1)", &chunk));
        assert!(!check("!(xPos = -7 AND zPos = 3)", &chunk));
        assert!(check("!(xPos = -7 AND zPos = 0)", &chunk));
        assert!(check("xPos = 0 OR !(!(zPos = 3))", &chunk));
        assert!(check("((xPos = -7 AND (zPos = 3 OR zPos = 4)))", &chunk));
        // empty groups and queries match everything (negated: nothing)
        assert!(check("", &chunk));
        assert!(check("()", &chunk));
        assert!(!check("!()", &chunk));
        assert!(check("xPos = 0 OR ()", &chunk));
    }

    #[test]
    fn legacy_chunks() {
        let v116 = stored(LEGACY_FIXTURE, 0);
        let v112 = stored(LEGACY_FIXTURE, 1);
        let v116 = context(0, 0, 0, Some(&v116), None);
        let v112 = context(0, 0, 1, Some(&v112), None);

        assert!(check("DataVersion = 2586", &v116));
        assert!(check("xPos = 0 AND zPos = 0", &v116));
        assert!(!check("yPos = 0 OR yPos != 0", &v116));
        assert!(check(
            "InhabitedTime = \"1 hour\" AND LastUpdate = 1000",
            &v116
        ));
        assert!(check("Status = full AND Status = minecraft:full", &v116));
        assert!(check("LightPopulated = 1", &v116));
        assert!(check(
            "Biome contains \"plains,desert\" AND Biome contains \"1,2\"",
            &v116
        ));
        assert!(!check("Biome intersects forest", &v116));
        assert!(check("Palette contains \"stone,grass_block,water\"", &v116));
        assert!(check(
            "Palette = \"air,stone,grass_block,water,sand\"",
            &v116
        ));
        assert!(!check("Palette = \"air,stone,grass_block,water\"", &v116));
        // before 1.17 entities are in the region/ chunk
        assert!(check(
            "#Entities = 2 AND Entities contains \"cow,pig\"",
            &v116
        ));
        assert!(check("#TileEntities = 1", &v116));
        assert!(check(
            "Structures contains Village AND Structures contains village",
            &v116
        ));
        assert!(check("Structures contains \"minecraft:village\"", &v116));
        assert!(check("Structures !contains Mineshaft", &v116));

        // 1.12: byte biomes (signed: id 129 reads as -127 and never matches), LightPopulated,
        // no status, no palette
        assert!(check("DataVersion = 1343", &v112));
        assert!(check("Biome contains plains", &v112));
        assert!(!check("Biome contains sunflower_plains", &v112));
        assert!(check("LightPopulated = 1", &v112));
        assert!(check("Status != full", &v112));
        assert!(!check("Status = full", &v112));
        assert!(!check("Palette intersects stone", &v112));
        assert!(check(
            "Entities contains zombie AND #Entities = 1 AND #TileEntities = 0",
            &v112
        ));
        assert!(check("LastUpdate = 500 AND InhabitedTime = 0", &v112));
    }

    #[test]
    fn missing_chunks() {
        // a position with only an entities/ chunk: region values read as MCA Selector's defaults
        let entities = stored(ENTITIES_FIXTURE, 121);
        let chunk = context(-1, 0, 121, None, Some(&entities.root));
        assert!(check("InhabitedTime = 0 AND #TileEntities = 0", &chunk));
        assert!(!check("DataVersion = 4189 OR DataVersion != 4189", &chunk));
        assert!(!check("xPos = -7 OR xPos != -7", &chunk));
        assert!(check("Entities contains cow AND #Entities = 3", &chunk));
        assert!(!check("Compression = ZLIB", &chunk));
        assert!(check("Compression != ZLIB", &chunk));
        assert!(!check("Size >= 0", &chunk));
        assert!(check("Timestamp = 0", &chunk));
    }

    #[test]
    fn border() {
        let spawner = stored(MODERN_FIXTURE, 121);
        let carvers = stored(MODERN_FIXTURE, 18);
        let filter = parse("Border > 0").unwrap();
        assert!(filter.needs_neighbours());
        assert!(!parse("xPos = 1").unwrap().needs_neighbours());
        let mut chunk = context(-1, 0, 121, Some(&spawner), None);
        // neighbours unknown: never matches
        assert!(!filter.matches(&chunk));
        // all neighbours present but the one to the north-west
        let exists = |x: i32, z: i32| (x, z) != (-8, 2);
        chunk.neighbours = Some(&exists);
        assert!(check("Border = 1", &chunk));
        assert!(!check("Border = 0", &chunk));
        let none = |_: i32, _: i32| false;
        chunk.neighbours = Some(&none);
        assert!(check("Border > 4", &chunk));
        // not full: counts 9
        let mut proto = context(-1, 0, 18, Some(&carvers), None);
        proto.neighbours = Some(&exists);
        assert!(check("Border > 4", &proto));
    }

    #[test]
    fn needs() {
        let needs = |query: &str| parse(query).unwrap().needs();
        assert_eq!(needs(""), Needs::default());
        assert_eq!(needs("Circle contains \"0;0;1\""), Needs::default());
        assert_eq!(
            needs("xPos = 1"),
            Needs {
                region: true,
                entities: false,
                poi: false
            }
        );
        assert_eq!(
            needs("Circle contains \"0;0;1\" OR (!(Entities contains cow))"),
            Needs {
                region: true,
                entities: true,
                poi: false
            }
        );
    }

    #[test]
    fn region_pruning() {
        let may = |query: &str, rx: i32, rz: i32| parse(query).unwrap().region_may_match(rx, rz);
        assert!(may("xPos = 5", 0, 0));
        assert!(!may("xPos = 5", 1, 0));
        assert!(!may("xPos = 32", 0, 7));
        assert!(may("xPos = 32", 1, 7));
        assert!(may("xPos >= 31", 0, 0));
        assert!(!may("xPos > 31", 0, 0));
        assert!(!may("xPos < -32", -1, 0));
        assert!(may("xPos < -31", -1, 0));
        assert!(!may("zPos = -1", 0, 0));
        assert!(may("zPos = -1", 5, -1));
        assert!(!may("xPos = 5 AND zPos = 40", 0, 0));
        assert!(may("xPos = 5 OR zPos = 40", 0, 0));
        assert!(may("xPos = 5 OR zPos = 40", 3, 1));
        assert!(!may("xPos = 5 OR zPos = 40", 3, 2));
        // unknown filters keep the region
        assert!(may("Status = full", 100, 100));
        assert!(may("Status = full AND xPos = 5", 0, 0));
        assert!(!may("Status = full AND xPos = 5", 1, 0));
        // negation: a region where every chunk matches the inner group is skipped
        assert!(!may("!(xPos >= 0)", 0, 0));
        assert!(may("!(xPos >= 0)", -1, 0));
        assert!(may("!(xPos >= 5)", 0, 0));
        assert!(!may("!(xPos >= 0 AND zPos >= 0)", 2, 2));
        assert!(may("!(xPos >= 0 AND zPos >= 0)", 2, -2));
        // circles: center region, edge overlap, far away
        assert!(may("Circle contains \"100;100;10\"", 3, 3));
        assert!(may("Circle contains \"100;100;10\"", 2, 3));
        assert!(!may("Circle contains \"100;100;10\"", 1, 3));
        assert!(!may("Circle contains \"100;100;10\"", -5, -5));
        assert!(!may("Circle !contains \"16;16;100\"", 0, 0));
        assert!(may("Circle !contains \"16;16;10\"", 0, 0));
        // an empty query keeps everything; an empty negated group nothing
        assert!(may("", 9, 9));
        assert!(!may("!()", 9, 9));
    }

    /// Every query MCA Selector 2.9 ran on tests/fixtures/mcaselector/world/ selects the same
    /// chunks here. MCA Selector evaluates all 1024 slots of every region file, missing chunks
    /// included (they read as InhabitedTime 0 and match Circle by position).
    #[test]
    fn mcaselector_parity() {
        use std::collections::{BTreeMap, HashMap};
        let world = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcaselector/world");
        let mut files: BTreeMap<(i32, i32), [Option<Vec<u8>>; 2]> = BTreeMap::new();
        for (kind, folder) in ["region", "entities"].into_iter().enumerate() {
            for entry in std::fs::read_dir(world.join(folder)).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                let parts: Vec<&str> = name.split('.').collect();
                let [_, x, z, _] = parts.as_slice() else {
                    panic!("{name}");
                };
                let key = (x.parse().unwrap(), z.parse().unwrap());
                files.entry(key).or_default()[kind] = Some(std::fs::read(entry.path()).unwrap());
            }
        }
        struct Position {
            rx: i32,
            rz: i32,
            index: usize,
            region: Option<Stored>,
            entities: Option<Compound>,
        }
        let mut positions = Vec::new();
        for (&(rx, rz), [region, entities]) in &files {
            let region_slots: HashMap<usize, anvil::Slot> = region
                .as_deref()
                .map(|data| anvil::slots(data).unwrap())
                .unwrap_or_default()
                .into_iter()
                .map(|slot| (slot.index, slot))
                .collect();
            let entity_slots: HashMap<usize, anvil::Slot> = entities
                .as_deref()
                .map(|data| anvil::slots(data).unwrap())
                .unwrap_or_default()
                .into_iter()
                .map(|slot| (slot.index, slot))
                .collect();
            for index in 0..anvil::CHUNKS {
                positions.push(Position {
                    rx,
                    rz,
                    index,
                    region: region_slots
                        .contains_key(&index)
                        .then(|| stored(region.as_deref().unwrap(), index)),
                    entities: entity_slots.get(&index).map(|&slot| {
                        anvil::chunk(entities.as_deref().unwrap(), slot)
                            .unwrap()
                            .unwrap()
                    }),
                });
            }
        }
        assert_eq!(positions.iter().filter(|p| p.region.is_some()).count(), 120);

        let expected: HashMap<String, Vec<(i32, i32)>> = serde_json::from_str(
            &std::fs::read_to_string(world.join("../queries-expected.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(expected.len(), 23);
        for (query, chunks) in expected {
            let filter = parse(&query).unwrap_or_else(|err| panic!("{query}: {err:?}"));
            let mut matched: Vec<(i32, i32)> = positions
                .iter()
                .filter_map(|p| {
                    let chunk =
                        context(p.rx, p.rz, p.index, p.region.as_ref(), p.entities.as_ref());
                    filter.matches(&chunk).then_some((chunk.x, chunk.z))
                })
                .collect();
            matched.sort();
            let mut chunks = chunks;
            chunks.sort();
            assert_eq!(matched.len(), chunks.len(), "{query}");
            assert_eq!(matched, chunks, "{query}");
        }
    }
}
