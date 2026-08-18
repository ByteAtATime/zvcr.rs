use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::io::file_location::EXTENSION;
use crate::region::palette::bits_per_entry;

pub(crate) const HEADER_LENGTH: usize = EXTENSION.len() + 4;
pub(crate) const PRESENCE_BYTES: usize = SEGMENTS_PER_REGION / 8;
pub(crate) const PART_COUNT: usize = 16;
pub(crate) const BUCKETS: usize = 10;

#[derive(Clone, Copy)]
pub(crate) enum Domain {
    Block,
    Biome,
}

impl Domain {
    pub(crate) fn offset(self) -> usize {
        match self {
            Domain::Block => 0,
            Domain::Biome => 5,
        }
    }

    pub(crate) fn cell_count(self) -> usize {
        match self {
            Domain::Block => SECTION_SIZE_BLOCKS,
            Domain::Biome => SECTION_SIZE_BIOMES,
        }
    }

    pub(crate) fn bucket(self, bpe: usize) -> usize {
        self.offset() + bpe.trailing_zeros() as usize
    }
}

pub(crate) fn palette_bpe(palette_len: usize) -> usize {
    if palette_len == 0 {
        16
    } else {
        bits_per_entry(palette_len)
    }
}

pub(crate) fn packed_byte_len(unpacked_size: usize, bpe: usize) -> usize {
    (unpacked_size * bpe).div_ceil(64) * 8
}

pub(crate) fn max_level(counts: &[u16]) -> usize {
    counts.iter().copied().max().unwrap_or(0) as usize
}

pub(crate) fn set_presence_bit(presence: &mut [u8; PRESENCE_BYTES], slot: usize) {
    presence[slot >> 3] |= 1 << (7 - (slot & 7));
}

pub(crate) fn presence_bit(presence: &[u8; PRESENCE_BYTES], slot: usize) -> bool {
    (presence[slot >> 3] >> (7 - (slot & 7))) & 1 == 1
}

pub(crate) fn pack_descriptor(descriptors: &mut [u8], scan: usize, descriptor: u8) {
    descriptors[scan >> 2] |= descriptor << (6 - 2 * (scan & 3));
}

pub(crate) fn descriptor_bits(packed: &[u8], scan: usize) -> u8 {
    (packed[scan >> 2] >> (6 - 2 * (scan & 3))) & 0b11
}
