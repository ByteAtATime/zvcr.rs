pub(crate) const SIDE: usize = 512;
pub(crate) const Z_STRIDE: usize = SIDE;
pub(crate) const Y_STRIDE: usize = SIDE * SIDE;
pub(crate) const SEGMENT_SIDE: usize = 32;
pub(crate) const SECTION_SIDE: usize = 16;

pub(crate) type SectionOrigin = (usize, usize, usize);

#[derive(Clone, Copy)]
pub(crate) struct SectionPos {
    pub(crate) slot: usize,
    pub(crate) section_y: usize,
}

impl SectionPos {
    pub(crate) fn origin(self) -> SectionOrigin {
        section_origin(self.slot, self.section_y)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VoxelPos {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) z: usize,
}

impl VoxelPos {
    pub(crate) fn index(self) -> usize {
        self.y * Y_STRIDE + self.z * Z_STRIDE + self.x
    }
}

pub(crate) fn section_origin(slot: usize, section_y: usize) -> SectionOrigin {
    (
        (slot / SEGMENT_SIDE) * SECTION_SIDE,
        (slot % SEGMENT_SIDE) * SECTION_SIDE,
        section_y * SECTION_SIDE,
    )
}

#[inline]
pub(crate) fn for_each_section_cell(origin: SectionOrigin, mut visit: impl FnMut(usize, usize)) {
    let (origin_x, origin_z, origin_y) = origin;
    let mut i = 0usize;
    for local_y in 0..SECTION_SIDE {
        let plane = (origin_y + local_y) * Y_STRIDE;
        for local_z in 0..SECTION_SIDE {
            let row = plane + (origin_z + local_z) * Z_STRIDE + origin_x;
            for local_x in 0..SECTION_SIDE {
                visit(row + local_x, i);
                i += 1;
            }
        }
    }
}

pub(super) fn fill_section(
    voxels: &mut [u16],
    origin: SectionOrigin,
    values: &[u16; crate::definitions::SECTION_SIZE_BLOCKS],
) {
    for_each_section_cell(origin, |idx, i| voxels[idx] = values[i]);
}

pub(super) fn fill_uniform(voxels: &mut [u16], origin: SectionOrigin, value: u16) {
    for_each_section_cell(origin, |idx, _| voxels[idx] = value);
}
