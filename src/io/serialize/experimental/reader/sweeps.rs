use std::sync::Arc;

use crate::definitions::SEGMENTS_PER_REGION;
use crate::io::serialize::error::ReadError;
use crate::io::serialize::experimental::layout::{self, BUCKETS, Domain};
use crate::io::serialize::experimental::models::spatial::{for_each_section_cell, section_origin};
use crate::io::serialize::primitives::ByteCursor;
use crate::region::packed_data::{Data, PackedData, PackedSnapshot, PalettedData};
use crate::region::palette::{DIRECT_PALETTE, PackScratch, Palette};

pub(super) struct RegionSlots<'a> {
    pub(super) presence: &'a [bool; SEGMENTS_PER_REGION],
    pub(super) slot_storage: &'a [usize; SEGMENTS_PER_REGION],
    pub(super) section_count: usize,
}

pub(super) struct DomainTables<'a> {
    pub(super) counts: &'a [u16],
    pub(super) descriptors: &'a [u8],
    pub(super) tags: &'a [u8],
    pub(super) timestamps: &'a [i64],
    pub(super) singles: &'a [u16],
    pub(super) palette_atoms: &'a [u16],
    pub(super) starts: &'a [u32],
    pub(super) levels: usize,
}

pub(super) fn sweep_domain<const UNPACKED_SIZE: usize>(
    meta_cursor: &mut ByteCursor,
    packed_cursors: &mut [ByteCursor; BUCKETS],
    domain: Domain,
    tables: &DomainTables,
    slots: &RegionSlots,
    storages: &mut [Vec<PackedSnapshot<UNPACKED_SIZE>>],
    level_zero: Option<(&[u16], &[u16])>,
) -> Result<(), ReadError> {
    if level_zero.is_some() && UNPACKED_SIZE != crate::definitions::SECTION_SIZE_BLOCKS {
        return Err(ReadError::Generic(
            "modeled level zero grid requires block sized sections".to_string(),
        ));
    }
    let mut owned_scratch = level_zero.map(|_| PackScratch::new());
    let mut local_atoms: Vec<u16> = Vec::new();
    let mut snapshot_index = 0usize;
    let mut single_index = 0usize;
    let mut delta_index = 0usize;
    for level in 0..tables.levels {
        for slot in 0..SEGMENTS_PER_REGION {
            if !slots.presence[slot] {
                continue;
            }
            let storage_index = slots.slot_storage[slot];
            for section_index in 0..slots.section_count {
                let scan = slot * slots.section_count + section_index;
                if tables.counts[scan] as usize <= level {
                    continue;
                }
                let timestamp = tables.timestamps[snapshot_index];
                snapshot_index += 1;
                let snapshot = if let Some((grid, remap)) = level_zero.filter(|_| level == 0) {
                    let scratch = owned_scratch.as_mut().unwrap();
                    PackedSnapshot {
                        data: pack_grid_section(grid, remap, slot, section_index, scratch)?,
                        timestamp,
                    }
                } else {
                    let kind = if level == 0 {
                        tables.descriptors[scan]
                    } else {
                        let tag = tables.tags[delta_index];
                        delta_index += 1;
                        tag
                    };
                    if kind == 1 {
                        let atom = tables.singles[single_index];
                        single_index += 1;
                        PackedSnapshot {
                            data: PackedData {
                                data: Data::Single(atom),
                            },
                            timestamp,
                        }
                    } else {
                        PackedSnapshot {
                            data: read_paletted_snapshot(
                                meta_cursor,
                                packed_cursors,
                                domain,
                                tables.palette_atoms,
                                &mut local_atoms,
                            )?,
                            timestamp,
                        }
                    }
                };
                storages[storage_index][tables.starts[scan] as usize + level] = snapshot;
            }
        }
    }
    Ok(())
}

fn pack_grid_section<const UNPACKED_SIZE: usize>(
    grid: &[u16],
    remap: &[u16],
    slot: usize,
    section_index: usize,
    scratch: &mut PackScratch,
) -> Result<PackedData<UNPACKED_SIZE>, ReadError> {
    if UNPACKED_SIZE != crate::definitions::SECTION_SIZE_BLOCKS {
        return Err(ReadError::Generic(
            "modeled grid section size mismatch".to_string(),
        ));
    }
    let mut cells = [0u16; UNPACKED_SIZE];
    for_each_section_cell(section_origin(slot, section_index), |idx, i| {
        cells[i] = remap[grid[idx] as usize];
    });
    Ok(PackedData::pack_with(&cells, scratch))
}

fn read_paletted_snapshot<const UNPACKED_SIZE: usize>(
    meta_cursor: &mut ByteCursor,
    packed_cursors: &mut [ByteCursor; BUCKETS],
    domain: Domain,
    palette_atoms: &[u16],
    local_atoms: &mut Vec<u16>,
) -> Result<PackedData<UNPACKED_SIZE>, ReadError> {
    let palette_len = meta_cursor.read_u16()? as usize;
    let bpe = layout::palette_bpe(palette_len);
    let packed_cursor = &mut packed_cursors[domain.bucket(bpe)];
    if palette_len == 0 {
        let packed_long_array = packed_cursor.take_slice(UNPACKED_SIZE * 2)?;
        return Ok(PackedData {
            data: Data::Paletted(PalettedData {
                packed_long_array,
                palette: DIRECT_PALETTE.clone(),
            }),
        });
    }
    let global_length = palette_atoms.len();
    local_atoms.clear();
    for _ in 0..palette_len {
        let index = meta_cursor.read_u16()? as usize;
        if index >= global_length {
            return Err(ReadError::InvalidPaletteIndex {
                index: index as u32,
                max: global_length,
            });
        }
        local_atoms.push(palette_atoms[index]);
    }
    let palette = Palette {
        palette: Arc::from(&local_atoms[..]),
        bits_per_entry: bpe,
    };
    let packed_bytes = layout::packed_byte_len(UNPACKED_SIZE, bpe);
    let packed_long_array = packed_cursor.take_slice(packed_bytes)?;
    Ok(PackedData {
        data: Data::Paletted(PalettedData {
            packed_long_array,
            palette,
        }),
    })
}
