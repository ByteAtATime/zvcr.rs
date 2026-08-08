use crate::definitions::*;
use crate::region::palette::{Palette, PackScratch, build_palette_with};
use crate::region::unpacked_view::UnpackedData;

pub type LongArray = Vec<u64>;

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
            packed_long_array: vec![0; packed_length],
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

    for &cell in &paletted_data.packed_long_array {
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
        let mut paletted_data = PalettedData::new(palette);

        match bits {
            4 => pack_bits::<4, UNPACKED_SIZE>(
                section_data,
                direct,
                &scratch.indices,
                &mut paletted_data.packed_long_array,
            ),
            8 => pack_bits::<8, UNPACKED_SIZE>(
                section_data,
                direct,
                &scratch.indices,
                &mut paletted_data.packed_long_array,
            ),
            _ => pack_bits::<16, UNPACKED_SIZE>(
                section_data,
                direct,
                &scratch.indices,
                &mut paletted_data.packed_long_array,
            ),
        }

        Self {
            data: Data::Paletted(paletted_data),
        }
    }

    pub fn unpack(&self) -> UnpackedData<UNPACKED_SIZE> {
        match &self.data {
            Data::Single(atom) => [*atom; UNPACKED_SIZE],
            Data::Paletted(paletted_data) => {
                match paletted_data.palette.bits_per_entry as u8 {
                    4 => unpack_bits::<4, UNPACKED_SIZE>(paletted_data),
                    8 => unpack_bits::<8, UNPACKED_SIZE>(paletted_data),
                    _ => unpack_bits::<16, UNPACKED_SIZE>(paletted_data),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSnapshot<const UNPACKED_SIZE: usize> {
    pub data: PackedData<UNPACKED_SIZE>,
    pub timestamp: i64,
}
