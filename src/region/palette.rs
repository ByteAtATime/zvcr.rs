use crate::definitions::*;
use crate::region::unpacked_view::UnpackedData;

pub const MAX_INDIRECT_PALETTE_SIZE: usize = u8::MAX as usize + 1;

pub fn bits_per_entry(palette_length: usize) -> usize {
    if palette_length <= 16 {
        4
    } else if palette_length <= MAX_INDIRECT_PALETTE_SIZE {
        8
    } else {
        16
    }
}

pub type VectorPalette = Vec<SegmentAtom>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Palette {
    pub palette: VectorPalette,
    pub bits_per_entry: usize,
}

impl Palette {
    pub fn length(&self) -> usize {
        self.palette.len()
    }

    pub fn direct(&self) -> bool {
        self.palette.is_empty()
    }
}

pub const DIRECT_PALETTE: Palette = Palette {
    palette: Vec::new(),
    bits_per_entry: 16,
};

pub fn build_palette<const UNPACKED_SIZE: usize>(
    data: &UnpackedData<UNPACKED_SIZE>,
    indices: &mut [u8; u16::MAX as usize + 1],
) -> Palette {
    let mut palette = Vec::new();
    let mut unique = vec![false; u16::MAX as usize + 1];

    for &atom in data {
        if unique[atom as usize] {
            continue;
        }
        unique[atom as usize] = true;
        if palette.len() >= MAX_INDIRECT_PALETTE_SIZE {
            return DIRECT_PALETTE;
        }
        indices[atom as usize] = palette.len() as u8;
        palette.push(atom);
    }

    let bpe = bits_per_entry(palette.len());
    Palette {
        palette,
        bits_per_entry: bpe,
    }
}