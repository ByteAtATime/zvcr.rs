pub mod bench;
pub mod definitions;
pub mod dimension;
pub mod io;
pub mod raw;
pub mod region;
pub mod time_utils;
pub mod version;

pub use definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS};
pub use dimension::DimensionType;
pub use io::compression::{ZSTD_COMPRESSION_LEVEL_DEFAULT, default_compression_threads};
pub use io::file_location::RegionLocation;
pub use io::serialize::experimental::{ExperimentalReader, ExperimentalWriter};
pub use io::serialize::reference::{ReferenceReader, ReferenceWriter};
pub use io::serialize::types::{Reader, Writer};
pub use region::packed_data::PackedData;

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn palette_packing_roundtrip_blocks() {
        let mut buffer = [0u16; SECTION_SIZE_BLOCKS];
        for (i, slot) in buffer.iter_mut().enumerate() {
            *slot = i as u16;
        }
        let packed = PackedData::<SECTION_SIZE_BLOCKS>::pack(&buffer);
        assert_eq!(buffer, packed.unpack());
    }

    #[test]
    fn palette_packing_roundtrip_biomes() {
        let mut buffer = [0u16; SECTION_SIZE_BIOMES];
        for (i, slot) in buffer.iter_mut().enumerate() {
            *slot = i as u16;
        }
        let packed = PackedData::<SECTION_SIZE_BIOMES>::pack(&buffer);
        assert_eq!(buffer, packed.unpack());
    }

    #[test]
    fn read_succeeds_with_zero_and_one_max_deltas() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test_files"));
        let location = RegionLocation {
            rx: 0,
            rz: 0,
            dimension_type: DimensionType::Overworld,
        };
        let path = location.file_path(dir);
        let zero = ReferenceReader::new(0).read(&path);
        let one = ReferenceReader::new(1).read(&path);
        assert!(zero.is_ok(), "max_deltas=0 failed: {:?}", zero.err());
        assert!(one.is_ok(), "max_deltas=1 failed: {:?}", one.err());
    }

    #[test]
    fn from_file_name_parses_valid_and_rejects_invalid() {
        let dim = DimensionType::Overworld;

        let loc = RegionLocation::from_file_name(dim, std::path::Path::new("r.5.7.zvcr3d"));
        assert!(loc.is_some());
        let loc = loc.unwrap();
        assert_eq!(loc.rx, 5);
        assert_eq!(loc.rz, 7);

        assert!(
            RegionLocation::from_file_name(dim, std::path::Path::new("r.-1.-1.zvcr3d")).is_some()
        );
        assert!(
            RegionLocation::from_file_name(dim, std::path::Path::new("notregion.zvcr3d")).is_none()
        );
        assert!(RegionLocation::from_file_name(dim, std::path::Path::new("r.-1.-1.txt")).is_none());
        assert!(
            RegionLocation::from_file_name(dim, std::path::Path::new("r.abc.-1.zvcr3d")).is_none()
        );
    }
}
