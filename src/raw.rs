use crate::definitions::{
    SECTION_SIZE_BLOCKS, SECTION_SIZE_BIOMES, SEGMENTS_PER_REGION, STATE_UNCHANGED,
};
use crate::dimension::DimensionType;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{PackedData, PackedSnapshot};
use crate::region::segment::Segment;
use crate::region::segment_info::SegmentState;
use crate::region::tile_entities::{DeltaTileEntityData, TileEntity, TileEntityDelta, TileEntityList};
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
