use crate::definitions::{
    SECTION_SIZE_BLOCKS, SECTION_SIZE_BIOMES, SEGMENTS_PER_REGION, STATE_UNCHANGED,
};
use crate::dimension::DimensionType;
use crate::io::file_type::File;
use crate::region::delta::PackedDeltaData;
use crate::region::segment::Segment;
use crate::region::segment_info::SegmentState;
use crate::region::tile_entities::TileEntityList;
use crate::version::Version;

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

pub type BlockSectionGrid = [u16; SECTION_SIZE_BLOCKS];
pub type BiomeSectionGrid = [u16; SECTION_SIZE_BIOMES];

#[derive(Debug, Clone)]
pub struct SegmentData {
    pub block_sections: Vec<SectionHistory<BlockSectionGrid>>,
    pub biome_sections: Vec<SectionHistory<BiomeSectionGrid>>,
    pub states: Vec<SegmentState>,
    pub tile_entities: Vec<Snapshot<TileEntityList>>,
}

#[derive(Debug, Clone)]
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
        states: Vec::new(),
        tile_entities: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::DimensionType;
    use crate::io::file_location::RegionLocation;
    use crate::io::serialize::reader::read_file_at;
    use crate::region::delta_sequence::DeltaSequence;

    #[test]
    fn reconstruct_history_matches_snapshot_before() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();
        let segment = file.region.segments.iter().flatten().next().unwrap();

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
    }
}
