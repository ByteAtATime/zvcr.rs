use std::time::Instant;
use zvcr::raw;
use zvcr::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let location = RegionLocation {
        rx: -1,
        rz: -1,
        dimension_type: DimensionType::Overworld,
    };

    let test_dir = std::path::Path::new("test_files");

    let t0 = Instant::now();
    let new_file = match read_file_at(test_dir, &location, 0) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Could not read file, read error = {err}");
            return Ok(());
        }
    };
    let t1 = Instant::now();

    let backup_path = location
        .directory(test_dir)
        .join(format!("{}.bak", location.file_name()));
    let write_result = write_file(
        &new_file,
        &backup_path,
        ZSTD_COMPRESSION_LEVEL_DEFAULT,
        default_compression_threads(),
    );

    let bytes_written = match write_result {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("Could not write file, write error = {err}");
            return Ok(());
        }
    };
    let t2 = Instant::now();

    println!("Read took {:?}", t1.duration_since(t0));
    println!("Write took {:?}", t2.duration_since(t1));
    println!("Wrote {bytes_written} bytes");

    let t3 = Instant::now();
    let region_data = raw::reconstruct_region(&new_file);
    let t4 = Instant::now();

    let present_segments = region_data.segments.iter().filter(|s| s.is_some()).count();
    let block_snapshots: usize = region_data
        .segments
        .iter()
        .flatten()
        .map(|segment| {
            segment
                .block_sections
                .iter()
                .map(|history| history.len())
                .sum::<usize>()
        })
        .sum();
    let biome_snapshots: usize = region_data
        .segments
        .iter()
        .flatten()
        .map(|segment| {
            segment
                .biome_sections
                .iter()
                .map(|history| history.len())
                .sum::<usize>()
        })
        .sum();

    println!("Reconstruct took {:?}", t4.duration_since(t3));
    println!("Present segments = {present_segments}");
    println!("Total block snapshots = {block_snapshots}");
    println!("Total biome snapshots = {biome_snapshots}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let zero = read_file_at(dir, &location, 0);
        let one = read_file_at(dir, &location, 1);
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
