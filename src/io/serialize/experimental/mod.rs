pub mod coders;
mod layout;
mod models;
mod pack;
pub(crate) mod reader;
pub(crate) mod writer;

use crate::io::serialize::types::{Reader, Writer};
use crate::raw::RegionData;

use self::reader::deserialize_region_data;
use self::writer::serialize_region_data;

pub struct ExperimentalReader;

impl ExperimentalReader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExperimentalReader {
    fn default() -> Self {
        Self
    }
}

impl Reader for ExperimentalReader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String> {
        deserialize_region_data(bytes).map_err(|e| e.to_string())
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
        serialize_region_data(data, self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
    use crate::dimension::DimensionType;
    use crate::io::compression::ZSTD_COMPRESSION_LEVEL_DEFAULT;
    use crate::io::file_location::RegionLocation;
    use crate::raw::SegmentData;
    use crate::region::delta::PackedDeltaData;
    use crate::region::packed_data::{Data, PackedData, PackedSnapshot};
    use crate::region::segment_info::{SegmentState, SegmentStateType};
    use crate::region::tile_entities::DeltaTileEntityData;
    use crate::version::Version;

    fn read_reference_region_data() -> RegionData {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: 0,
            rz: 0,
            dimension_type: DimensionType::Overworld,
        };
        crate::ReferenceReader::new(0)
            .read(&location.file_path(dir))
            .unwrap()
    }

    fn snapshots_equal<const N: usize>(a: &PackedDeltaData<N>, b: &PackedDeltaData<N>) -> bool {
        let sa = a.snapshots();
        let sb = b.snapshots();
        sa.len() == sb.len()
            && sa
                .iter()
                .zip(sb.iter())
                .all(|(x, y)| x.timestamp == y.timestamp && x.data.unpack() == y.data.unpack())
    }

    fn semantically_equal(a: &RegionData, b: &RegionData) -> bool {
        a.segments
            .iter()
            .zip(b.segments.iter())
            .all(|(sa, sb)| match (sa, sb) {
                (None, None) => true,
                (Some(x), Some(y)) => {
                    x.block_sections.len() == y.block_sections.len()
                        && x.biome_sections.len() == y.biome_sections.len()
                        && x.block_sections
                            .iter()
                            .zip(y.block_sections.iter())
                            .all(|(p, q)| snapshots_equal(p, q))
                        && x.biome_sections
                            .iter()
                            .zip(y.biome_sections.iter())
                            .all(|(p, q)| snapshots_equal(p, q))
                        && x.states == y.states
                        && x.tile_entities == y.tile_entities
                }
                _ => false,
            })
    }

    #[test]
    fn roundtrip_preserves_region_data() {
        let region_data = read_reference_region_data();
        let bytes = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT)
            .to_bytes(&region_data)
            .unwrap();
        let decoded = ExperimentalReader::new().from_bytes(&bytes).unwrap();

        assert_eq!(region_data.version, decoded.version);
        assert_eq!(region_data.protocol_version, decoded.protocol_version);
        assert_eq!(region_data.dimension, decoded.dimension);
        assert!(semantically_equal(&region_data, &decoded));
    }

    #[test]
    fn roundtrip_direct_bpe16_block_snapshot() {
        let dimension = DimensionType::Overworld;
        let section_count = dimension.section_count();
        let mut grid = [0u16; SECTION_SIZE_BLOCKS];
        for (i, slot) in grid.iter_mut().enumerate() {
            *slot = i as u16;
        }
        let direct_snapshot = PackedSnapshot {
            data: PackedData::pack(&grid),
            timestamp: 42,
        };
        assert!(matches!(
            &direct_snapshot.data.data,
            Data::Paletted(paletted) if paletted.palette.direct()
        ));
        let block_sections: Vec<PackedDeltaData<SECTION_SIZE_BLOCKS>> = (0..section_count)
            .map(|section_index| {
                if section_index == section_count / 2 {
                    PackedDeltaData::new(vec![direct_snapshot.clone()])
                } else {
                    PackedDeltaData::default()
                }
            })
            .collect();
        let biome_sections: Vec<PackedDeltaData<SECTION_SIZE_BIOMES>> = (0..section_count)
            .map(|_| PackedDeltaData::default())
            .collect();
        let mut segments: [Option<SegmentData>; SEGMENTS_PER_REGION] =
            std::array::from_fn(|_| None);
        segments[5] = Some(SegmentData {
            block_sections,
            biome_sections,
            states: vec![SegmentState {
                state_type: SegmentStateType::New,
                timestamp: 7,
            }],
            tile_entities: DeltaTileEntityData::default(),
        });
        let region_data = RegionData {
            version: Version::default(),
            protocol_version: 769,
            dimension,
            segments,
        };

        let bytes = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT)
            .to_bytes(&region_data)
            .unwrap();
        let decoded = ExperimentalReader::new().from_bytes(&bytes).unwrap();

        assert_eq!(region_data.version, decoded.version);
        assert_eq!(region_data.protocol_version, decoded.protocol_version);
        assert_eq!(region_data.dimension, decoded.dimension);
        assert!(semantically_equal(&region_data, &decoded));
    }
}
