use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::dimension::DimensionType;
pub use crate::region::delta::PackedDeltaData;
use crate::region::segment_info::SegmentState;
pub use crate::region::tile_entities::{DeltaTileEntityData, TileEntityDelta, TileEntityList};
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
        delta.data.unpack_delta_into(&mut current);
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
    pub block_sections: Vec<PackedDeltaData<SECTION_SIZE_BLOCKS>>,
    pub biome_sections: Vec<PackedDeltaData<SECTION_SIZE_BIOMES>>,
    pub states: Vec<SegmentState>,
    pub tile_entities: DeltaTileEntityData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionData {
    pub version: Version,
    pub protocol_version: u16,
    pub dimension: DimensionType,
    pub segments: [Option<SegmentData>; SEGMENTS_PER_REGION],
}
