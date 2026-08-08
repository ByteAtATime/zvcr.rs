use crate::definitions::{
    SECTION_SIZE_BLOCKS, SECTION_SIZE_BIOMES, SEGMENTS_PER_REGION, STATE_UNCHANGED,
};
use crate::dimension::DimensionType;
use crate::io::serialize::reference::File;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{PackedData, PackedSnapshot};
use crate::region::segment::{Region, Segment};
use crate::region::segment_info::SegmentState;
use crate::region::tile_entities::{DeltaTileEntityData, TileEntity, TileEntityDelta, TileEntityList};
use crate::version::Version;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<T> {
    pub timestamp: i64,
    pub data: T,
}

pub type SectionHistory<T> = Vec<Snapshot<T>>;

pub fn reconstruct_history<const N: usize>(
    packed: &PackedDeltaData<N>,
) -> SectionHistory<[u16; N]> {
    let deltas = &packed.reverse_deltas;
    if deltas.is_empty() {
        return Vec::new();
    }
    let mut current = deltas[0].data.unpack();
    let mut history = Vec::with_capacity(deltas.len());
    history.push(Snapshot {
        timestamp: deltas[0].timestamp,
        data: current,
    });
    for delta in deltas.iter().skip(1) {
        let unpacked = delta.data.unpack();
        for j in 0..N {
            if unpacked[j] != STATE_UNCHANGED {
                current[j] = unpacked[j];
            }
        }
        history.push(Snapshot {
            timestamp: delta.timestamp,
            data: current,
        });
    }
    history
}

pub fn reconstruct_tile_entities(data: &DeltaTileEntityData) -> Vec<Snapshot<TileEntityList>> {
    let deltas = &data.reverse_deltas;
    if deltas.is_empty() {
        return Vec::new();
    }
    let mut current = TileEntityList::new();
    for (pos, delta) in &deltas[0].deltas {
        if let TileEntityDelta::Put(te) = delta {
            current.insert(*pos, te.clone());
        }
    }
    let mut history = Vec::with_capacity(deltas.len());
    history.push(Snapshot {
        timestamp: deltas[0].timestamp,
        data: current.clone(),
    });
    for list_delta in deltas.iter().skip(1) {
        for (pos, delta) in &list_delta.deltas {
            match delta {
                TileEntityDelta::Put(te) => {
                    current.insert(*pos, te.clone());
                }
                TileEntityDelta::Erase => {
                    current.remove(pos);
                }
            }
        }
        history.push(Snapshot {
            timestamp: list_delta.timestamp,
            data: current.clone(),
        });
    }
    history
}

pub type BlockSectionGrid = [u16; SECTION_SIZE_BLOCKS];
pub type BiomeSectionGrid = [u16; SECTION_SIZE_BIOMES];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentData {
    pub block_sections: Vec<SectionHistory<BlockSectionGrid>>,
    pub biome_sections: Vec<SectionHistory<BiomeSectionGrid>>,
    pub states: Vec<SegmentState>,
    pub tile_entities: Vec<Snapshot<TileEntityList>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionData {
    pub version: Version,
    pub protocol_version: u16,
    pub dimension: DimensionType,
    pub segments: [Option<SegmentData>; SEGMENTS_PER_REGION],
}

pub fn reconstruct_segment(segment: &Segment) -> SegmentData {
    SegmentData {
        block_sections: segment
            .block_sections
            .active()
            .iter()
            .map(reconstruct_history)
            .collect(),
        biome_sections: segment
            .biome_sections
            .active()
            .iter()
            .map(reconstruct_history)
            .collect(),
        states: segment.info.reverse_deltas.clone(),
        tile_entities: reconstruct_tile_entities(&segment.tile_entities),
    }
}

pub fn reconstruct_region(file: &File) -> RegionData {
    RegionData {
        version: file.version,
        protocol_version: file.protocol_version,
        dimension: file.dimension_type,
        segments: std::array::from_fn(|i| {
            file.region
                .segments[i]
                .as_ref()
                .map(|arc| reconstruct_segment(arc))
        }),
    }
}

pub fn encode_history<const N: usize>(history: &SectionHistory<[u16; N]>) -> PackedDeltaData<N> {
    let mut packed = PackedDeltaData::default();
    for snap in history.iter().rev() {
        packed
            .insert_snapshot(PackedSnapshot {
                data: PackedData::pack(&snap.data),
                timestamp: snap.timestamp,
            })
            .expect("encode_history: non-canonical input");
    }
    packed
}

pub fn encode_tile_entities(history: &[Snapshot<TileEntityList>]) -> DeltaTileEntityData {
    let mut data = DeltaTileEntityData::default();
    for snap in history.iter().rev() {
        let entities: Vec<TileEntity> = snap.data.values().cloned().collect();
        data.insert_snapshot(snap.timestamp, &entities)
            .expect("encode_tile_entities: non-canonical input");
    }
    data
}

pub fn encode_segment(sd: &SegmentData) -> Segment {
    debug_assert_eq!(sd.block_sections.len(), sd.biome_sections.len());
    let section_count = sd.block_sections.len();
    let mut segment = Segment::with_section_count(section_count);
    for i in 0..section_count {
        segment.block_sections.sections[i] = encode_history(&sd.block_sections[i]);
        segment.biome_sections.sections[i] = encode_history(&sd.biome_sections[i]);
    }
    segment.info.reverse_deltas = sd.states.clone();
    segment.tile_entities = encode_tile_entities(&sd.tile_entities);
    segment
}

pub fn encode_region(rd: &RegionData) -> File {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::SEGMENTS_PER_REGION;
    use crate::dimension::DimensionType;
    use crate::io::file_location::RegionLocation;
    use crate::io::serialize::reference::reader::read_file_at;
    use crate::region::delta_sequence::DeltaSequence;
    use crate::write_file;
    use crate::{ZSTD_COMPRESSION_LEVEL_DEFAULT, default_compression_threads};

    #[test]
    fn reconstruct_history_matches_snapshot_before() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();

        for segment in file.region.segments.iter().flatten() {
            for section in segment.block_sections.active() {
                let history = reconstruct_history(section);
                for (i, snapshot) in history.iter().enumerate() {
                    let expected = section
                        .snapshot_before(section.reverse_deltas[i].timestamp)
                        .unwrap();
                    assert_eq!(snapshot.data, expected);
                }
            }

            for section in segment.biome_sections.active() {
                let history = reconstruct_history(section);
                for (i, snapshot) in history.iter().enumerate() {
                    let expected = section
                        .snapshot_before(section.reverse_deltas[i].timestamp)
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
    fn encode_produces_byte_identical_output() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();
        let encoded = encode_region(&reconstruct_region(&file));

        let tmp_original = std::env::temp_dir().join("zvcr_encode_original.bak");
        let tmp_encoded = std::env::temp_dir().join("zvcr_encode_encoded.bak");
        write_file(
            &file,
            &tmp_original,
            ZSTD_COMPRESSION_LEVEL_DEFAULT,
            default_compression_threads(),
        )
        .unwrap();
        write_file(
            &encoded,
            &tmp_encoded,
            ZSTD_COMPRESSION_LEVEL_DEFAULT,
            default_compression_threads(),
        )
        .unwrap();
        let a = std::fs::read(&tmp_original).unwrap();
        let b = std::fs::read(&tmp_encoded).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn encode_is_exact_inverse_of_reconstruct() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();
        let encoded = encode_region(&reconstruct_region(&file));

        assert_eq!(encoded.version, file.version);
        assert_eq!(encoded.protocol_version, file.protocol_version);
        assert_eq!(encoded.dimension_type, file.dimension_type);
        assert_eq!(encoded.region.protocol_version, file.region.protocol_version);
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
                    assert_eq!(a.tile_entities.reverse_deltas, b.tile_entities.reverse_deltas);
                }
                _ => panic!("segment {i} presence mismatch"),
            }
        }
    }
}
