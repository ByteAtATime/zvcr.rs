use std::sync::Arc;

use crate::definitions::*;
use crate::io::buffer::PooledBytes;
use crate::region::packed_data::{Data, PackedData, PalettedData};
use crate::region::palette::{
    ATOM_COUNT, DIRECT_PALETTE, MAX_INDIRECT_PALETTE_SIZE, Palette, bits_per_entry,
};
use crate::region::unpacked_view::UnpackedData;

pub(crate) struct ReusablePackScratch {
    indices: [u8; ATOM_COUNT],
    seen: [u8; ATOM_COUNT],
    seen_gen: u8,
    atoms: Vec<SegmentAtom>,
    counts: Vec<u32>,
    order: Vec<usize>,
    sorted: Vec<SegmentAtom>,
}

impl ReusablePackScratch {
    pub(crate) fn new() -> Self {
        Self {
            indices: [0; ATOM_COUNT],
            seen: [0; ATOM_COUNT],
            seen_gen: 0,
            atoms: Vec::new(),
            counts: Vec::new(),
            order: Vec::new(),
            sorted: Vec::new(),
        }
    }
}

pub(crate) fn build_palette_reused<const UNPACKED_SIZE: usize>(
    data: &UnpackedData<UNPACKED_SIZE>,
    scratch: &mut ReusablePackScratch,
) -> Palette {
    scratch.seen_gen = scratch.seen_gen.wrapping_add(1);
    if scratch.seen_gen == 0 {
        scratch.seen.fill(0);
        scratch.seen_gen = 1;
    }
    let seen_mark = scratch.seen_gen;

    scratch.atoms.clear();
    for &atom in data {
        if scratch.seen[atom as usize] == seen_mark {
            continue;
        }
        scratch.seen[atom as usize] = seen_mark;
        if scratch.atoms.len() >= MAX_INDIRECT_PALETTE_SIZE {
            return DIRECT_PALETTE.clone();
        }
        scratch.indices[atom as usize] = scratch.atoms.len() as u8;
        scratch.atoms.push(atom);
    }

    scratch.counts.clear();
    scratch.counts.resize(scratch.atoms.len(), 0);
    for &atom in data {
        scratch.counts[scratch.indices[atom as usize] as usize] += 1;
    }

    scratch.order.clear();
    scratch.order.extend(0..scratch.atoms.len());
    scratch
        .order
        .sort_unstable_by_key(|&i| std::cmp::Reverse(scratch.counts[i]));

    scratch.sorted.clear();
    for &i in &scratch.order {
        scratch.sorted.push(scratch.atoms[i]);
    }
    for (new_idx, &i) in scratch.order.iter().enumerate() {
        scratch.indices[scratch.atoms[i] as usize] = new_idx as u8;
    }

    let bpe = bits_per_entry(scratch.sorted.len());
    Palette {
        palette: Arc::from(&scratch.sorted[..]),
        bits_per_entry: bpe,
    }
}

fn vec_u64_to_bytes(v: Vec<u64>) -> PooledBytes {
    let len = v.len() * 8;
    let capacity = v.capacity() * 8;
    let mut v = std::mem::ManuallyDrop::new(v);
    let bytes: Vec<u8> = unsafe { Vec::from_raw_parts(v.as_mut_ptr() as *mut u8, len, capacity) };
    PooledBytes::from_vec(bytes)
}

fn pack_bits<const BITS: u8, const UNPACKED_SIZE: usize>(
    section_data: &UnpackedData<UNPACKED_SIZE>,
    direct: bool,
    indices: &[u8],
    packed: &mut [u64],
) {
    let mask = (1u64 << BITS) - 1;
    let mut unpacked_index = 0;
    for cell in packed.iter_mut() {
        let mut value = 0u64;
        let mut bit_index = 0u8;
        while bit_index < 64 {
            if unpacked_index >= UNPACKED_SIZE {
                break;
            }
            let mut slice = section_data[unpacked_index];
            if !direct {
                slice = indices[slice as usize] as u16;
            }
            value |= (slice as u64 & mask) << bit_index;
            unpacked_index += 1;
            bit_index += BITS;
        }
        *cell = value;
    }
}

pub(crate) fn pack_reused<const UNPACKED_SIZE: usize>(
    section_data: &UnpackedData<UNPACKED_SIZE>,
    scratch: &mut ReusablePackScratch,
) -> PackedData<UNPACKED_SIZE> {
    let palette = build_palette_reused(section_data, scratch);

    if palette.length() == 1 {
        return PackedData {
            data: Data::Single(palette.palette[0]),
        };
    }

    let bits = palette.bits_per_entry as u8;
    let direct = palette.direct();
    let values_per_long = 64 / palette.bits_per_entry;
    let packed_length = UNPACKED_SIZE.div_ceil(values_per_long);
    let mut packed = vec![0u64; packed_length];

    match bits {
        1 => pack_bits::<1, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed),
        2 => pack_bits::<2, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed),
        4 => pack_bits::<4, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed),
        8 => pack_bits::<8, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed),
        _ => pack_bits::<16, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed),
    }

    PackedData {
        data: Data::Paletted(PalettedData {
            packed_long_array: vec_u64_to_bytes(packed),
            palette,
        }),
    }
}
