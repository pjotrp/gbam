/// Every GBAM file has a metadata block that contains a record in a
/// JSON format and is parsed/written with [Serde](https://serde.rs/).

use super::GBAM_MAGIC;
use crate::{U32_SIZE, U64_SIZE};
use bam_tools::record::fields::{field_item_size, Fields, FIELDS_NUM};
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
#[cfg(not(feature = "python-ffi"))]
pub const FILE_INFO_SIZE: usize = U64_SIZE + U32_SIZE * 2 + U64_SIZE + U32_SIZE;
#[cfg(feature = "python-ffi")]
pub const FILE_INFO_SIZE: usize = 28;

impl From<&[u8]> for FileInfo {
    fn from(bytes: &[u8]) -> Self {
        assert!(
            bytes.len() == FILE_INFO_SIZE,
            "Not enough bytes to form file info struct.",
        );
        if &bytes[..U64_SIZE] != GBAM_MAGIC {
            panic!("The file GBAM_MAGIC is not correct. Are you trying to open GBAM file or BAM file?");
        }
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
/// TODO: use MessagePack or another compact form of serialization.
#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum Codecs {
    /// Gzip encoding
    Gzip,
    /// LZ4 encoding
    Lz4,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
/// Currently block stats only for RefID or POS are supported.
pub(crate) struct Stat {
    pub min_value: i32,
    pub max_value: i32,
}

impl Stat {
    pub fn update(&mut self, val: i32) {
        self.max_value = std::cmp::max(val, self.max_value);
        self.min_value = std::cmp::min(val, self.min_value);
    }

    /// Checks if it's in reset state.
    #[allow(dead_code)]
    pub fn is_reset(&self) -> bool {
        (self.min_value == std::i32::MAX) && (self.max_value == std::i32::MIN)
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.min_value = std::i32::MAX;
        self.max_value = std::i32::MIN;
    }
}

impl Default for Stat {
    fn default() -> Self { 
        Self {
            min_value:std::i32::MAX,
            max_value:std::i32::MIN,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct BlockMeta {
    pub seekpos: u64,
    pub numitems: u32,
    pub block_size: u32,
    pub uncompressed_size: u64,
    pub stats: Option<Stat>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct FieldMeta {
    item_size: Option<u32>, // NONE for variable sized fields
    codec: Codecs,
    blocks: Vec<BlockMeta>,
}

impl FieldMeta {
    pub fn new(field: &Fields, codec: Codecs) -> Self {
        FieldMeta {
            item_size: field_item_size(field).map(|v| v as u32), // TODO
            codec,
            blocks: Vec::<BlockMeta>::new(),
        }
    }
}

impl Default for FieldMeta {
    fn default() -> Self {
        FieldMeta {
            item_size: None,
            codec: Codecs::Gzip,
            blocks: Vec::<BlockMeta>::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct FileMeta {
    // Improvised hashmap for speed
    #[serde(
        deserialize_with = "from_str",
        serialize_with = "serialize_field_to_meta"
    )]
    field_to_meta: [FieldMeta; FIELDS_NUM],
    sam_header: Vec<u8>,
    name_to_ref_id: Vec<(String, u32)>,
}

impl FileMeta {
    pub fn get_ref_seqs(&self) -> &Vec<(String, u32)> {
        &self.name_to_ref_id
    }

    #[allow(dead_code)]
    pub fn get_sam_header(&self) -> &[u8] {
        &self.sam_header[..]
    }
}

// To make metadata easier to read, convert to json where fields are represented
// as strings with their names, not numbers in enum.

fn serialize_field_to_meta<S>(
    meta: &[FieldMeta; FIELDS_NUM],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = FieldMetaMap(HashMap::new());
    for field in Fields::iterator() {
        map.0.insert(*field, meta[*field as usize].clone());
    }
    map.serialize(serializer)
}

fn from_str<'de, D>(deserializer: D) -> Result<[FieldMeta; FIELDS_NUM], D::Error>
where
    D: Deserializer<'de>,
{
    let mut map = FieldMetaMap::deserialize(deserializer)?;
    let mut field_to_meta: [FieldMeta; FIELDS_NUM] = Default::default();

    for field in Fields::iterator() {
        field_to_meta[*field as usize] = map.0.remove(field).unwrap();
    }

    Ok(field_to_meta)
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
    pub fn new(codec: Codecs, ref_seqs: Vec<(String, u32)>, sam_header: Vec<u8>) -> Self {
        let mut map: [FieldMeta; FIELDS_NUM] = Default::default();
        for field in Fields::iterator() {
            map[*field as usize] = FieldMeta::new(field, codec);
        }

        FileMeta {
            field_to_meta: map,
            sam_header,
            name_to_ref_id: ref_seqs,
        }
    }

    /// Used to retrieve BlockMeta vector mutable borrow, to push new blocks
    /// directly into it, avoiding field matching.
    pub fn get_blocks(&mut self, field: &Fields) -> &mut Vec<BlockMeta> {
        &mut self.field_to_meta[*field as usize].blocks
    }

    pub fn view_blocks(&self, field: &Fields) -> &Vec<BlockMeta> {
        &self.field_to_meta[*field as usize].blocks
    }

    pub fn get_field_size(&self, field: &Fields) -> &Option<u32> {
        &self.field_to_meta[*field as usize].item_size
    }

    pub fn get_field_codec(&self, field: &Fields) -> &Codecs {
        &self.field_to_meta[*field as usize].codec
    }
}
