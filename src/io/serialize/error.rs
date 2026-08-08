pub const MAX_DELTA_LENGTH: u64 = 65536;
pub const MAX_SEGMENT_STATES_LENGTH: u64 = 65536;
pub const MAX_TILE_ENTITY_LIST_LENGTH: u64 = 98304;
pub const MAX_TILE_ENTITY_NBT_LENGTH: u64 = 65536;
pub const MAX_PACKED_LENGTH: u64 = 1024;
pub const MAX_PALETTE_TABLE_LENGTH: u32 = 262144;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Generic read error: {0}")]
    Generic(String),
    #[error("ZSTD error: {0}")]
    Zstd(String),
    #[error("Read out of bounds at offset {offset}")]
    OutOfBounds { offset: usize },
    #[error("Invalid version: {0}")]
    InvalidVersion(u8),
    #[error("Invalid dimension type: {0}")]
    InvalidDimensionType(u8),
    #[error("Invalid palette index: {index} >= {max}")]
    InvalidPaletteIndex { index: u32, max: usize },
    #[error("Header prefix mismatch")]
    HeaderMismatch,
    #[error("Length constraint exceeded: {0}")]
    LengthExceeded(String),
}
