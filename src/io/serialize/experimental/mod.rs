pub(crate) mod file;
pub(crate) mod reader;
pub(crate) mod writer;

pub(crate) use file::File;

use crate::io::buffer::PooledBytes;
use crate::io::serialize::types::{Reader, Writer};
use crate::raw::RegionData;
use crate::region::segment::Region;
use std::sync::Arc;

use self::reader::ReadHandle;
use self::writer::serialize_file_to_vec;
use super::codec::{encode_segment, reconstruct_segment};

pub(crate) fn reconstruct_region(file: &File) -> RegionData {
    RegionData {
        version: file.version,
        protocol_version: file.protocol_version,
        dimension: file.dimension_type,
        segments: std::array::from_fn(|i| {
            file.region.segments[i]
                .as_ref()
                .map(|arc| reconstruct_segment(arc))
        }),
    }
}

pub(crate) fn encode_region(rd: &RegionData) -> File {
    let mut region = Region::new(rd.protocol_version);
    for (i, slot) in rd.segments.iter().enumerate() {
        region.segments[i] = slot.as_ref().map(|sd| Arc::new(encode_segment(sd)));
    }
    File {
        version: rd.version,
        protocol_version: rd.protocol_version,
        dimension_type: rd.dimension,
        region,
    }
}

pub struct ExperimentalReader {
    max_deltas: usize,
}

impl ExperimentalReader {
    pub fn new(max_deltas: usize) -> Self {
        Self { max_deltas }
    }
}

impl Reader for ExperimentalReader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String> {
        let mut handle = ReadHandle::new(PooledBytes::from_vec(bytes.to_vec()), self.max_deltas);
        let file = handle.deserialize_file().map_err(|e| e.to_string())?;
        Ok(reconstruct_region(&file))
    }
}

pub struct ExperimentalWriter {
    level: i32,
}

impl ExperimentalWriter {
    pub fn new(level: i32) -> Self {
        Self { level }
    }
}

impl Writer for ExperimentalWriter {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String> {
        serialize_file_to_vec(&encode_region(data), self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::writer::write_file;
    use super::*;
    use crate::definitions::SEGMENTS_PER_REGION;
    use crate::dimension::DimensionType;
    use crate::io::compression::ZSTD_COMPRESSION_LEVEL_DEFAULT;
    use crate::io::file_location::RegionLocation;
    use crate::{Reader, ReferenceReader};

    fn read_experimental_file() -> File {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let region_data = ReferenceReader::new(0)
            .read(&location.file_path(dir))
            .unwrap();
        encode_region(&region_data)
    }

    #[test]
    fn encode_produces_byte_identical_output() {
        let file = read_experimental_file();
        let encoded = encode_region(&reconstruct_region(&file));

        let tmp_original = std::env::temp_dir().join("zvcr_encode_original.bak");
        let tmp_encoded = std::env::temp_dir().join("zvcr_encode_encoded.bak");
        write_file(&file, &tmp_original, ZSTD_COMPRESSION_LEVEL_DEFAULT).unwrap();
        write_file(&encoded, &tmp_encoded, ZSTD_COMPRESSION_LEVEL_DEFAULT).unwrap();
        let a = std::fs::read(&tmp_original).unwrap();
        let b = std::fs::read(&tmp_encoded).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn encode_is_exact_inverse_of_reconstruct() {
        let file = read_experimental_file();
        let encoded = encode_region(&reconstruct_region(&file));

        assert_eq!(encoded.version, file.version);
        assert_eq!(encoded.protocol_version, file.protocol_version);
        assert_eq!(encoded.dimension_type, file.dimension_type);
        assert_eq!(
            encoded.region.protocol_version,
            file.region.protocol_version
        );
        for i in 0..SEGMENTS_PER_REGION {
            match (&encoded.region.segments[i], &file.region.segments[i]) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    let (a, b) = (a.as_ref(), b.as_ref());
                    assert_eq!(
                        a.block_sections.section_count,
                        b.block_sections.section_count
                    );
                    assert_eq!(
                        a.biome_sections.section_count,
                        b.biome_sections.section_count
                    );
                    for s in 0..a.block_sections.section_count {
                        assert_eq!(a.block_sections.sections[s], b.block_sections.sections[s]);
                        assert_eq!(a.biome_sections.sections[s], b.biome_sections.sections[s]);
                    }
                    assert_eq!(a.info.reverse_deltas, b.info.reverse_deltas);
                    assert_eq!(
                        a.tile_entities.reverse_deltas,
                        b.tile_entities.reverse_deltas
                    );
                }
                _ => panic!("segment {i} presence mismatch"),
            }
        }
    }

    #[test]
    fn full_byte_roundtrip_preserves_data() {
        let file = read_experimental_file();
        let region_data = reconstruct_region(&file);

        let bytes = serialize_file_to_vec(&file, ZSTD_COMPRESSION_LEVEL_DEFAULT).unwrap();

        let reader = ExperimentalReader::new(0);
        let decoded = reader.from_bytes(&bytes).unwrap();

        for i in 0..SEGMENTS_PER_REGION {
            match (&region_data.segments[i], &decoded.segments[i]) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(
                        a.block_sections.len(),
                        b.block_sections.len(),
                        "block section count mismatch for segment {i}"
                    );
                    for s in 0..a.block_sections.len() {
                        let orig = a.block_sections[s].snapshots()[0].data.unpack();
                        let dec = b.block_sections[s].snapshots()[0].data.unpack();
                        assert_eq!(orig, dec, "block section {i}/{s} mismatch");
                        let orig_biome = a.biome_sections[s].snapshots()[0].data.unpack();
                        let dec_biome = b.biome_sections[s].snapshots()[0].data.unpack();
                        assert_eq!(orig_biome, dec_biome, "biome section {i}/{s} mismatch");
                    }
                }
                _ => panic!("segment {i} presence mismatch"),
            }
        }
    }
}
