use crate::definitions::*;
use crate::region::packed_data::{PackedData, PackedSnapshot};

pub type UnpackedData<const UNPACKED_SIZE: usize> = [SegmentAtom; UNPACKED_SIZE];

#[derive(Debug, Clone)]
pub struct UnpackedView<const UNPACKED_SIZE: usize> {
    pub unpacked: UnpackedData<UNPACKED_SIZE>,
    sidelength: u8,
}

impl<const UNPACKED_SIZE: usize> UnpackedView<UNPACKED_SIZE> {
    pub fn new(sidelength: u8, fill: SegmentAtom) -> Self {
        Self {
            unpacked: [fill; UNPACKED_SIZE],
            sidelength,
        }
    }

    pub fn from_data(sidelength: u8, unpacked: UnpackedData<UNPACKED_SIZE>) -> Self {
        Self {
            unpacked,
            sidelength,
        }
    }

    pub fn voxel(&self, x: u8, y: u8, z: u8) -> SegmentAtom {
        self.unpacked[self.unpacked_index(x, y, z)]
    }

    pub fn set_voxel(&mut self, x: u8, y: u8, z: u8, voxel: SegmentAtom) {
        let idx = self.unpacked_index(x, y, z);
        self.unpacked[idx] = voxel;
    }

    pub fn pack(&self) -> PackedData<UNPACKED_SIZE> {
        PackedData::pack(&self.unpacked)
    }

    pub fn pack_snapshot(&self, timestamp: i64) -> PackedSnapshot<UNPACKED_SIZE> {
        PackedSnapshot {
            data: self.pack(),
            timestamp,
        }
    }

    pub fn unpacked_index(&self, x: u8, y: u8, z: u8) -> usize {
        assert!(x < self.sidelength, "X coordinate out of bounds");
        assert!(y < self.sidelength, "Y coordinate out of bounds");
        assert!(z < self.sidelength, "Z coordinate out of bounds");
        (y as usize) * (self.sidelength as usize) * (self.sidelength as usize)
            + (z as usize) * (self.sidelength as usize)
            + (x as usize)
    }

    pub fn sidelength(&self) -> u8 {
        self.sidelength
    }
}

pub fn create_block_view(fill: SegmentAtom) -> UnpackedView<SECTION_SIZE_BLOCKS> {
    UnpackedView::new(SEGMENT_SIDELENGTH_BLOCKS as u8, fill)
}

pub fn create_biome_view(fill: SegmentAtom) -> UnpackedView<SECTION_SIZE_BIOMES> {
    UnpackedView::new(SEGMENT_SIDELENGTH_BIOMES as u8, fill)
}