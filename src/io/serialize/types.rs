use crate::raw::RegionData;
use std::path::Path;

pub trait Reader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String>;

    fn read(&self, src: &Path) -> Result<RegionData, String> {
        let bytes = std::fs::read(src).map_err(|e| format!("Failed to read file from disk: {e}"))?;
        self.from_bytes(&bytes)
    }
}

pub trait Writer {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String>;

    fn write(&self, data: &RegionData, dst: &Path) -> Result<usize, String> {
        let bytes = self.to_bytes(data)?;
        std::fs::write(dst, &bytes).map_err(|e| format!("Failed to write file to disk: {e}"))?;
        Ok(bytes.len())
    }
}
