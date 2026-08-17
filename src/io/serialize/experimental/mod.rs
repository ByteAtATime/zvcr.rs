pub mod coders;
pub(crate) mod reader;
pub(crate) mod writer;

use crate::io::serialize::types::{Reader, Writer};
use crate::raw::RegionData;

use self::reader::deserialize_region_data;
use self::writer::serialize_region_data;

pub struct ExperimentalReader;

impl ExperimentalReader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExperimentalReader {
    fn default() -> Self {
        Self
    }
}

impl Reader for ExperimentalReader {
    fn from_bytes(&self, bytes: &[u8]) -> Result<RegionData, String> {
        deserialize_region_data(bytes).map_err(|e| e.to_string())
    }
}

pub struct ExperimentalWriter {
    level: i32,
}

impl ExperimentalWriter {
    pub fn new(level: i32) -> Self {
        Self { level }
    }
}

impl Writer for ExperimentalWriter {
    fn to_bytes(&self, data: &RegionData) -> Result<Vec<u8>, String> {
        serialize_region_data(data, self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::DimensionType;
    use crate::io::compression::ZSTD_COMPRESSION_LEVEL_DEFAULT;
    use crate::io::file_location::RegionLocation;

    fn read_reference_region_data() -> RegionData {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: 0,
            rz: 0,
            dimension_type: DimensionType::Overworld,
        };
        crate::ReferenceReader::new(0)
            .read(&location.file_path(dir))
            .unwrap()
    }

    #[test]
    fn roundtrip_preserves_region_data() {
        let region_data = read_reference_region_data();
        let bytes = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT)
            .to_bytes(&region_data)
            .unwrap();
        let decoded = ExperimentalReader::new().from_bytes(&bytes).unwrap();

        assert_eq!(region_data, decoded);
    }
}
