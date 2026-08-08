use crate::raw::{
    SegmentData, SectionHistory, Snapshot, reconstruct_history,
    reconstruct_tile_entities,
};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{PackedData, PackedSnapshot};
use crate::region::palette::PackScratch;
use crate::region::segment::Segment;
use crate::region::tile_entities::{DeltaTileEntityData, TileEntity, TileEntityList};

pub(crate) fn reconstruct_segment(segment: &Segment) -> SegmentData {
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

fn encode_history<const N: usize>(history: &SectionHistory<[u16; N]>) -> PackedDeltaData<N> {
    let mut packed = PackedDeltaData::default();
    let mut scratch = PackScratch::new();
    for snap in history.iter().rev() {
        packed
            .insert_snapshot(PackedSnapshot {
                data: PackedData::pack_with(&snap.data, &mut scratch),
                timestamp: snap.timestamp,
            })
            .expect("encode_history: non-canonical input");
    }
    packed
}

fn encode_tile_entities(history: &[Snapshot<TileEntityList>]) -> DeltaTileEntityData {
    let mut data = DeltaTileEntityData::default();
    for snap in history.iter().rev() {
        let entities: Vec<TileEntity> = snap.data.values().cloned().collect();
        data.insert_snapshot(snap.timestamp, &entities)
            .expect("encode_tile_entities: non-canonical input");
    }
    data
}

pub(crate) fn encode_segment(sd: &SegmentData) -> Segment {
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
