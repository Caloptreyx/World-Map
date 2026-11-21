//! Reading Java Edition NBT: big-endian named tags with modified UTF-8 strings. Adapted from
//! Caloptreyx/Player-Manager (MIT), reader only.
use flate2::read::{GzDecoder, ZlibDecoder};
use std::{borrow::Cow, io::Read};

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
}

impl Compound {
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
}
