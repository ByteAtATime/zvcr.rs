use crate::definitions::*;
use crate::region::unpacked_view::UnpackedData;
use std::sync::{Arc, LazyLock};

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

pub type VectorPalette = Arc<[SegmentAtom]>;

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

pub static DIRECT_PALETTE: LazyLock<Palette> = LazyLock::new(|| Palette {
    palette: Arc::from([]),
    bits_per_entry: 16,
});

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
            return DIRECT_PALETTE.clone();
        }
        scratch.indices[atom as usize] = palette.len() as u8;
        palette.push(atom);
    }

    let mut counts = vec![0u32; palette.len()];
    for &atom in data {
        counts[scratch.indices[atom as usize] as usize] += 1;
    }

    let mut order: Vec<usize> = (0..palette.len()).collect();
    order.sort_unstable_by_key(|&i| std::cmp::Reverse(counts[i]));

    let mut sorted = Vec::with_capacity(palette.len());
    for &i in &order {
        sorted.push(palette[i]);
    }
    for (new_idx, &i) in order.iter().enumerate() {
        scratch.indices[palette[i] as usize] = new_idx as u8;
    }

    let bpe = bits_per_entry(sorted.len());
    Palette {
        palette: sorted.into(),
        bits_per_entry: bpe,
    }
}
