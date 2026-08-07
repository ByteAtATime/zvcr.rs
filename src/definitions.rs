pub const REGION_SIDELENGTH_SEGMENTS: usize = 32;
pub const SEGMENTS_PER_REGION: usize = REGION_SIDELENGTH_SEGMENTS * REGION_SIDELENGTH_SEGMENTS;
pub const SEGMENT_SIDELENGTH_BLOCKS: usize = 16;
pub const SECTION_SIZE_BLOCKS: usize =
    SEGMENT_SIDELENGTH_BLOCKS * SEGMENT_SIDELENGTH_BLOCKS * SEGMENT_SIDELENGTH_BLOCKS;
pub const SEGMENT_SIDELENGTH_BIOMES: usize = 4;
pub const SECTION_SIZE_BIOMES: usize =
    SEGMENT_SIDELENGTH_BIOMES * SEGMENT_SIDELENGTH_BIOMES * SEGMENT_SIDELENGTH_BIOMES;

pub type SegmentAtom = u16;
pub const STATE_UNCHANGED: SegmentAtom = 0xFFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeltaInsertionStatus {
    #[error("Snapshot is older than or equal to the latest snapshot")]
    SnapshotOlderThanLatest,
    #[error("No changes were made between snapshots")]
    NoChangesMade,
}
