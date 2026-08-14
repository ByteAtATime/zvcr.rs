mod models;

use clap::Parser;
use rayon::prelude::*;
use std::path::Path;
use std::time::Instant;

use models::{Modeler, find_modeler};
use zvcr::Reader;
use zvcr::io::compression::{ZSTD_COMPRESSION_LEVEL_DEFAULT, compress_zstd_parts, decompress_zstd};
use zvcr::io::serialize::experimental::coders::rans::{
    RansDecoder, RansEncoder, build_decode_table, build_freq_table,
};
use zvcr::raw::RegionData;
use zvcr::region::packed_data::Data;
use zvcr::{ReferenceReader, SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS};

const RANS_FREQ_TOTAL: u64 = 4096;

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
    rans_bytes: u64,
    bytes: u64,
    encode_ns: u128,
    decode_ns: u128,
}

impl KindTotals {
    fn bits_per_voxel(&self) -> f64 {
        if self.voxels == 0 {
            return 0.0;
        }
        (self.bytes as f64 * 8.0) / self.voxels as f64
    }
}

#[derive(Default)]
struct RegionOutcome {
    block: KindTotals,
    biome: KindTotals,
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

fn extract_four_bit_index_streams<const N: usize>(
    sections: &[zvcr::region::delta::PackedDeltaData<N>],
) -> Vec<Vec<u8>> {
    let mut streams = Vec::new();
    for section in sections {
        for snapshot in section.snapshots() {
            let Data::Paletted(paletted) = &snapshot.data.data else {
                continue;
            };
            if paletted.palette.bits_per_entry != 4 {
                continue;
            }
            streams.push(nibbles_to_bytes(&paletted.packed_long_array, N));
        }
    }
    streams
}

fn collect_region_streams(region: &RegionData) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut blocks = Vec::new();
    let mut biomes = Vec::new();
    for segment in region.segments.iter().flatten() {
        blocks.extend(extract_four_bit_index_streams(&segment.block_sections));
        biomes.extend(extract_four_bit_index_streams(&segment.biome_sections));
    }
    (blocks, biomes)
}

fn serialize_freq_table(freq: &[u16; 256]) -> Vec<u8> {
    let mut side = vec![0u8; 32];
    for (symbol, &frequency) in freq.iter().enumerate() {
        if frequency > 0 {
            side[symbol / 8] |= 1 << (symbol % 8);
        }
    }
    for &frequency in freq.iter().filter(|&frequency| *frequency > 0) {
        side.extend_from_slice(&frequency.to_le_bytes());
    }
    side
}

fn deserialize_freq_table(side: &[u8]) -> [u16; 256] {
    let bitmap = side
        .get(..32)
        .expect("freq table truncated: missing symbol bitmap");
    let present = bitmap.iter().map(|byte| byte.count_ones()).sum::<u32>() as usize;
    let payload = side
        .get(32..32 + present * 2)
        .expect("freq table truncated: missing frequencies");

    let mut freq = [0u16; 256];
    let mut payload_index = 0;
    for symbol in 0..256 {
        if bitmap[symbol / 8] & (1 << (symbol % 8)) == 0 {
            continue;
        }
        freq[symbol] = u16::from_le_bytes([payload[payload_index], payload[payload_index + 1]]);
        payload_index += 2;
    }
    let total: u64 = freq.iter().map(|&frequency| frequency as u64).sum();
    assert_eq!(
        total, RANS_FREQ_TOTAL,
        "freq table sums to {total} instead of the rANS precision total"
    );
    freq
}

fn starts_from_freq(freq: &[u16; 256]) -> [u16; 256] {
    let mut starts = [0u16; 256];
    let mut cumulative = 0u16;
    for symbol in 0..256 {
        starts[symbol] = cumulative;
        cumulative += freq[symbol];
    }
    starts
}

fn rans_encode_stream(symbols: &[u8]) -> Vec<u8> {
    let (freq, start) = build_freq_table(symbols);
    let mut encoder = RansEncoder::with_capacity(symbols.len());
    for &symbol in symbols.iter().rev() {
        encoder.put(freq[symbol as usize] as u32, start[symbol as usize] as u32);
    }
    encoder.finish()
}

fn rans_decode_stream(body: &[u8], symbol_count: usize, side: &[u8]) -> Vec<u8> {
    let freq = deserialize_freq_table(side);
    let start = starts_from_freq(&freq);
    let table = build_decode_table(&freq, &start);

    let mut decoder = RansDecoder::new(body);
    let mut symbols = Vec::with_capacity(symbol_count);
    for _ in 0..symbol_count {
        let entry = table[decoder.slot() as usize];
        symbols.push(entry.sym);
        decoder.advance(entry.freq as u32, entry.start as u32);
    }
    symbols
}

fn bench_kind(
    make_modeler: &(dyn Fn() -> Box<dyn Modeler> + Sync),
    streams: &[Vec<u8>],
    voxels_per_stream: u64,
    context: &str,
) -> KindTotals {
    if streams.is_empty() {
        return KindTotals::default();
    }

    let encode_start = Instant::now();
    let mut modeler = make_modeler();
    let mut transformed = Vec::with_capacity(streams.len());
    let mut combined_len = 0usize;
    for stream in streams {
        let len = modeler.transformed_len(stream.len());
        let t = modeler.transform(stream);
        assert_eq!(
            t.len(),
            len,
            "{context}: transformed stream length disagrees with transformed_len"
        );
        combined_len += t.len();
        transformed.push(t);
    }
    let mut combined = Vec::with_capacity(combined_len);
    for stream in &transformed {
        combined.extend_from_slice(stream);
    }

    let (freq, _) = build_freq_table(&combined);
    let side = serialize_freq_table(&freq);
    let body = rans_encode_stream(&combined);
    let rans_bytes = side.len() + body.len();

    let compressed = compress_zstd_parts(&[&side, &body], ZSTD_COMPRESSION_LEVEL_DEFAULT)
        .expect("zstd8 compression failed");
    let encode_ns = encode_start.elapsed().as_nanos();

    let decode_start = Instant::now();
    let decompressed = decompress_zstd(&compressed).expect("zstd8 decompression failed");
    let (restored_side, restored_body) = decompressed.split_at(side.len());

    let decoded = rans_decode_stream(restored_body, combined_len, restored_side);
    let mut modeler = make_modeler();
    let mut decoded_offset = 0;
    for (stream_index, stream) in streams.iter().enumerate() {
        let len = modeler.transformed_len(stream.len());
        let restored_part = decoded
            .get(decoded_offset..decoded_offset + len)
            .unwrap_or_else(|| panic!("{context}: decoded stream is shorter than encoded stream"));
        let restored = modeler.inverse(restored_part);
        assert_eq!(
            restored, *stream,
            "{context}: stream {stream_index} differs after modeler, rANS, and zstd8 roundtrip"
        );
        decoded_offset += len;
    }
    let decode_ns = decode_start.elapsed().as_nanos();

    KindTotals {
        streams: streams.len() as u64,
        voxels: streams.len() as u64 * voxels_per_stream,
        rans_bytes: rans_bytes as u64,
        bytes: compressed.len() as u64,
        encode_ns,
        decode_ns,
    }
}

fn bench_region(
    make_modeler: &(dyn Fn() -> Box<dyn Modeler> + Sync),
    reader: &ReferenceReader,
    path: &Path,
) -> RegionOutcome {
    let region = reader
        .read(path)
        .unwrap_or_else(|e| panic!("failed to read region {}: {e}", path.display()));
    let (blocks, biomes) = collect_region_streams(&region);

    let context = path.display().to_string();
    RegionOutcome {
        block: bench_kind(
            make_modeler,
            &blocks,
            SECTION_SIZE_BLOCKS as u64,
            &format!("{context} blocks"),
        ),
        biome: bench_kind(
            make_modeler,
            &biomes,
            SECTION_SIZE_BIOMES as u64,
            &format!("{context} biomes"),
        ),
    }
}

fn print_kind_totals(label: &str, totals: &KindTotals) {
    println!(
        "{label}: {} streams, {} voxels, {} rans bytes, {} zstd8 bytes, {:.4} bits per voxel",
        totals.streams,
        totals.voxels,
        totals.rans_bytes,
        totals.bytes,
        totals.bits_per_voxel()
    );
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
    let outcomes: Vec<RegionOutcome> = paths
        .par_iter()
        .map(|path| bench_region(make_modeler, &reader, path))
        .collect();
    let wall = wall_start.elapsed().as_secs_f64();

    let mut block = KindTotals::default();
    let mut biome = KindTotals::default();
    for outcome in &outcomes {
        block.streams += outcome.block.streams;
        block.voxels += outcome.block.voxels;
        block.rans_bytes += outcome.block.rans_bytes;
        block.bytes += outcome.block.bytes;
        block.encode_ns += outcome.block.encode_ns;
        block.decode_ns += outcome.block.decode_ns;
        biome.streams += outcome.biome.streams;
        biome.voxels += outcome.biome.voxels;
        biome.rans_bytes += outcome.biome.rans_bytes;
        biome.bytes += outcome.biome.bytes;
        biome.encode_ns += outcome.biome.encode_ns;
        biome.decode_ns += outcome.biome.decode_ns;
    }

    print_kind_totals("blocks", &block);
    print_kind_totals("biomes", &biome);

    let total_rans_bytes = block.rans_bytes + biome.rans_bytes;
    let total_bytes = block.bytes + biome.bytes;
    let total_voxels = block.voxels + biome.voxels;
    let total_encode_ns = block.encode_ns + biome.encode_ns;
    let total_decode_ns = block.decode_ns + biome.decode_ns;
    println!("total rans bytes: {total_rans_bytes}");
    println!("total zstd8 bytes: {total_bytes}");
    println!("wall: {wall:.1} s");
    println!("throughput:");
    println!(
        "{}",
        phase_line(" - encode:", total_voxels, total_encode_ns)
    );
    println!(
        "{}",
        phase_line(" - decode:", total_voxels, total_decode_ns)
    );
    println!("------------------------");
    make_modeler().print_summary();
    println!("------------------------");
    if total_voxels == 0 {
        println!("bits per voxel: no four bit palette sections found");
        return;
    }
    let bits_per_voxel = (total_bytes as f64 * 8.0) / total_voxels as f64;
    println!("bits per voxel: {bits_per_voxel:.4}");
}
