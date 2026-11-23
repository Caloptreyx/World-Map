//! Java Edition NBT: big-endian named tags with modified UTF-8 strings. The reader is adapted from
//! Caloptreyx/Player-Manager (MIT); the writer produces the bytes vanilla writes for the same tree.
use flate2::{
    Compression,
    read::{GzDecoder, ZlibDecoder},
    write::{GzEncoder, ZlibEncoder},
};
use std::{
    borrow::Cow,
    io::{Read, Write},
};

const END: u8 = 0;
const BYTE: u8 = 1;
const SHORT: u8 = 2;
const INT: u8 = 3;
const LONG: u8 = 4;
const FLOAT: u8 = 5;
const DOUBLE: u8 = 6;
const BYTE_ARRAY: u8 = 7;
const STRING: u8 = 8;
const LIST: u8 = 9;
const COMPOUND: u8 = 10;
const INT_ARRAY: u8 = 11;
const LONG_ARRAY: u8 = 12;

/// Nesting limit, the one Minecraft enforces.
const MAX_DEPTH: usize = 512;
/// Decompressed size limit: chunks and player files stay far below it.
pub const MAX_DECOMPRESSED: u64 = 64 << 20;

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

/// A compound tag: named tags in file order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Compound(Vec<(String, Tag)>);

impl Tag {
    /// Any integer tag.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Byte(value) => Some(value.into()),
            Self::Short(value) => Some(value.into()),
            Self::Int(value) => Some(value.into()),
            Self::Long(value) => Some(value),
            _ => None,
        }
    }

    /// Any number tag.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Self::Float(value) => Some(value.into()),
            Self::Double(value) => Some(value),
            _ => self.as_i64().map(|value| value as f64),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Self::Compound(compound) => Some(compound),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[i8]> {
        match self {
            Self::ByteArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_ints(&self) -> Option<&[i32]> {
        match self {
            Self::IntArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_longs(&self) -> Option<&[i64]> {
        match self {
            Self::LongArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_compound_mut(&mut self) -> Option<&mut Compound> {
        match self {
            Self::Compound(compound) => Some(compound),
            _ => None,
        }
    }

    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Tag>> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_ints_mut(&mut self) -> Option<&mut Vec<i32>> {
        match self {
            Self::IntArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_longs_mut(&mut self) -> Option<&mut Vec<i64>> {
        match self {
            Self::LongArray(values) => Some(values),
            _ => None,
        }
    }

    /// The NBT type id (1..=12).
    pub fn id(&self) -> u8 {
        match self {
            Self::Byte(_) => BYTE,
            Self::Short(_) => SHORT,
            Self::Int(_) => INT,
            Self::Long(_) => LONG,
            Self::Float(_) => FLOAT,
            Self::Double(_) => DOUBLE,
            Self::ByteArray(_) => BYTE_ARRAY,
            Self::String(_) => STRING,
            Self::List(_) => LIST,
            Self::Compound(_) => COMPOUND,
            Self::IntArray(_) => INT_ARRAY,
            Self::LongArray(_) => LONG_ARRAY,
        }
    }
}

impl Compound {
    /// An empty compound.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(name, _)| name == key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        self.0
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, tag)| tag)
    }

    pub fn compound_mut(&mut self, key: &str) -> Option<&mut Compound> {
        self.get_mut(key).and_then(Tag::as_compound_mut)
    }

    pub fn list_mut(&mut self, key: &str) -> Option<&mut Vec<Tag>> {
        self.get_mut(key).and_then(Tag::as_list_mut)
    }

    /// Replaces the value in place (keeping the key's position) or appends; the old value.
    pub fn insert(&mut self, key: impl Into<String>, tag: Tag) -> Option<Tag> {
        let key = key.into();
        if let Some(position) = self.0.iter().position(|(name, _)| *name == key) {
            return Some(std::mem::replace(&mut self.0[position].1, tag));
        }
        self.0.push((key, tag));
        None
    }

    /// Removes an entry, keeping the order of the others; the old value.
    pub fn remove(&mut self, key: &str) -> Option<Tag> {
        let position = self.0.iter().position(|(name, _)| name == key)?;
        Some(self.0.remove(position).1)
    }

    /// The entries in file order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(name, tag)| (name.as_str(), tag))
    }

    /// The entries in file order, values mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Tag)> {
        self.0.iter_mut().map(|(name, tag)| (name.as_str(), tag))
    }

    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, tag)| tag)
    }

    pub fn compound(&self, key: &str) -> Option<&Compound> {
        self.get(key).and_then(Tag::as_compound)
    }

    pub fn list(&self, key: &str) -> Option<&[Tag]> {
        self.get(key).and_then(Tag::as_list)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Tag::as_str)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Tag::as_i64)
    }

    /// A list of numbers, such as `Pos` or `Rotation`.
    pub fn numbers(&self, key: &str) -> Option<Vec<f64>> {
        self.list(key)?.iter().map(Tag::as_f64).collect()
    }
}

/// Collects entries in order; a repeated key replaces the earlier value in place, so keys stay
/// unique as NBT requires.
impl FromIterator<(String, Tag)> for Compound {
    fn from_iter<I: IntoIterator<Item = (String, Tag)>>(entries: I) -> Self {
        let mut compound = Self::new();
        for (key, tag) in entries {
            compound.insert(key, tag);
        }
        compound
    }
}

/// Serializes a root compound with an empty name (what chunks use); modified UTF-8 strings.
/// Lists are written with the type of their first element (END when empty, as vanilla does);
/// elements of another type cannot be stored in an NBT list and are left out. A string longer
/// than 65535 encoded bytes is cut at the last whole character that fits (Java refuses those).
pub fn write(root: &Compound) -> Vec<u8> {
    let mut output = vec![COMPOUND, 0, 0];
    write_compound(&mut output, root);
    output
}

/// Gzip, compression type 1 (and `.dat` files).
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 4), Compression::default());
    encoder
        .write_all(data)
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

/// A zlib stream, compression type 2 (what the game writes by default).
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(data.len() / 4), Compression::default());
    encoder
        .write_all(data)
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

fn write_compound(output: &mut Vec<u8>, compound: &Compound) {
    for (name, tag) in &compound.0 {
        output.push(tag.id());
        write_string(output, name);
        write_payload(output, tag);
    }
    output.push(END);
}

/// An array or list length (saturated: no array in memory comes near `i32::MAX` elements).
fn write_length(output: &mut Vec<u8>, length: usize) {
    output.extend(i32::try_from(length).unwrap_or(i32::MAX).to_be_bytes());
}

fn write_payload(output: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(value) => output.extend(value.to_be_bytes()),
        Tag::Short(value) => output.extend(value.to_be_bytes()),
        Tag::Int(value) => output.extend(value.to_be_bytes()),
        Tag::Long(value) => output.extend(value.to_be_bytes()),
        Tag::Float(value) => output.extend(value.to_be_bytes()),
        Tag::Double(value) => output.extend(value.to_be_bytes()),
        Tag::ByteArray(values) => {
            write_length(output, values.len());
            output.extend(values.iter().map(|&value| value as u8));
        }
        Tag::String(value) => write_string(output, value),
        Tag::List(items) => {
            let kind = items.first().map_or(END, Tag::id);
            output.push(kind);
            write_length(output, items.iter().filter(|item| item.id() == kind).count());
            for item in items.iter().filter(|item| item.id() == kind) {
                write_payload(output, item);
            }
        }
        Tag::Compound(compound) => write_compound(output, compound),
        Tag::IntArray(values) => {
            write_length(output, values.len());
            output.reserve(values.len() * 4);
            for value in values {
                output.extend(value.to_be_bytes());
            }
        }
        Tag::LongArray(values) => {
            write_length(output, values.len());
            output.reserve(values.len() * 8);
            for value in values {
                output.extend(value.to_be_bytes());
            }
        }
    }
}

/// A length-prefixed modified UTF-8 string, as Java's `DataOutput.writeUTF`: UTF-16 units, NUL
/// as `C0 80`, supplementary characters as two 3-byte surrogates.
fn write_string(output: &mut Vec<u8>, value: &str) {
    let at = output.len();
    output.extend([0, 0]);
    let start = output.len();
    let mut units = [0u16; 2];
    for character in value.chars() {
        let before = output.len();
        for &unit in character.encode_utf16(&mut units).iter() {
            match unit {
                0x0001..=0x007F => output.push(unit as u8),
                0x0000 | 0x0080..=0x07FF => {
                    output.extend([0xC0 | (unit >> 6) as u8, 0x80 | (unit & 0x3F) as u8]);
                }
                _ => output.extend([
                    0xE0 | (unit >> 12) as u8,
                    0x80 | ((unit >> 6) & 0x3F) as u8,
                    0x80 | (unit & 0x3F) as u8,
                ]),
            }
        }
        if output.len() - start > usize::from(u16::MAX) {
            output.truncate(before);
            break;
        }
    }
    let length = (output.len() - start) as u16;
    output[at..start].copy_from_slice(&length.to_be_bytes());
}

fn read_limited(reader: impl Read, what: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    reader
        .take(MAX_DECOMPRESSED + 1)
        .read_to_end(&mut output)
        .map_err(|err| format!("broken {what} data ({err})"))?;
    if output.len() as u64 > MAX_DECOMPRESSED {
        return Err(format!(
            "more than {} MiB uncompressed",
            MAX_DECOMPRESSED >> 20
        ));
    }
    Ok(output)
}

/// The uncompressed bytes of an NBT file that may be gzipped (by its magic bytes). Wings serves
/// gzip files decompressed, so both arrive.
pub fn decompress(data: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    if !data.starts_with(&[0x1F, 0x8B]) {
        return Ok(Cow::Borrowed(data));
    }
    read_limited(GzDecoder::new(data), "gzip").map(Cow::Owned)
}

pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    read_limited(GzDecoder::new(data), "gzip")
}

pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    read_limited(ZlibDecoder::new(data), "zlib")
}

/// The compound of a file's root tag (bytes after it are ignored, as Minecraft does).
pub fn read(data: &[u8]) -> Result<Compound, String> {
    let mut reader = Reader { data, position: 0 };
    match reader.u8()? {
        COMPOUND => {}
        kind => return Err(format!("the root tag is not a compound (type {kind})")),
    }
    reader.string()?;
    reader.compound(0)
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

/// The smallest payload of a tag type, to refuse lengths the remaining bytes cannot hold
/// before allocating for them.
fn min_size(kind: u8) -> usize {
    match kind {
        SHORT | STRING => 2,
        INT | FLOAT | BYTE_ARRAY | INT_ARRAY | LONG_ARRAY => 4,
        LIST => 5,
        LONG | DOUBLE => 8,
        _ => 1,
    }
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(count)
            .filter(|&end| end <= self.data.len())
            .ok_or("unexpected end of data")?;
        let bytes = &self.data[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        Ok(self.take(N)?.try_into().expect("take returns N bytes"))
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    /// An array or list length, refused when the rest of the data cannot hold that many
    /// elements of `element_size` bytes.
    fn length(&mut self, element_size: usize) -> Result<usize, String> {
        let length = i32::from_be_bytes(self.array()?);
        let length = usize::try_from(length).map_err(|_| format!("negative length {length}"))?;
        if length > (self.data.len() - self.position) / element_size {
            return Err(format!("length {length} beyond the end of data"));
        }
        Ok(length)
    }

    fn string(&mut self) -> Result<String, String> {
        let length = u16::from_be_bytes(self.array()?);
        decode_mutf8(self.take(length.into())?)
    }

    fn payload(&mut self, kind: u8, depth: usize) -> Result<Tag, String> {
        Ok(match kind {
            BYTE => Tag::Byte(i8::from_be_bytes(self.array()?)),
            SHORT => Tag::Short(i16::from_be_bytes(self.array()?)),
            INT => Tag::Int(i32::from_be_bytes(self.array()?)),
            LONG => Tag::Long(i64::from_be_bytes(self.array()?)),
            FLOAT => Tag::Float(f32::from_be_bytes(self.array()?)),
            DOUBLE => Tag::Double(f64::from_be_bytes(self.array()?)),
            BYTE_ARRAY => {
                let length = self.length(1)?;
                Tag::ByteArray(self.take(length)?.iter().map(|&byte| byte as i8).collect())
            }
            STRING => Tag::String(self.string()?),
            LIST => Tag::List(self.list(depth + 1)?),
            COMPOUND => Tag::Compound(self.compound(depth + 1)?),
            INT_ARRAY => {
                let length = self.length(4)?;
                Tag::IntArray(
                    self.take(length * 4)?
                        .chunks_exact(4)
                        .map(|chunk| i32::from_be_bytes(chunk.try_into().expect("4 bytes")))
                        .collect(),
                )
            }
            LONG_ARRAY => {
                let length = self.length(8)?;
                Tag::LongArray(
                    self.take(length * 8)?
                        .chunks_exact(8)
                        .map(|chunk| i64::from_be_bytes(chunk.try_into().expect("8 bytes")))
                        .collect(),
                )
            }
            kind => return Err(format!("unknown tag type {kind}")),
        })
    }

    fn list(&mut self, depth: usize) -> Result<Vec<Tag>, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let kind = self.u8()?;
        let length = self.length(min_size(kind))?;
        if kind == END && length > 0 {
            return Err("a list of end tags is not empty".into());
        }
        (0..length).map(|_| self.payload(kind, depth)).collect()
    }

    fn compound(&mut self, depth: usize) -> Result<Compound, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let mut entries = Vec::new();
        loop {
            let kind = self.u8()?;
            if kind == END {
                return Ok(Compound(entries));
            }
            let name = self.string()?;
            entries.push((name, self.payload(kind, depth)?));
        }
    }
}

fn decode_mutf8(bytes: &[u8]) -> Result<String, String> {
    if bytes.is_ascii() {
        return String::from_utf8(bytes.to_vec()).map_err(|err| err.to_string());
    }
    fn continuation(byte: Option<u8>) -> Result<u16, String> {
        match byte {
            Some(byte) if byte & 0xC0 == 0x80 => Ok(u16::from(byte & 0x3F)),
            _ => Err("invalid modified UTF-8".into()),
        }
    }
    let mut units = Vec::with_capacity(bytes.len());
    let mut bytes = bytes.iter().copied();
    while let Some(first) = bytes.next() {
        let unit = match first {
            0x00..=0x7F => u16::from(first),
            0xC0..=0xDF => (u16::from(first & 0x1F) << 6) | continuation(bytes.next())?,
            0xE0..=0xEF => {
                let high = continuation(bytes.next())?;
                (u16::from(first & 0x0F) << 12) | (high << 6) | continuation(bytes.next())?
            }
            _ => return Err("invalid modified UTF-8".into()),
        };
        units.push(unit);
    }
    String::from_utf16(&units).map_err(|_| "a string has an unpaired surrogate".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{"": {name: "Steve", Pos: [1.5d, 64d, -2.25d], DataVersion: 4189, UUID: [I; 1, 2, 3, 4]}}`
    fn sample() -> Vec<u8> {
        let mut data = vec![COMPOUND, 0, 0];
        data.extend([STRING, 0, 4]);
        data.extend(b"name");
        data.extend([0, 5]);
        data.extend(b"Steve");
        data.extend([LIST, 0, 3]);
        data.extend(b"Pos");
        data.extend([DOUBLE, 0, 0, 0, 3]);
        for value in [1.5f64, 64.0, -2.25] {
            data.extend(value.to_be_bytes());
        }
        data.extend([INT, 0, 11]);
        data.extend(b"DataVersion");
        data.extend(4189i32.to_be_bytes());
        data.extend([INT_ARRAY, 0, 4]);
        data.extend(b"UUID");
        data.extend(4i32.to_be_bytes());
        for value in 1..=4i32 {
            data.extend(value.to_be_bytes());
        }
        data.push(END);
        data
    }

    #[test]
    fn reads_tags() {
        let root = read(&sample()).unwrap();
        assert_eq!(root.str("name"), Some("Steve"));
        assert_eq!(root.numbers("Pos"), Some(vec![1.5, 64.0, -2.25]));
        assert_eq!(root.i64("DataVersion"), Some(4189));
        assert_eq!(
            root.get("UUID").and_then(Tag::as_ints),
            Some(&[1, 2, 3, 4][..])
        );
        assert_eq!(root.get("missing"), None);
    }

    #[test]
    fn refuses_truncated_and_bogus_data() {
        let data = sample();
        assert!(read(&data[..data.len() - 3]).is_err());
        assert!(read(&[STRING, 0, 0]).is_err());
        // a list claiming more elements than bytes remain
        let mut bogus = vec![COMPOUND, 0, 0, LIST, 0, 1, b'x', LONG];
        bogus.extend(i32::MAX.to_be_bytes());
        assert!(read(&bogus).is_err());
    }

    #[test]
    fn decompresses_gzip_and_passes_plain_data() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let data = sample();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&data).unwrap();
        let gz = encoder.finish().unwrap();
        assert_eq!(decompress(&gz).unwrap().as_ref(), &data[..]);
        assert_eq!(decompress(&data).unwrap().as_ref(), &data[..]);
    }

    #[test]
    fn writes_the_bytes_it_read() {
        let data = sample();
        assert_eq!(write(&read(&data).unwrap()), data);
        // real chunks, copied byte for byte from a Paper 1.21.4 world
        const OVERWORLD: &[u8] = include_bytes!("../tests/fixtures/overworld.mca");
        let slots = crate::anvil::slots(OVERWORLD).unwrap();
        assert_eq!(slots.len(), 5);
        for slot in slots {
            let (compression, payload) = crate::anvil::payload(OVERWORLD, slot).unwrap().unwrap();
            let bytes = crate::anvil::decompress(compression, payload).unwrap();
            assert_eq!(write(&read(&bytes).unwrap()), bytes, "chunk {}", slot.index);
        }
    }

    #[test]
    fn reads_what_it_wrote() {
        let nested: Compound = [
            ("byte".to_string(), Tag::Byte(-3)),
            ("short".to_string(), Tag::Short(-300)),
            ("float".to_string(), Tag::Float(0.25)),
            ("bytes".to_string(), Tag::ByteArray(vec![-128, 0, 127])),
            ("longs".to_string(), Tag::LongArray(vec![i64::MIN, 0, i64::MAX])),
        ]
        .into_iter()
        .collect();
        let root: Compound = [
            ("ascii".to_string(), Tag::String("minecraft:stone".into())),
            ("nul".to_string(), Tag::String("a\0b".into())),
            ("umlaut é ß".to_string(), Tag::String("Grüße, 世界".into())),
            ("emoji".to_string(), Tag::String("🧱 block 𝄞".into())),
            ("empty".to_string(), Tag::List(Vec::new())),
            (
                "compounds".to_string(),
                Tag::List(vec![Tag::Compound(nested.clone()), Tag::Compound(Compound::new())]),
            ),
            ("nested".to_string(), Tag::Compound(nested)),
            ("ints".to_string(), Tag::IntArray(vec![-1, 2, i32::MAX])),
            ("long".to_string(), Tag::Long(-1 << 40)),
            ("double".to_string(), Tag::Double(-0.5)),
        ]
        .into_iter()
        .collect();
        let bytes = write(&root);
        assert_eq!(read(&bytes).unwrap(), root);

        // an empty list has element type END
        let empty: Compound = [("e".to_string(), Tag::List(Vec::new()))].into_iter().collect();
        assert_eq!(write(&empty), vec![COMPOUND, 0, 0, LIST, 0, 1, b'e', END, 0, 0, 0, 0, END]);
    }

    #[test]
    fn encodes_modified_utf8() {
        let encoded = |value: &str| {
            let mut output = Vec::new();
            write_string(&mut output, value);
            output
        };
        assert_eq!(encoded("a\0"), vec![0, 3, b'a', 0xC0, 0x80]);
        assert_eq!(encoded("é"), vec![0, 2, 0xC3, 0xA9]);
        assert_eq!(encoded("世"), vec![0, 3, 0xE4, 0xB8, 0x96]);
        // U+1F9F1 as the surrogates D83E DDF1, 3 bytes each
        assert_eq!(
            encoded("🧱"),
            vec![0, 6, 0xED, 0xA0, 0xBE, 0xED, 0xB7, 0xB1]
        );
        for value in ["a\0", "é", "世", "🧱", ""] {
            assert_eq!(decode_mutf8(&encoded(value)[2..]).unwrap(), value);
        }
        // too long: cut at a whole character
        let long = "é".repeat(40_000);
        let output = encoded(&long);
        assert_eq!(u16::from_be_bytes([output[0], output[1]]), 65534);
        assert_eq!(output.len(), 2 + 65534);
    }

    #[test]
    fn mutates_compounds_in_place() {
        let mut root = read(&sample()).unwrap();
        assert_eq!(root.len(), 4);
        assert!(root.contains("Pos") && !root.contains("pos"));
        assert_eq!(root.insert("name", Tag::String("Alex".into())), Some(Tag::String("Steve".into())));
        assert_eq!(root.insert("Health", Tag::Float(20.0)), None);
        let keys: Vec<&str> = root.iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["name", "Pos", "DataVersion", "UUID", "Health"]);
        assert_eq!(root.str("name"), Some("Alex"));

        if let Some(Tag::Int(version)) = root.get_mut("DataVersion") {
            *version = 4325;
        }
        root.get_mut("UUID").and_then(Tag::as_ints_mut).unwrap().push(5);
        root.list_mut("Pos").unwrap().pop();
        assert_eq!(root.i64("DataVersion"), Some(4325));
        assert_eq!(root.get("UUID").and_then(Tag::as_ints), Some(&[1, 2, 3, 4, 5][..]));
        assert_eq!(root.numbers("Pos"), Some(vec![1.5, 64.0]));
        assert!(root.compound_mut("name").is_none());

        assert_eq!(root.remove("Pos").map(|tag| tag.id()), Some(LIST));
        assert_eq!(root.remove("Pos"), None);
        let keys: Vec<&str> = root.iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["name", "DataVersion", "UUID", "Health"]);

        for (_, tag) in root.iter_mut() {
            if let Some(longs) = tag.as_longs_mut() {
                longs.clear();
            }
            if let Some(compound) = tag.as_compound_mut() {
                compound.insert("x", Tag::Byte(1));
            }
        }
        let mut level = Compound::new();
        assert!(level.is_empty());
        level.insert("Data", Tag::Compound(root));
        level.compound_mut("Data").unwrap().insert("y", Tag::Short(2));
        assert_eq!(level.compound("Data").unwrap().len(), 5);
        assert_eq!(read(&write(&level)).unwrap(), level);

        // repeated keys collapse when collecting
        let collected: Compound = [
            ("a".to_string(), Tag::Byte(1)),
            ("b".to_string(), Tag::Byte(2)),
            ("a".to_string(), Tag::Byte(3)),
        ]
        .into_iter()
        .collect();
        let entries: Vec<(&str, &Tag)> = collected.iter().collect();
        assert_eq!(entries, [("a", &Tag::Byte(3)), ("b", &Tag::Byte(2))]);
    }

    #[test]
    fn compresses_with_gzip_and_zlib() {
        let data = sample();
        assert_eq!(gunzip(&gzip(&data)).unwrap(), data);
        assert_eq!(inflate(&deflate(&data)).unwrap(), data);
        assert!(gzip(&data).starts_with(&[0x1F, 0x8B]));
        assert_eq!(decompress(&gzip(&data)).unwrap().as_ref(), &data[..]);
    }
}
