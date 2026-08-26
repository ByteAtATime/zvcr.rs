use crate::definitions::SegmentAtom;
use crate::region::packed_data::PalettedData;
use crate::region::palette::ATOM_COUNT;

pub(crate) const MAX_CELLS: usize = 4096;

pub(crate) struct ReusablePackScratch {
    pub(crate) indices: [u8; ATOM_COUNT],
    pub(crate) seen: [u8; ATOM_COUNT],
    pub(crate) seen_gen: u8,
    pub(crate) values: [u8; MAX_CELLS],
    pub(crate) out: [u8; MAX_CELLS],
    pub(crate) wide: [u16; MAX_CELLS],
    pub(crate) wide_out: [u8; MAX_CELLS * 2],
    pub(crate) hist: [u32; 256],
    pub(crate) remap: [u8; 256],
    pub(crate) distinct: [u8; 256],
    pub(crate) atoms: Vec<SegmentAtom>,
    pub(crate) rank_atoms: Vec<SegmentAtom>,
    pub(crate) counts: Vec<u32>,
    pub(crate) order: Vec<u16>,
}

impl ReusablePackScratch {
    pub(crate) fn new() -> Self {
        Self {
            indices: [0; ATOM_COUNT],
            seen: [0; ATOM_COUNT],
            seen_gen: 0,
            values: [0; MAX_CELLS],
            out: [0; MAX_CELLS],
            wide: [0; MAX_CELLS],
            wide_out: [0; MAX_CELLS * 2],
            hist: [0; 256],
            remap: [0; 256],
            distinct: [0; 256],
            atoms: Vec::new(),
            rank_atoms: Vec::new(),
            counts: Vec::new(),
            order: Vec::new(),
        }
    }
}

pub(crate) fn extract_indices(source_bits: usize, bytes: &[u8], out: &mut [u8]) {
    match source_bits {
        8 => out.copy_from_slice(bytes),
        4 => {
            for (byte, pair) in bytes.iter().zip(out.chunks_exact_mut(2)) {
                pair[0] = byte & 0x0F;
                pair[1] = byte >> 4;
            }
        }
        2 => {
            for (byte, quad) in bytes.iter().zip(out.chunks_exact_mut(4)) {
                quad[0] = byte & 0b11;
                quad[1] = (byte >> 2) & 0b11;
                quad[2] = (byte >> 4) & 0b11;
                quad[3] = byte >> 6;
            }
        }
        1 => {
            for (byte, octet) in bytes.iter().zip(out.chunks_exact_mut(8)) {
                octet[0] = byte & 1;
                octet[1] = (byte >> 1) & 1;
                octet[2] = (byte >> 2) & 1;
                octet[3] = (byte >> 3) & 1;
                octet[4] = (byte >> 4) & 1;
                octet[5] = (byte >> 5) & 1;
                octet[6] = (byte >> 6) & 1;
                octet[7] = byte >> 7;
            }
        }
        _ => unreachable!("unsupported source bits per entry {source_bits}"),
    }
}

pub(crate) fn hist_indices(source_bits: usize, bytes: &[u8], hist: &mut [u16; 256]) {
    match source_bits {
        8 => {
            for &byte in bytes {
                let index = byte as usize;
                hist[index] = hist[index].wrapping_add(1);
            }
        }
        4 => {
            for &byte in bytes {
                let byte = byte as usize;
                let lo = byte & 0x0F;
                let hi = byte >> 4;
                hist[lo] = hist[lo].wrapping_add(1);
                hist[hi] = hist[hi].wrapping_add(1);
            }
        }
        2 => {
            for &byte in bytes {
                let byte = byte as usize;
                let i0 = byte & 0b11;
                let i1 = (byte >> 2) & 0b11;
                let i2 = (byte >> 4) & 0b11;
                let i3 = byte >> 6;
                hist[i0] = hist[i0].wrapping_add(1);
                hist[i1] = hist[i1].wrapping_add(1);
                hist[i2] = hist[i2].wrapping_add(1);
                hist[i3] = hist[i3].wrapping_add(1);
            }
        }
        1 => {
            for &byte in bytes {
                let ones = byte.count_ones() as u16;
                hist[1] = hist[1].wrapping_add(ones);
                hist[0] = hist[0].wrapping_add(8 - ones);
            }
        }
        _ => unreachable!("unsupported source bits per entry {source_bits}"),
    }
}

pub(crate) fn extract_direct_atoms(bytes: &[u8], wide: &mut [u16]) {
    for (atom, pair) in wide.iter_mut().zip(bytes.chunks_exact(2)) {
        *atom = u16::from_le_bytes([pair[0], pair[1]]);
    }
}

pub(crate) fn extract_atoms<const UNPACKED_SIZE: usize>(
    paletted: &PalettedData<UNPACKED_SIZE>,
    wide: &mut [u16],
) {
    let palette = &paletted.palette;
    let direct = palette.direct();
    let source_bits = palette.bits_per_entry;
    let mask = (1u64 << source_bits) - 1;
    let mut atoms = wide[..UNPACKED_SIZE].iter_mut();
    'cells: for cell in paletted.packed_long_array.chunks_exact(8) {
        let cell = u64::from_le_bytes(cell.try_into().unwrap());
        let mut bit = 0;
        while bit < 64 {
            let Some(atom) = atoms.next() else {
                break 'cells;
            };
            let slice = (cell >> bit) & mask;
            *atom = if direct {
                slice as u16
            } else {
                palette.palette[slice as usize]
            };
            bit += source_bits;
        }
    }
}

pub(crate) fn remap_values(values: &mut [u8], remap: &[u8; 256]) {
    for value in values.iter_mut() {
        *value = remap[*value as usize];
    }
}

pub(crate) fn pack_values(source_bits: usize, vals: &[u8], out: &mut [u8]) {
    match source_bits {
        8 => out.copy_from_slice(vals),
        4 => {
            for (byte, pair) in out.iter_mut().zip(vals.chunks_exact(2)) {
                *byte = pair[0] | (pair[1] << 4);
            }
        }
        2 => {
            for (byte, quad) in out.iter_mut().zip(vals.chunks_exact(4)) {
                *byte = quad[0] | (quad[1] << 2) | (quad[2] << 4) | (quad[3] << 6);
            }
        }
        1 => {
            for (byte, octet) in out.iter_mut().zip(vals.chunks_exact(8)) {
                *byte = octet[0]
                    | (octet[1] << 1)
                    | (octet[2] << 2)
                    | (octet[3] << 3)
                    | (octet[4] << 4)
                    | (octet[5] << 5)
                    | (octet[6] << 6)
                    | (octet[7] << 7);
            }
        }
        _ => unreachable!("unsupported target bits per entry {source_bits}"),
    }
}

pub(crate) fn pack_atoms_le(atoms: &[u16], out: &mut [u8]) {
    for (pair, atom) in out.chunks_exact_mut(2).zip(atoms) {
        let bytes = atom.to_le_bytes();
        pair[0] = bytes[0];
        pair[1] = bytes[1];
    }
}
