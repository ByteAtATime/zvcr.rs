mod models;

use clap::Parser;
use rayon::prelude::*;
use std::path::Path;
use std::time::Instant;

use models::{Modeler, SectionContext, find_modeler};
use zvcr::Reader;
use zvcr::definitions::REGION_SIDELENGTH_SEGMENTS;
use zvcr::io::compression::{ZSTD_COMPRESSION_LEVEL_DEFAULT, compress_zstd_parts, decompress_zstd};
use zvcr::raw::RegionData;
use zvcr::region::packed_data::Data;
use zvcr::{ReferenceReader, SECTION_SIZE_BLOCKS};

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value_t = 128)]
    sample: usize,

    #[arg(long, default_value = "identity")]
    modeler: String,
}

#[derive(Default)]
struct KindTotals {
    streams: u64,
    voxels: u64,
    raw_bytes: u64,
    bytes: u64,
    encode_ns: u128,
    decode_ns: u128,
}

struct SectionStream {
    ctx: SectionContext,
    indices: Vec<u8>,
}

fn nibbles_to_bytes(packed: &[u8], entry_count: usize) -> Vec<u8> {
    let last_bit = entry_count * 4;
    let required_bytes = last_bit.div_ceil(8);
    assert!(
        packed.len() >= required_bytes,
        "packed array of {} bytes cannot hold {} four bit entries",
        packed.len(),
        entry_count
    );
    let mut entries = Vec::with_capacity(entry_count);
    for entry in 0..entry_count {
        let bit = entry * 4;
        let byte = packed[bit / 8];
        entries.push((byte >> (bit % 8)) & 0x0F);
    }
    entries
}

fn extract_four_bit_streams<const N: usize>(
    sections: &[zvcr::region::delta::PackedDeltaData<N>],
    x: u8,
    z: u8,
) -> Vec<SectionStream> {
    let mut streams = Vec::new();
    for (y, section) in sections.iter().enumerate() {
        for snapshot in section.snapshots() {
            let Data::Paletted(paletted) = &snapshot.data.data else {
                continue;
            };
            if paletted.palette.bits_per_entry != 4 {
                continue;
            }
            streams.push(SectionStream {
                ctx: SectionContext {
                    x,
                    y: y as u8,
                    z,
                    palette: paletted.palette.clone(),
                },
                indices: nibbles_to_bytes(&paletted.packed_long_array, N),
            });
        }
    }
    streams
}

fn collect_region_streams(region: &RegionData) -> Vec<SectionStream> {
    let mut blocks = Vec::new();
    for (segment_index, segment) in region.segments.iter().enumerate() {
        let Some(segment) = segment else {
            continue;
        };
        let x = (segment_index / REGION_SIDELENGTH_SEGMENTS) as u8;
        let z = (segment_index % REGION_SIDELENGTH_SEGMENTS) as u8;
        blocks.extend(extract_four_bit_streams(&segment.block_sections, x, z));
    }
    blocks
}

fn bench_kind(
    make_modeler: &(dyn Fn() -> Box<dyn Modeler> + Sync),
    streams: &[SectionStream],
    voxels_per_stream: u64,
    context: &str,
) -> KindTotals {
    if streams.is_empty() {
        return KindTotals::default();
    }

    let encode_start = Instant::now();
    let mut modeler = make_modeler();
    let mut transformed = Vec::with_capacity(streams.len());
    let mut transformed_lens = Vec::with_capacity(streams.len());
    let mut combined_len = 0usize;
    for stream in streams {
        let t = modeler.transform(&stream.ctx, &stream.indices);
        combined_len += t.len();
        transformed_lens.push(t.len());
        transformed.push(t);
    }
    let mut combined = Vec::with_capacity(combined_len);
    for stream in &transformed {
        combined.extend_from_slice(stream);
    }

    let raw_bytes = combined.len() as u64;
    let compressed = compress_zstd_parts(&[&combined], ZSTD_COMPRESSION_LEVEL_DEFAULT)
        .expect("zstd8 compression failed");
    let encode_ns = encode_start.elapsed().as_nanos();

    let decode_start = Instant::now();
    let decoded = decompress_zstd(&compressed).expect("zstd8 decompression failed");
    let mut modeler = make_modeler();
    let mut decoded_offset = 0;
    for (stream_index, (stream, len)) in streams.iter().zip(&transformed_lens).enumerate() {
        let restored_part = decoded
            .get(decoded_offset..decoded_offset + len)
            .unwrap_or_else(|| panic!("{context}: decoded stream is shorter than encoded stream"));
        let restored = modeler.inverse(&stream.ctx, restored_part);
        assert_eq!(
            restored, stream.indices,
            "{context}: stream {stream_index} differs after modeler and zstd8 roundtrip"
        );
        decoded_offset += len;
    }
    let decode_ns = decode_start.elapsed().as_nanos();

    KindTotals {
        streams: streams.len() as u64,
        voxels: streams.len() as u64 * voxels_per_stream,
        raw_bytes,
        bytes: compressed.len() as u64,
        encode_ns,
        decode_ns,
    }
}

fn bench_region(
    make_modeler: &(dyn Fn() -> Box<dyn Modeler> + Sync),
    reader: &ReferenceReader,
    path: &Path,
) -> KindTotals {
    let region = reader
        .read(path)
        .unwrap_or_else(|e| panic!("failed to read region {}: {e}", path.display()));
    let blocks = collect_region_streams(&region);

    let context = path.display().to_string();
    bench_kind(
        make_modeler,
        &blocks,
        SECTION_SIZE_BLOCKS as u64,
        &format!("{context} blocks"),
    )
}

fn format_rate(rate: f64, unit: &str) -> String {
    const K: f64 = 1e3;
    const M: f64 = 1e6;
    const G: f64 = 1e9;
    if rate >= G {
        format!("{:.2} G{}/s", rate / G, unit)
    } else if rate >= M {
        format!("{:.2} M{}/s", rate / M, unit)
    } else if rate >= K {
        format!("{:.2} K{}/s", rate / K, unit)
    } else {
        format!("{rate:.0} {}/s", unit)
    }
}

fn phase_line(label: &str, voxels: u64, ns: u128) -> String {
    let agg_ms = ns as f64 / 1e6;
    if ns == 0 {
        format!("{label} n/a  ({agg_ms:.0} ms aggregate)")
    } else {
        let vps = voxels as f64 / (ns as f64 / 1e9);
        format!(
            "{label} {} ({agg_ms:.0} ms aggregate)",
            format_rate(vps, "voxel")
        )
    }
}

fn main() {
    let cli = Cli::parse();
    let modeler_name = cli.modeler.as_str();
    let make_modeler = find_modeler(cli.modeler.as_str());

    let paths = zvcr::bench::discover::discover(Path::new("test_files"), Some(cli.sample));
    if paths.is_empty() {
        panic!("no region files found");
    }

    println!("modeler: {modeler_name}");
    println!("sampled regions: {}", paths.len());

    let reader = ReferenceReader::new(0);
    let wall_start = Instant::now();
    let per_region: Vec<KindTotals> = paths
        .par_iter()
        .map(|path| bench_region(make_modeler, &reader, path))
        .collect();
    let wall = wall_start.elapsed().as_secs_f64();

    let mut totals = KindTotals::default();
    for outcome in &per_region {
        totals.streams += outcome.streams;
        totals.voxels += outcome.voxels;
        totals.raw_bytes += outcome.raw_bytes;
        totals.bytes += outcome.bytes;
        totals.encode_ns += outcome.encode_ns;
        totals.decode_ns += outcome.decode_ns;
    }

    println!("total streams: {}", totals.streams);
    println!("total voxels: {}", totals.voxels);
    println!("total raw bytes: {}", totals.raw_bytes);
    println!("total zstd8 bytes: {}", totals.bytes);
    println!("wall: {wall:.1} s");
    println!("throughput:");
    println!(
        "{}",
        phase_line(" - encode:", totals.voxels, totals.encode_ns)
    );
    println!(
        "{}",
        phase_line(" - decode:", totals.voxels, totals.decode_ns)
    );
    println!("------------------------");
    make_modeler().print_summary();
    println!("------------------------");
    if totals.voxels == 0 {
        println!("bits per voxel: no four bit palette sections found");
        return;
    }
    let bits_per_voxel = (totals.bytes as f64 * 8.0) / totals.voxels as f64;
    println!("bits per voxel: {bits_per_voxel:.4}");
}
