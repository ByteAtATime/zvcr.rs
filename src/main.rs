use std::time::Instant;
use zvcr::*;

fn test_palette_packing<const SECTION_SIZE: usize>() {
    let mut buffer = [0u16; SECTION_SIZE];
    for i in 0..SECTION_SIZE {
        buffer[i] = i as u16;
    }

    let packed = PackedData::<SECTION_SIZE>::pack(&buffer);
    let unpacked = packed.unpack();
    assert_eq!(buffer, unpacked);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    test_palette_packing::<SECTION_SIZE_BLOCKS>();
    test_palette_packing::<SECTION_SIZE_BIOMES>();

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

    Ok(())
}
