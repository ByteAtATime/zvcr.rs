use std::sync::LazyLock;

use crate::io::compression::compress_zstd_parts;
use crate::io::serialize::experimental::layout::PART_COUNT;

use super::Streams;

static ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("ZVCR_STREAM_STATS").is_some());

const PART_NAMES: [&str; PART_COUNT] = [
    "metadata",
    "model",
    "global_palette",
    "chunk_info",
    "timestamps",
    "singles",
    "local_palettes",
    "1b",
    "2b",
    "4b",
    "8b",
    "16b",
    "1m",
    "2m",
    "4m",
    "8m",
    "tile_entities",
];

pub(super) fn emit_if_enabled(streams: &Streams, level: i32) {
    if !*ENABLED {
        return;
    }
    for (name, stream) in PART_NAMES.into_iter().zip(streams.parts()) {
        let alone = compress_zstd_parts(&[stream], level).unwrap_or_default();
        eprintln!("zvcr_stream_stats {name} {} {}", stream.len(), alone.len());
    }
    let joined = streams.buckets.concat();
    let alone = compress_zstd_parts(&[joined.as_slice()], level).unwrap_or_default();
    eprintln!("zvcr_stream_stats buckets {} {}", joined.len(), alone.len());
    eprintln!(
        "zvcr_stream_stats bucket_counts {} {} {} {} {} {} {} {} {} {}",
        streams.bucket_counts[0],
        streams.bucket_counts[1],
        streams.bucket_counts[2],
        streams.bucket_counts[3],
        streams.bucket_counts[4],
        streams.bucket_counts[5],
        streams.bucket_counts[6],
        streams.bucket_counts[7],
        streams.bucket_counts[8],
        streams.bucket_counts[9],
    );
}
