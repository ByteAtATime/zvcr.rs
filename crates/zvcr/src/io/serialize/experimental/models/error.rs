use crate::io::serialize::error::ReadError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ModelError {
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error("model voxel count {actual} does not match expected {expected}")]
    VoxelCountMismatch { actual: usize, expected: usize },
    #[error("unsupported model encoding mode {0}")]
    UnsupportedMode(u8),
    #[error("uniform rank {rank} out of range {len}")]
    UniformRankOutOfRange { rank: usize, len: usize },
    #[error("palette rank {rank} out of range {len}")]
    PaletteRankOutOfRange { rank: usize, len: usize },
    #[error("invalid section bit depth {0}")]
    InvalidBitDepth(usize),
    #[error("palette length {len} inconsistent with bit depth {bit_depth}")]
    PaletteInconsistent { len: usize, bit_depth: usize },
    #[error("palette index {index} out of range {len}")]
    PaletteIndexOutOfRange { index: usize, len: usize },
}
