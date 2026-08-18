use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::io::serialize::experimental::layout;
use crate::io::serialize::primitives::{put_bytes, put_u8, put_u16_le};
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedSnapshot};

use super::Streams;

pub(super) fn write(
    streams: &mut Streams,
    data: &RegionData,
    section_count: usize,
) -> Result<(), String> {
    let total_sections = SEGMENTS_PER_REGION * section_count;

    let mut block_counts = vec![0u16; total_sections];
    let mut biome_counts = vec![0u16; total_sections];
    let descriptor_bytes = (total_sections * 2).div_ceil(8);
    let mut block_descriptors = vec![0u8; descriptor_bytes];
    let mut biome_descriptors = vec![0u8; descriptor_bytes];

    for (slot, segment) in data.segments.iter().enumerate() {
        let Some(segment) = segment else { continue };
        for (section_index, section) in segment.block_sections.iter().enumerate() {
            let scan = slot * section_count + section_index;
            fill_descriptor_and_count(
                &mut block_descriptors,
                &mut block_counts,
                scan,
                section.snapshots(),
            )?;
        }
        for (section_index, section) in segment.biome_sections.iter().enumerate() {
            let scan = slot * section_count + section_index;
            fill_descriptor_and_count(
                &mut biome_descriptors,
                &mut biome_counts,
                scan,
                section.snapshots(),
            )?;
        }
    }

    put_bytes(&mut streams.metadata, &block_descriptors);
    put_bytes(&mut streams.metadata, &biome_descriptors);

    for &count in &block_counts {
        if count != 0 {
            put_u16_le(&mut streams.metadata, count);
        }
    }
    for &count in &biome_counts {
        if count != 0 {
            put_u16_le(&mut streams.metadata, count);
        }
    }

    write_delta_tags::<SECTION_SIZE_BLOCKS>(
        &mut streams.metadata,
        data,
        |segment| &segment.block_sections,
        layout::max_level(&block_counts),
    );
    write_delta_tags::<SECTION_SIZE_BIOMES>(
        &mut streams.metadata,
        data,
        |segment| &segment.biome_sections,
        layout::max_level(&biome_counts),
    );
    Ok(())
}

fn write_delta_tags<const UNPACKED_SIZE: usize>(
    out: &mut Vec<u8>,
    data: &RegionData,
    sections: fn(&SegmentData) -> &[PackedDeltaData<UNPACKED_SIZE>],
    levels: usize,
) {
    for level in 1..levels {
        for segment in data.segments.iter().flatten() {
            for section in sections(segment) {
                let snapshots = section.snapshots();
                if snapshots.len() > level {
                    put_u8(out, snapshot_kind(&snapshots[level].data.data));
                }
            }
        }
    }
}

fn snapshot_kind<const UNPACKED_SIZE: usize>(data: &Data<UNPACKED_SIZE>) -> u8 {
    match data {
        Data::Single(_) => 1,
        Data::Paletted(_) => 2,
    }
}

fn fill_descriptor_and_count<const UNPACKED_SIZE: usize>(
    descriptors: &mut [u8],
    counts: &mut [u16],
    scan: usize,
    snapshots: &[PackedSnapshot<UNPACKED_SIZE>],
) -> Result<(), String> {
    let descriptor = match snapshots.first() {
        None => 0,
        Some(snapshot) => snapshot_kind(&snapshot.data.data),
    };
    layout::pack_descriptor(descriptors, scan, descriptor);
    if descriptor != 0 && snapshots.len() > u16::MAX as usize {
        return Err(format!(
            "snapshot count {} exceeds 65535 for scan index {scan}",
            snapshots.len()
        ));
    }
    counts[scan] = snapshots.len() as u16;
    Ok(())
}
