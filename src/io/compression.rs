use std::thread;

pub const ZSTD_COMPRESSION_LEVEL_DEFAULT: i32 = 8;

pub fn default_compression_threads() -> u32 {
    (thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        / 2) as u32
}

pub fn compress_zstd(
    input: &[u8],
    compression_level: i32,
    compression_threads: u32,
) -> Result<Vec<u8>, String> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), compression_level)
        .map_err(|e| format!("Failed to create ZSTD encoder: {e}"))?;

    encoder
        .set_pledged_src_size(Some(input.len() as u64))
        .map_err(|e| format!("Failed to set pledged source size: {e}"))?;

    if compression_threads > 0 {
        encoder
            .set_parameter(zstd::stream::raw::CParameter::NbWorkers(
                compression_threads,
            ))
            .map_err(|e| format!("Failed to set compression workers: {e}"))?;
    }

    use std::io::Write;
    encoder
        .write_all(input)
        .map_err(|e| format!("Compression write error: {e}"))?;

    encoder
        .finish()
        .map_err(|e| format!("Failed to finish ZSTD compression: {e}"))
}

pub fn decompress_zstd(input: &[u8]) -> Result<Vec<u8>, String> {
    zstd::stream::decode_all(input).map_err(|e| format!("Failed to decompress ZSTD stream: {e}"))
}
