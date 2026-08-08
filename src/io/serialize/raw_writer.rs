use crate::io::serialize::writer::serialize_file_to_vec;
use crate::raw::{RegionData, encode_region};
use std::path::Path;

pub trait Writer {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String>;

    fn write(&self, data: &RegionData, dst: &Path) -> Result<usize, String> {
        let bytes = self.to_bytes(data)?;
        std::fs::write(dst, &bytes).map_err(|e| format!("Failed to write file to disk: {e}"))?;
        Ok(bytes.len())
    }
}

pub struct ReferenceWriter {
    level: i32,
    threads: u32,
}

impl ReferenceWriter {
    pub fn new(level: i32, threads: u32) -> Self {
        Self { level, threads }
    }
}

impl Writer for ReferenceWriter {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String> {
        serialize_file_to_vec(&encode_region(data), self.level, self.threads)
    }
}
