use std::sync::Arc;

use crate::definitions::{SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::io::serialize::error::ReadError;
use crate::io::serialize::experimental::layout::{self, BUCKETS, Domain};
use crate::io::serialize::experimental::models::context::GridUniforms;
use crate::io::serialize::experimental::models::spatial::{for_each_section_cell, section_origin};
use crate::io::serialize::primitives::ByteCursor;
use crate::region::packed_data::{vec_u64_to_bytes, Data, PackedData, PackedSnapshot, PalettedData};
use crate::region::palette::{bits_per_entry, DIRECT_PALETTE, MAX_INDIRECT_PALETTE_SIZE, Palette};

pub(super) struct ModeledGrid<'a> {
    pub(super) grid: &'a [u16],
    pub(super) remap: &'a [u16],
    pub(super) uniforms: &'a GridUniforms,
}

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
    level_zero: Option<&ModeledGrid>,
) -> Result<(), ReadError> {
    if level_zero.is_some() && UNPACKED_SIZE != SECTION_SIZE_BLOCKS {
        return Err(ReadError::Generic(
            "modeled level zero grid requires block sized sections".to_string(),
        ));
    }
    let mut owned_scratch = level_zero.map(|_| GridPackScratch::new());
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
                let snapshot = if let Some((modeled, scratch)) =
                    level_zero.zip(owned_scratch.as_mut()).filter(|_| level == 0)
                {
                    let data = match modeled.uniforms.rank[scan] {
                        Some(rank) => PackedData {
                            data: Data::Single(modeled.remap[rank as usize]),
                        },
                        None => pack_modeled_section(
                            modeled.grid,
                            modeled.remap,
                            slot,
                            section_index,
                            scratch,
                        ),
                    };
                    PackedSnapshot { data, timestamp }
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

const RANK_TABLE_SLOTS: usize = 512;
const RANK_TABLE_MASK: usize = RANK_TABLE_SLOTS - 1;
const EMPTY_RANK_SLOT: RankSlot = RankSlot {
    count: 0,
    key: 0,
    val: 0,
    stamp: 0,
};

#[derive(Clone, Copy)]
struct RankSlot {
    count: u32,
    key: u16,
    val: u8,
    stamp: u8,
}

struct GridPackScratch {
    cells: [u16; SECTION_SIZE_BLOCKS],
    slots: [RankSlot; RANK_TABLE_SLOTS],
    first_seen: [u16; MAX_INDIRECT_PALETTE_SIZE],
    counts: [u32; MAX_INDIRECT_PALETTE_SIZE],
    order: [usize; MAX_INDIRECT_PALETTE_SIZE],
    stamp: u8,
}

impl GridPackScratch {
    fn new() -> Self {
        Self {
            cells: [0; SECTION_SIZE_BLOCKS],
            slots: [EMPTY_RANK_SLOT; RANK_TABLE_SLOTS],
            first_seen: [0; MAX_INDIRECT_PALETTE_SIZE],
            counts: [0; MAX_INDIRECT_PALETTE_SIZE],
            order: [0; MAX_INDIRECT_PALETTE_SIZE],
            stamp: 0,
        }
    }
}

#[inline]
fn rank_slot(slots: &[RankSlot; RANK_TABLE_SLOTS], stamp: u8, rank: u16) -> usize {
    let mut at = ((rank as u32).wrapping_mul(2654435761) >> 23) as usize;
    loop {
        let slot = &slots[at];
        if slot.stamp != stamp || slot.key == rank {
            return at;
        }
        at = (at + 1) & RANK_TABLE_MASK;
    }
}

fn pack_modeled_section<const UNPACKED_SIZE: usize>(
    grid: &[u16],
    remap: &[u16],
    slot: usize,
    section_index: usize,
    scratch: &mut GridPackScratch,
) -> PackedData<UNPACKED_SIZE> {
    scratch.stamp = scratch.stamp.wrapping_add(1);
    if scratch.stamp == 0 {
        scratch.slots = [EMPTY_RANK_SLOT; RANK_TABLE_SLOTS];
        scratch.stamp = 1;
    }
    let stamp = scratch.stamp;
    let GridPackScratch {
        cells,
        slots,
        first_seen,
        counts,
        order,
        ..
    } = scratch;
    for_each_section_cell(section_origin(slot, section_index), |idx, i| {
        cells[i] = grid[idx];
    });

    let mut distinct = 0usize;
    for &rank in cells.iter() {
        let at = rank_slot(slots, stamp, rank);
        let entry = &mut slots[at];
        if entry.stamp == stamp {
            entry.count += 1;
            continue;
        }
        if distinct == MAX_INDIRECT_PALETTE_SIZE {
            let mut packed = vec![0u64; SECTION_SIZE_BLOCKS.div_ceil(4)];
            pack_direct(cells, remap, &mut packed);
            return PackedData {
                data: Data::Paletted(PalettedData {
                    packed_long_array: vec_u64_to_bytes(packed),
                    palette: DIRECT_PALETTE.clone(),
                }),
            };
        }
        *entry = RankSlot {
            count: 1,
            key: rank,
            val: distinct as u8,
            stamp,
        };
        first_seen[distinct] = rank;
        distinct += 1;
    }

    if distinct == 1 {
        return PackedData {
            data: Data::Single(remap[first_seen[0] as usize]),
        };
    }

    for pos in 0..distinct {
        counts[pos] = slots[rank_slot(slots, stamp, first_seen[pos])].count;
        order[pos] = pos;
    }
    order[..distinct].sort_unstable_by_key(|&pos| std::cmp::Reverse(counts[pos]));

    let mut sorted: Vec<u16> = Vec::with_capacity(distinct);
    for (new_idx, &pos) in order[..distinct].iter().enumerate() {
        slots[rank_slot(slots, stamp, first_seen[pos])].val = new_idx as u8;
        sorted.push(remap[first_seen[pos] as usize]);
    }

    let bits = bits_per_entry(distinct);
    let mut packed = vec![0u64; SECTION_SIZE_BLOCKS.div_ceil(64 / bits)];
    match bits {
        1 => pack_ranked::<1>(cells, slots, stamp, &mut packed),
        2 => pack_ranked::<2>(cells, slots, stamp, &mut packed),
        4 => pack_ranked::<4>(cells, slots, stamp, &mut packed),
        _ => pack_ranked::<8>(cells, slots, stamp, &mut packed),
    }
    PackedData {
        data: Data::Paletted(PalettedData {
            packed_long_array: vec_u64_to_bytes(packed),
            palette: Palette {
                palette: sorted.into(),
                bits_per_entry: bits,
            },
        }),
    }
}

fn pack_direct(
    cells: &[u16; SECTION_SIZE_BLOCKS],
    remap: &[u16],
    packed: &mut [u64],
) {
    let mask = (1u64 << 16) - 1;
    let mut unpacked_index = 0;
    for cell in packed.iter_mut() {
        let mut value = 0u64;
        let mut bit_index = 0;
        while bit_index < 64 {
            if unpacked_index >= SECTION_SIZE_BLOCKS {
                break;
            }
            let slice = remap[cells[unpacked_index] as usize];
            value |= (slice as u64 & mask) << bit_index;
            unpacked_index += 1;
            bit_index += 16;
        }
        *cell = value;
    }
}

fn pack_ranked<const BITS: usize>(
    cells: &[u16; SECTION_SIZE_BLOCKS],
    slots: &[RankSlot; RANK_TABLE_SLOTS],
    stamp: u8,
    packed: &mut [u64],
) {
    let mask = (1u64 << BITS) - 1;
    let mut unpacked_index = 0;
    for cell in packed.iter_mut() {
        let mut value = 0u64;
        let mut bit_index = 0;
        while bit_index < 64 {
            if unpacked_index >= SECTION_SIZE_BLOCKS {
                break;
            }
            let at = rank_slot(slots, stamp, cells[unpacked_index]);
            value |= (slots[at].val as u64 & mask) << bit_index;
            unpacked_index += 1;
            bit_index += BITS;
        }
        *cell = value;
    }
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
