use crate::raw::SegmentData;
use crate::region::segment::Segment;

pub(crate) fn reconstruct_segment(segment: &Segment) -> SegmentData {
    SegmentData {
        block_sections: segment.block_sections.active().to_vec(),
        biome_sections: segment.biome_sections.active().to_vec(),
        states: segment.info.reverse_deltas.clone(),
        tile_entities: segment.tile_entities.clone(),
    }
}

pub(crate) fn encode_segment(sd: &SegmentData) -> Segment {
    debug_assert_eq!(sd.block_sections.len(), sd.biome_sections.len());
    let section_count = sd.block_sections.len();
    let mut segment = Segment::with_section_count(section_count);
    for i in 0..section_count {
        segment.block_sections.sections[i] = sd.block_sections[i].clone();
        segment.biome_sections.sections[i] = sd.biome_sections[i].clone();
    }
    segment.info.reverse_deltas = sd.states.clone();
    segment.tile_entities = sd.tile_entities.clone();
    segment
}
