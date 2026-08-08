use crate::io::serialize::writer::write_file;
use crate::raw::{RegionData, encode_region};
use std::path::Path;

pub trait Writer {
    fn write(&self, data: &RegionData, dst: &Path) -> Result<usize, String>;
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
    fn write(&self, data: &RegionData, dst: &Path) -> Result<usize, String> {
        write_file(&encode_region(data), dst, self.level, self.threads)
    }
}
