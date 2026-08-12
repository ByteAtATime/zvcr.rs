use crate::definitions::*;
use crate::region::palette::{PackScratch, Palette, build_palette_with};
use crate::region::unpacked_view::UnpackedData;

pub type LongArray = bytes::Bytes;

fn vec_u64_to_bytes(v: Vec<u64>) -> bytes::Bytes {
    let len = v.len() * 8;
    let capacity = v.capacity() * 8;
    let mut v = std::mem::ManuallyDrop::new(v);
    let bytes: Vec<u8> = unsafe { Vec::from_raw_parts(v.as_mut_ptr() as *mut u8, len, capacity) };
    bytes::Bytes::from(bytes)
}

fn packed_u64_iter(bytes: &[u8]) -> impl Iterator<Item = u64> + '_ {
    bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedData<const UNPACKED_SIZE: usize> {
    pub packed_long_array: LongArray,
    pub palette: Palette,
}

impl<const UNPACKED_SIZE: usize> PalettedData<UNPACKED_SIZE> {
    pub fn new(palette: Palette) -> Self {
        let values_per_long = 64 / palette.bits_per_entry;
        let packed_length = UNPACKED_SIZE.div_ceil(values_per_long);
        Self {
            packed_long_array: vec_u64_to_bytes(vec![0; packed_length]),
            palette,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data<const UNPACKED_SIZE: usize> {
    Paletted(PalettedData<UNPACKED_SIZE>),
    Single(SegmentAtom),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedData<const UNPACKED_SIZE: usize> {
    pub data: Data<UNPACKED_SIZE>,
}

#[inline]
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

#[inline]
fn unpack_bits<const BITS: u8, const UNPACKED_SIZE: usize>(
    paletted_data: &PalettedData<UNPACKED_SIZE>,
) -> UnpackedData<UNPACKED_SIZE> {
    let mut unpacked = [0u16; UNPACKED_SIZE];
    let palette = &paletted_data.palette;
    let direct = palette.direct();
    let mask = (1u64 << BITS) - 1;
    let mut unpacked_index = 0;

    for cell in packed_u64_iter(&paletted_data.packed_long_array) {
        let mut bit_index = 0u8;
        while bit_index < 64 {
            if unpacked_index >= UNPACKED_SIZE {
                break;
            }
            let mut slice = (cell >> bit_index) & mask;
            if !direct {
                slice = palette.palette[slice as usize] as u64;
            }
            unpacked[unpacked_index] = slice as u16;
            unpacked_index += 1;
            bit_index += BITS;
        }
    }
    unpacked
}

#[inline]
fn merge_bits<const BITS: u8, const UNPACKED_SIZE: usize>(
    paletted_data: &PalettedData<UNPACKED_SIZE>,
    grid: &mut UnpackedData<UNPACKED_SIZE>,
) {
    let palette = &paletted_data.palette;
    let direct = palette.direct();
    let mask = (1u64 << BITS) - 1;
    let mut unpacked_index = 0;

    for cell in packed_u64_iter(&paletted_data.packed_long_array) {
        let mut bit_index = 0u8;
        while bit_index < 64 {
            if unpacked_index >= UNPACKED_SIZE {
                break;
            }
            let mut slice = (cell >> bit_index) & mask;
            if !direct {
                slice = palette.palette[slice as usize] as u64;
            }
            if slice as u16 != STATE_UNCHANGED {
                grid[unpacked_index] = slice as u16;
            }
            unpacked_index += 1;
            bit_index += BITS;
        }
    }
}

impl<const UNPACKED_SIZE: usize> PackedData<UNPACKED_SIZE> {
    pub fn pack(section_data: &UnpackedData<UNPACKED_SIZE>) -> Self {
        let mut scratch = PackScratch::new();
        Self::pack_with(section_data, &mut scratch)
    }

    pub fn pack_with(
        section_data: &UnpackedData<UNPACKED_SIZE>,
        scratch: &mut PackScratch,
    ) -> Self {
        let palette = build_palette_with(section_data, scratch);

        if palette.length() == 1 {
            return Self {
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
            _ => {
                pack_bits::<16, UNPACKED_SIZE>(section_data, direct, &scratch.indices, &mut packed)
            }
        }

        Self {
            data: Data::Paletted(PalettedData {
                packed_long_array: vec_u64_to_bytes(packed),
                palette,
            }),
        }
    }

    pub fn unpack(&self) -> UnpackedData<UNPACKED_SIZE> {
        match &self.data {
            Data::Single(atom) => [*atom; UNPACKED_SIZE],
            Data::Paletted(paletted_data) => match paletted_data.palette.bits_per_entry as u8 {
                1 => unpack_bits::<1, UNPACKED_SIZE>(paletted_data),
                2 => unpack_bits::<2, UNPACKED_SIZE>(paletted_data),
                4 => unpack_bits::<4, UNPACKED_SIZE>(paletted_data),
                8 => unpack_bits::<8, UNPACKED_SIZE>(paletted_data),
                _ => unpack_bits::<16, UNPACKED_SIZE>(paletted_data),
            },
        }
    }

    pub fn unpack_delta_into(&self, grid: &mut UnpackedData<UNPACKED_SIZE>) {
        match &self.data {
            Data::Single(atom) => {
                if *atom != STATE_UNCHANGED {
                    grid.fill(*atom);
                }
            }
            Data::Paletted(paletted_data) => match paletted_data.palette.bits_per_entry as u8 {
                1 => merge_bits::<1, UNPACKED_SIZE>(paletted_data, grid),
                2 => merge_bits::<2, UNPACKED_SIZE>(paletted_data, grid),
                4 => merge_bits::<4, UNPACKED_SIZE>(paletted_data, grid),
                8 => merge_bits::<8, UNPACKED_SIZE>(paletted_data, grid),
                _ => merge_bits::<16, UNPACKED_SIZE>(paletted_data, grid),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSnapshot<const UNPACKED_SIZE: usize> {
    pub data: PackedData<UNPACKED_SIZE>,
    pub timestamp: i64,
}
