use crate::definitions::*;
use crate::region::unpacked_view::UnpackedData;

pub const MAX_INDIRECT_PALETTE_SIZE: usize = u8::MAX as usize + 1;
pub const ATOM_COUNT: usize = u16::MAX as usize + 1;

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

pub struct PackScratch {
    pub indices: [u8; ATOM_COUNT],
    seen: [u8; ATOM_COUNT],
    seen_gen: u8,
}

impl PackScratch {
    pub fn new() -> Self {
        Self {
            indices: [0; ATOM_COUNT],
            seen: [0; ATOM_COUNT],
            seen_gen: 0,
        }
    }
}

pub fn build_palette_with<const UNPACKED_SIZE: usize>(
    data: &UnpackedData<UNPACKED_SIZE>,
    scratch: &mut PackScratch,
) -> Palette {
    scratch.seen_gen = scratch.seen_gen.wrapping_add(1);
    if scratch.seen_gen == 0 {
        scratch.seen.fill(0);
        scratch.seen_gen = 1;
    }
    let seen_mark = scratch.seen_gen;

    let mut palette = Vec::new();
    for &atom in data {
        if scratch.seen[atom as usize] == seen_mark {
            continue;
        }
        scratch.seen[atom as usize] = seen_mark;
        if palette.len() >= MAX_INDIRECT_PALETTE_SIZE {
            return DIRECT_PALETTE;
        }
        scratch.indices[atom as usize] = palette.len() as u8;
        palette.push(atom);
    }
    let bpe = bits_per_entry(palette.len());
    Palette {
        palette,
        bits_per_entry: bpe,
    }
}
