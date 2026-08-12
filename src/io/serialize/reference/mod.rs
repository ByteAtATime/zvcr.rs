pub(crate) mod file;
pub(crate) mod reader;
pub(crate) mod writer;

pub(crate) use file::File;

use crate::dimension::DimensionType;
use crate::io::buffer::PooledBytes;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::EXTENSION;
use crate::io::serialize::context::Context;
use crate::io::serialize::error::ReadError;
use crate::io::serialize::types::{Reader, Writer};
use crate::raw::RegionData;
use crate::region::segment::Region;
use crate::version::{Version, ZVCR3D_LATEST_VERSION};
use std::sync::Arc;

use self::reader::ReadHandle;
use self::writer::serialize_file_to_vec;
use super::codec::encode_segment;

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

pub struct ReferenceReader {
    max_deltas: usize,
}

impl ReferenceReader {
    pub fn new(max_deltas: usize) -> Self {
        Self { max_deltas }
    }
}

impl Reader for ReferenceReader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String> {
        let header_len = EXTENSION.len() + 4;
        if bytes.len() < header_len {
            return Err(ReadError::OutOfBounds {
                offset: bytes.len(),
            }
            .to_string());
        }
        if &bytes[..EXTENSION.len()] != EXTENSION.as_bytes() {
            return Err(ReadError::HeaderMismatch.to_string());
        }
        let version_num = bytes[EXTENSION.len()];
        if version_num > ZVCR3D_LATEST_VERSION as u8 {
            return Err(ReadError::InvalidVersion(version_num).to_string());
        }
        let version = Version::from_u8(version_num)
            .ok_or(ReadError::InvalidVersion(version_num))
            .map_err(|e| e.to_string())?;
        let dim_num = bytes[EXTENSION.len() + 1];
        let dimension = DimensionType::from_u8(dim_num)
            .ok_or(ReadError::InvalidDimensionType(dim_num))
            .map_err(|e| e.to_string())?;
        let protocol_version =
            u16::from_le_bytes([bytes[EXTENSION.len() + 2], bytes[EXTENSION.len() + 3]]);

        let mut ctx = Context::default();
        ctx.initialize_section_count(dimension);
        ctx.protocol_version = protocol_version;

        let uncompressed =
            decompress_zstd(&bytes[header_len..]).map_err(|e| ReadError::Zstd(e).to_string())?;

        let mut region_handle =
            ReadHandle::new(PooledBytes::from_vec(uncompressed), self.max_deltas);
        region_handle.ctx = ctx;

        let mut rd = RegionData {
            version,
            protocol_version,
            dimension,
            segments: std::array::from_fn(|_| None),
        };

        region_handle
            .deserialize_region_data(&mut rd)
            .map_err(|e| e.to_string())?;
        Ok(rd)
    }
}

pub struct ReferenceWriter {
    level: i32,
}

impl ReferenceWriter {
    pub fn new(level: i32) -> Self {
        Self { level }
    }
}

impl Writer for ReferenceWriter {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String> {
        serialize_file_to_vec(&encode_region(data), self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::reader::read_file_at;
    use super::*;
    use crate::dimension::DimensionType;
    use crate::io::compression::ZSTD_COMPRESSION_LEVEL_DEFAULT;
    use crate::io::file_location::RegionLocation;
    use crate::raw::{reconstruct_history, reconstruct_tile_entities};
    use crate::region::delta_sequence::DeltaSequence;

    #[test]
    fn reconstruct_history_matches_snapshot_before() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: 0,
            rz: 0,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();

        for segment in file.region.segments.iter().flatten() {
            for section in segment.block_sections.active() {
                let history = reconstruct_history(section);
                for (i, snapshot) in history.iter().enumerate() {
                    let expected = section
                        .snapshot_before(section.snapshots()[i].timestamp)
                        .unwrap();
                    assert_eq!(snapshot.data, expected);
                }
            }

            for section in segment.biome_sections.active() {
                let history = reconstruct_history(section);
                for (i, snapshot) in history.iter().enumerate() {
                    let expected = section
                        .snapshot_before(section.snapshots()[i].timestamp)
                        .unwrap();
                    assert_eq!(snapshot.data, expected);
                }
            }

            let tile_history = reconstruct_tile_entities(&segment.tile_entities);
            for (i, snapshot) in tile_history.iter().enumerate() {
                let expected = segment
                    .tile_entities
                    .snapshot_before(segment.tile_entities.reverse_deltas[i].timestamp)
                    .unwrap();
                assert_eq!(snapshot.data, expected);
            }
        }
    }

    #[test]
    fn round_trip_preserves_data() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: 0,
            rz: 0,
            dimension_type: DimensionType::Overworld,
        };
        let bytes = std::fs::read(location.file_path(dir)).unwrap();
        let reader = ReferenceReader::new(0);
        let writer = ReferenceWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT);
        let rd = reader.from_bytes(&bytes).unwrap();
        let encoded = writer.to_bytes(&rd).unwrap();
        let rd2 = reader.from_bytes(&encoded).unwrap();
        assert_eq!(rd, rd2);
    }
}
