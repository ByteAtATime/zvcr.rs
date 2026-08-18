mod meta;
mod prescan;
mod sweeps;
mod tile_entities;

use std::sync::Arc;

use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::dimension::DimensionType;
use crate::io::buffer::PooledBytes;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::EXTENSION;
use crate::io::serialize::error::ReadError;
use crate::io::serialize::experimental::layout::{self, BUCKETS, Domain, HEADER_LENGTH};
use crate::io::serialize::primitives::ByteCursor;
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedData, PackedSnapshot};
use crate::version::Version;

pub(crate) fn deserialize_region_data(bytes: &[u8]) -> Result<RegionData, ReadError> {
    let mut header = ByteCursor::new(PooledBytes::from_vec(bytes.to_vec()));
    let magic = header.take_slice(EXTENSION.len())?;
    if &magic[..] != EXTENSION.as_bytes() {
        return Err(ReadError::HeaderMismatch);
    }
    let version_u8 = header.read_u8()?;
    let version = Version::from_u8(version_u8).ok_or(ReadError::InvalidVersion(version_u8))?;
    let dimension_u8 = header.read_u8()?;
    let dimension = DimensionType::from_u8(dimension_u8)
        .ok_or(ReadError::InvalidDimensionType(dimension_u8))?;
    let protocol_version = header.read_u16()?;

    let body = decompress_zstd(&bytes[HEADER_LENGTH..]).map_err(ReadError::Zstd)?;
    let mut cursor = ByteCursor::new(PooledBytes::from_vec(body));

    let section_count = dimension.section_count();
    let presence = meta::read_presence(&mut cursor)?;
    let block_descriptors = meta::read_descriptors(&mut cursor, &presence, section_count)?;
    let biome_descriptors = meta::read_descriptors(&mut cursor, &presence, section_count)?;
    let block_counts = meta::read_counts(&mut cursor, &block_descriptors)?;
    let biome_counts = meta::read_counts(&mut cursor, &biome_descriptors)?;

    let total_block_snapshots: usize = block_counts.iter().map(|&c| c as usize).sum();
    let total_biome_snapshots: usize = biome_counts.iter().map(|&c| c as usize).sum();
    ensure_timestamps_fit(&cursor, total_block_snapshots, total_biome_snapshots)?;

    let block_tags = meta::read_delta_tags(&mut cursor, &block_counts, &block_descriptors)?;
    let biome_tags = meta::read_delta_tags(&mut cursor, &biome_counts, &biome_descriptors)?;
    let block_palette = meta::read_palette(&mut cursor)?;
    let biome_palette = meta::read_palette(&mut cursor)?;

    let mut storages =
        SnapshotStorages::new(&presence, section_count, &block_counts, &biome_counts);
    let mut states_by_segment = meta::read_states_by_segment(&mut cursor, &presence)?;

    let block_timestamps = meta::read_timestamps(&mut cursor, total_block_snapshots)?;
    let biome_timestamps = meta::read_timestamps(&mut cursor, total_biome_snapshots)?;
    let block_singles =
        meta::read_singles(&mut cursor, kind_count(&block_descriptors, &block_tags, 1))?;
    let biome_singles =
        meta::read_singles(&mut cursor, kind_count(&biome_descriptors, &biome_tags, 1))?;

    let mut sizes = [0usize; BUCKETS];
    let mut probe = ByteCursor::new(cursor.data.clone());
    probe.pos = cursor.pos;
    let block_paletted = kind_count(&block_descriptors, &block_tags, 2);
    let biome_paletted = kind_count(&biome_descriptors, &biome_tags, 2);
    prescan::prescan_domain(&mut probe, Domain::Block, block_paletted, &mut sizes)?;
    prescan::prescan_domain(&mut probe, Domain::Biome, biome_paletted, &mut sizes)?;
    let mut packed = prescan::build_packed_cursors(&cursor.data, probe.pos, &sizes)?;

    let mut meta_cursor = ByteCursor::new(cursor.data.clone());
    meta_cursor.pos = cursor.pos;
    let slots = sweeps::RegionSlots {
        presence: &presence,
        slot_storage: &storages.slot_storage,
        section_count,
    };
    sweeps::sweep_domain(
        &mut meta_cursor,
        &mut packed.cursors,
        Domain::Block,
        &sweeps::DomainTables {
            counts: &block_counts,
            descriptors: &block_descriptors,
            tags: &block_tags,
            timestamps: &block_timestamps,
            singles: &block_singles,
            palette_atoms: &block_palette,
            starts: &storages.block_starts,
            levels: layout::max_level(&block_counts),
        },
        &slots,
        &mut storages.block,
    )?;
    sweeps::sweep_domain(
        &mut meta_cursor,
        &mut packed.cursors,
        Domain::Biome,
        &sweeps::DomainTables {
            counts: &biome_counts,
            descriptors: &biome_descriptors,
            tags: &biome_tags,
            timestamps: &biome_timestamps,
            singles: &biome_singles,
            palette_atoms: &biome_palette,
            starts: &storages.biome_starts,
            levels: layout::max_level(&biome_counts),
        },
        &slots,
        &mut storages.biome,
    )?;

    let mut tail_cursor = ByteCursor::new(cursor.data.clone());
    tail_cursor.pos = packed.tail_start;
    let mut tile_entities_by_segment = Vec::new();
    for slot in 0..SEGMENTS_PER_REGION {
        if presence[slot] {
            tile_entities_by_segment.push(tile_entities::read(&mut tail_cursor)?);
        }
    }

    let mut segments: [Option<SegmentData>; SEGMENTS_PER_REGION] = std::array::from_fn(|_| None);
    for slot in 0..SEGMENTS_PER_REGION {
        if !presence[slot] {
            continue;
        }
        let storage_index = storages.slot_storage[slot];
        let block_storage = Arc::new(std::mem::take(&mut storages.block[storage_index]));
        let biome_storage = Arc::new(std::mem::take(&mut storages.biome[storage_index]));
        let block_sections: Vec<PackedDeltaData<SECTION_SIZE_BLOCKS>> = (0..section_count)
            .map(|section_index| {
                let scan = slot * section_count + section_index;
                let start = storages.block_starts[scan] as usize;
                PackedDeltaData::from_shared(
                    Arc::clone(&block_storage),
                    start..start + block_counts[scan] as usize,
                )
            })
            .collect();
        let biome_sections: Vec<PackedDeltaData<SECTION_SIZE_BIOMES>> = (0..section_count)
            .map(|section_index| {
                let scan = slot * section_count + section_index;
                let start = storages.biome_starts[scan] as usize;
                PackedDeltaData::from_shared(
                    Arc::clone(&biome_storage),
                    start..start + biome_counts[scan] as usize,
                )
            })
            .collect();
        segments[slot] = Some(SegmentData {
            block_sections,
            biome_sections,
            states: std::mem::take(&mut states_by_segment[storage_index]),
            tile_entities: std::mem::take(&mut tile_entities_by_segment[storage_index]),
        });
    }

    Ok(RegionData {
        version,
        protocol_version,
        dimension,
        segments,
    })
}

struct SnapshotStorages {
    block: Vec<Vec<PackedSnapshot<SECTION_SIZE_BLOCKS>>>,
    biome: Vec<Vec<PackedSnapshot<SECTION_SIZE_BIOMES>>>,
    slot_storage: [usize; SEGMENTS_PER_REGION],
    block_starts: Vec<u32>,
    biome_starts: Vec<u32>,
}

impl SnapshotStorages {
    fn new(
        presence: &[bool; SEGMENTS_PER_REGION],
        section_count: usize,
        block_counts: &[u16],
        biome_counts: &[u16],
    ) -> Self {
        let total_sections = SEGMENTS_PER_REGION * section_count;
        let mut block = Vec::new();
        let mut biome = Vec::new();
        let mut block_starts = vec![0u32; total_sections];
        let mut biome_starts = vec![0u32; total_sections];
        let mut slot_storage = [usize::MAX; SEGMENTS_PER_REGION];

        for slot in 0..SEGMENTS_PER_REGION {
            if !presence[slot] {
                continue;
            }
            slot_storage[slot] = block.len();
            let mut block_offset = 0usize;
            let mut biome_offset = 0usize;
            for section_index in 0..section_count {
                let scan = slot * section_count + section_index;
                block_starts[scan] = block_offset as u32;
                block_offset += block_counts[scan] as usize;
                biome_starts[scan] = biome_offset as u32;
                biome_offset += biome_counts[scan] as usize;
            }
            block.push(vec![placeholder_snapshot(); block_offset]);
            biome.push(vec![placeholder_snapshot(); biome_offset]);
        }

        Self {
            block,
            biome,
            slot_storage,
            block_starts,
            biome_starts,
        }
    }
}

fn placeholder_snapshot<const UNPACKED_SIZE: usize>() -> PackedSnapshot<UNPACKED_SIZE> {
    PackedSnapshot {
        data: PackedData {
            data: Data::Single(0),
        },
        timestamp: 0,
    }
}

fn kind_count(descriptors: &[u8], tags: &[u8], kind: u8) -> usize {
    descriptors.iter().filter(|&&d| d == kind).count() + tags.iter().filter(|&&t| t == kind).count()
}

fn ensure_timestamps_fit(
    cursor: &ByteCursor,
    total_block_snapshots: usize,
    total_biome_snapshots: usize,
) -> Result<(), ReadError> {
    let body_remaining = cursor.data.len() - cursor.pos;
    if total_block_snapshots.saturating_mul(8) > body_remaining
        || total_biome_snapshots.saturating_mul(8) > body_remaining
    {
        return Err(ReadError::LengthExceeded(
            "snapshot timestamp counts exceed remaining body".to_string(),
        ));
    }
    Ok(())
}
