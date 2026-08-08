use crate::definitions::STATE_UNCHANGED;
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

fn encode_history<const N: usize>(
    history: &SectionHistory<[u16; N]>,
    scratch: &mut PackScratch,
) -> PackedDeltaData<N> {
    let mut reverse_deltas = Vec::with_capacity(history.len());
    if history.is_empty() {
        return PackedDeltaData { reverse_deltas };
    }

    reverse_deltas.push(PackedSnapshot {
        data: PackedData::pack_with(&history[0].data, scratch),
        timestamp: history[0].timestamp,
    });

    for k in 1..history.len() {
        let prev = &history[k - 1].data;
        let curr = &history[k].data;
        let mut delta = [STATE_UNCHANGED; N];
        for i in 0..N {
            if curr[i] != prev[i] {
                delta[i] = curr[i];
            }
        }
        reverse_deltas.push(PackedSnapshot {
            data: PackedData::pack_with(&delta, scratch),
            timestamp: history[k].timestamp,
        });
    }

    PackedDeltaData { reverse_deltas }
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
    let mut scratch = PackScratch::new();
    for i in 0..section_count {
        segment.block_sections.sections[i] =
            encode_history(&sd.block_sections[i], &mut scratch);
        segment.biome_sections.sections[i] =
            encode_history(&sd.biome_sections[i], &mut scratch);
    }
    segment.info.reverse_deltas = sd.states.clone();
    segment.tile_entities = encode_tile_entities(&sd.tile_entities);
    segment
}
