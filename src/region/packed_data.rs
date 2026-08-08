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
        let mask = (1u64 << bits) - 1;
        let mut unpacked_index = 0;

        for cell_index in 0..paletted_data.packed_long_array.len() {
            let mut cell = 0u64;
            let mut bit_index = 0u8;
            while bit_index < 64 {
                if unpacked_index >= UNPACKED_SIZE {
                    break;
                }
                let mut slice = section_data[unpacked_index];
                if !direct {
                    slice = scratch.indices[slice as usize] as u16;
                }
                cell |= (slice as u64 & mask) << bit_index;
                unpacked_index += 1;
                bit_index += bits;
            }
            paletted_data.packed_long_array[cell_index] = cell;
        }

        Self {
            data: Data::Paletted(paletted_data),
        }
    }

    pub fn unpack(&self) -> UnpackedData<UNPACKED_SIZE> {
        match &self.data {
            Data::Single(atom) => [*atom; UNPACKED_SIZE],
            Data::Paletted(paletted_data) => {
                let mut unpacked = [0u16; UNPACKED_SIZE];
                let palette = &paletted_data.palette;
                let bits = palette.bits_per_entry as u8;
                let mask = (1u64 << bits) - 1;
                let mut unpacked_index = 0;

                for &cell in &paletted_data.packed_long_array {
                    let mut bit_index = 0u8;
                    while bit_index < 64 {
                        if unpacked_index >= UNPACKED_SIZE {
                            break;
                        }
                        let mut slice = (cell >> bit_index) & mask;
                        if !palette.direct() {
                            slice = palette.palette[slice as usize] as u64;
                        }
                        unpacked[unpacked_index] = slice as u16;
                        unpacked_index += 1;
                        bit_index += bits;
                    }
                }
                unpacked
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSnapshot<const UNPACKED_SIZE: usize> {
    pub data: PackedData<UNPACKED_SIZE>,
    pub timestamp: i64,
}
