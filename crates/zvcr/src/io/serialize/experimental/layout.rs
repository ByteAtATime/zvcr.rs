use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::region::palette::bits_per_entry;

pub(crate) const MAGIC: &str = "zvcrrs";
pub(crate) const HEADER_LENGTH: usize = MAGIC.len() + 4;

#[repr(u8)]
pub(crate) enum FormatVersion {
    V0_0_1 = 1,
}

impl FormatVersion {
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::V0_0_1),
            _ => None,
        }
    }
}
pub(crate) const PRESENCE_BYTES: usize = SEGMENTS_PER_REGION / 8;
pub(crate) const PART_COUNT: usize = 17;
pub(crate) const BUCKETS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
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
