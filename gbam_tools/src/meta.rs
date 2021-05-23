/// Every GBAM file has a metadata block that contains a record in a
/// JSON format and is parsed/written with [Serde](https://serde.rs/).

use super::GBAM_MAGIC;
use crate::{field_item_size, Fields, U32_SIZE, U64_SIZE};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor};
// use serde::de::{Deserialize, Deserializer};
// use serde_json::Result;
use std::collections::HashMap;
use std::io::Write;

/// Holds data related to GBAM file: gbam version, seekpos to meta.
pub(crate) struct FileInfo {
    pub gbam_version: [u32; 2],
    pub seekpos: u64,
    pub crc32: u32,
}

impl FileInfo {
    pub fn new(gbam_version: [u32; 2], seekpos: u64, crc32: u32) -> Self {
        FileInfo {
            gbam_version,
            seekpos,
            crc32,
        }
    }
}

/// The GBAM magic size is 8 bytes (U64_SIZE).
pub const FILE_INFO_SIZE: usize = U64_SIZE + U32_SIZE * 2 + U64_SIZE + U32_SIZE;

impl From<&[u8]> for FileInfo {
    fn from(bytes: &[u8]) -> Self {
        assert!(
            bytes.len() == FILE_INFO_SIZE,
            "Not enough bytes to form file info struct.",
        );
        assert_eq!(&bytes[..U64_SIZE], GBAM_MAGIC);
        let mut ver1 = &bytes[U64_SIZE..];
        let mut ver2 = &bytes[U64_SIZE + U32_SIZE..];
        let mut seekpos = &bytes[U64_SIZE + 2 * U32_SIZE..];
        let mut crc32 = &bytes[U64_SIZE + 2 * U32_SIZE + U64_SIZE..];
        FileInfo {
            gbam_version: [
                ver1.read_u32::<LittleEndian>()
                    .expect("file info is damaged: unable to read GBAM version."),
                ver2.read_u32::<LittleEndian>()
                    .expect("file info is damaged: unable to read GBAM version."),
            ],
            seekpos: seekpos
                .read_u64::<LittleEndian>()
                .expect("file info is damaged: unable to read seekpos."),
            crc32: crc32
                .read_u32::<LittleEndian>()
                .expect("file info is damaged: unable to read crc32."),
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<Vec<u8>> for FileInfo {
    fn into(self) -> Vec<u8> {
        let mut res = Vec::<u8>::new();
        res.write_all(GBAM_MAGIC).unwrap();
        for val in self.gbam_version.iter() {
            res.write_u32::<LittleEndian>(*val).unwrap();
        }
        res.write_u64::<LittleEndian>(self.seekpos).unwrap();
        res.write_u32::<LittleEndian>(self.crc32).unwrap();
        res
    }
}

/// Type of encoding used in GBAM writer
#[derive(Serialize, Deserialize, Clone)]
pub enum Codecs {
    /// Gzip encoding
    Gzip,
    /// LZ4 encoding
    Lz4,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct BlockMeta {
    pub seekpos: u64,
    pub numitems: u32,
}

#[derive(Serialize, Deserialize)]
struct FieldMeta {
    item_size: Option<u32>, // NONE for variable sized fields
    blocks_sizes: Vec<u32>, // NONE for fixed sized fields
    codec: Codecs,
    blocks: Vec<BlockMeta>,
}

impl FieldMeta {
    pub fn new(field: &Fields, codec: Codecs) -> Self {
        FieldMeta {
            item_size: match field_item_size(field) {
                Some(v) => Some(v as u32), // TODO.
                None => None,
            },
            blocks_sizes: Vec::<u32>::new(),
            codec,
            blocks: Vec::<BlockMeta>::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FileMeta {
    field_to_meta: HashMap<Fields, FieldMeta>,
}

/// This is a wrapper struct. It is necessary to create custom serializer/deserializer in Serde.
/// https://serde.rs/deserialize-map.html
pub struct FieldMetaMap(HashMap<Fields, FieldMeta>);

impl Serialize for FieldMetaMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(&k.to_string(), &v)?;
        }
        map.end()
    }
}

struct MyMapVisitor {
    marker: PhantomData<fn() -> FieldMetaMap>,
}

impl MyMapVisitor {
    fn new() -> Self {
        MyMapVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de> Visitor<'de> for MyMapVisitor {
    type Value = FieldMetaMap;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("Field to meta map")
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut map = HashMap::<Fields, FieldMeta>::with_capacity(access.size_hint().unwrap_or(0));

        // While there are entries remaining in the input, add them
        // into our map.
        while let Some((key, value)) = access.next_entry()? {
            map.insert(key, value);
        }
        Ok(FieldMetaMap(map))
    }
}
impl<'de> Deserialize<'de> for FieldMetaMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Instantiate our Visitor and ask the Deserializer to drive
        // it over the input data, resulting in an instance of MyMap.
        deserializer.deserialize_map(MyMapVisitor::new())
    }
}

impl FileMeta {
    pub fn new(codec: Codecs) -> Self {
        let mut map = HashMap::<Fields, FieldMeta>::new();
        for field in Fields::iterator() {
            map.insert(*field, FieldMeta::new(field, codec.clone()));
        }
        FileMeta { field_to_meta: map }
    }

    /// Used to retrieve BlockMeta vector mutable borrow, to push new blocks
    /// directly into it, avoiding field matching.
    pub fn get_blocks(&mut self, field: &Fields) -> &mut Vec<BlockMeta> {
        &mut self.field_to_meta.get_mut(field).unwrap().blocks
    }

    pub fn view_blocks(&self, field: &Fields) -> &Vec<BlockMeta> {
        &self.field_to_meta[field].blocks
    }

    pub fn get_field_size(&self, field: &Fields) -> &Option<u32> {
        &self.field_to_meta[field].item_size
    }

    pub fn get_field_codec(&self, field: &Fields) -> &Codecs {
        &self.field_to_meta[field].codec
    }
    pub fn get_blocks_sizes(&mut self, field: &Fields) -> &mut Vec<u32> {
        self.field_to_meta
            .get_mut(field)
            .unwrap()
            .blocks_sizes
            .as_mut()
    }

    pub fn push_block_size(&mut self, field: &Fields, size: usize) {
        self.field_to_meta
            .get_mut(field)
            .unwrap()
            .blocks_sizes
            .push(size as u32);
    }
}
