use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;
use zstd::bulk::{Compressor, Decompressor};

pub const ZSTD_COMPRESSION_LEVEL_DEFAULT: i32 = 8;

pub(crate) static COMPRESS_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static DECOMPRESS_NS: AtomicU64 = AtomicU64::new(0);

struct CachedCompressor {
    level: i32,
    compressor: Compressor<'static>,
}

thread_local! {
    static ZSTD_COMPRESSOR: RefCell<Option<CachedCompressor>> = const { RefCell::new(None) };
    static ZSTD_DECOMPRESSOR: RefCell<Option<Decompressor<'static>>> = const { RefCell::new(None) };
}

pub fn default_compression_threads() -> u32 {
    thread::available_parallelism()
        .map(|n| (n.get() / 2) as u32)
        .unwrap_or(1)
}

pub fn compress_zstd_parts(parts: &[&[u8]], compression_level: i32) -> Result<Vec<u8>, String> {
    let t = Instant::now();

    let contiguous_input = parts.concat();

    let result = ZSTD_COMPRESSOR.with(|cell| {
        let mut guard = cell.borrow_mut();

        if guard.as_ref().is_none_or(|c| c.level != compression_level) {
            let compressor = Compressor::new(compression_level)
                .map_err(|e| format!("Failed to create bulk compressor: {e}"))?;
            *guard = Some(CachedCompressor {
                level: compression_level,
                compressor,
            });
        }

        let cached = guard.as_mut().unwrap();
        cached
            .compressor
            .compress(&contiguous_input)
            .map_err(|e| format!("ZSTD bulk compression error: {e}"))
    });

    COMPRESS_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    result
}

pub fn decompress_zstd(input: &[u8]) -> Result<Vec<u8>, String> {
    let t = Instant::now();

    let result = ZSTD_DECOMPRESSOR.with(|cell| {
        let mut guard = cell.borrow_mut();

        let content_size = zstd::zstd_safe::get_frame_content_size(input)
            .ok()
            .flatten()
            .and_then(|sz| usize::try_from(sz).ok());

        if let Some(content_size) = content_size {
            if guard.is_none() {
                let decompressor = Decompressor::new()
                    .map_err(|e| format!("Failed to create bulk decompressor: {e}"))?;
                *guard = Some(decompressor);
            }

            let decompressor = guard.as_mut().unwrap();
            let mut decompressed = vec![0u8; content_size];

            match decompressor.decompress_to_buffer(input, &mut decompressed) {
                Ok(written) if written == content_size => return Ok(decompressed),
                Ok(written) => {
                    decompressed.truncate(written);
                    return Ok(decompressed);
                }
                Err(_) => {
                    // fall back to decode_all (prolly shouldn't happen though)
                }
            }
        }

        zstd::decode_all(input).map_err(|e| format!("ZSTD decompression error: {e}"))
    });

    DECOMPRESS_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    result
}
