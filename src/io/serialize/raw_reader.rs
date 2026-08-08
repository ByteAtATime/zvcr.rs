use crate::io::serialize::reader::ReadHandle;
use crate::raw::{RegionData, reconstruct_region};
use std::path::Path;

pub trait Reader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String>;

    fn read(&self, src: &Path) -> Result<RegionData, String> {
        let bytes = std::fs::read(src).map_err(|e| format!("Failed to read file from disk: {e}"))?;
        self.from_bytes(&bytes)
    }
}

pub struct ReferenceReader {
    max_deltas: usize,
}

impl ReferenceReader {
    pub fn new(max_deltas: usize) -> Self {
        Self { max_deltas }
    }
}

impl Reader for ReferenceReader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String> {
        let mut handle = ReadHandle::new(bytes.to_vec(), self.max_deltas);
        let file = handle.deserialize_file().map_err(|e| e.to_string())?;
        Ok(reconstruct_region(&file))
    }
}
