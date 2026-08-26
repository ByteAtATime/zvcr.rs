mod fixtures;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::PathBuf;
use zvcr::definitions::{
    REGION_SIDELENGTH_SEGMENTS, SEGMENT_SIDELENGTH_BLOCKS,
};
use zvcr::{
    ExperimentalReader, ExperimentalWriter, Reader, Writer, ZSTD_COMPRESSION_LEVEL_DEFAULT,
};
use zvcr::raw::RegionData;

const GRID_SIDE: usize = REGION_SIDELENGTH_SEGMENTS * SEGMENT_SIDELENGTH_BLOCKS;

fn grid_voxels(data: &RegionData) -> usize {
    GRID_SIDE * GRID_SIDE * data.dimension.section_count() * SEGMENT_SIDELENGTH_BLOCKS
}

fn load_region() -> (PathBuf, RegionData) {
    let path = fixtures::discover_region_file("ZVCR_MODEL_BENCH_FILE");
    let data = fixtures::decode_region(&path).unwrap_or_else(|e| panic!("{e}"));
    (path, data)
}

fn bench_encode(c: &mut Criterion, data: &RegionData, voxels: usize) {
    let writer = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT);
    let mut group = c.benchmark_group("model");
    group.throughput(Throughput::Elements(voxels as u64));
    group.sample_size(10);
    group.bench_function("encode_region", |b| {
        b.iter(|| black_box(writer.to_bytes(data).unwrap()));
    });
    group.finish();
}

fn bench_decode(c: &mut Criterion, data: &RegionData, voxels: usize) {
    let writer = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT);
    let encoded = writer.to_bytes(data).unwrap();
    let reader = ExperimentalReader::new();
    let mut group = c.benchmark_group("model");
    group.throughput(Throughput::Elements(voxels as u64));
    group.sample_size(10);
    group.bench_function("decode_region", |b| {
        b.iter(|| black_box(reader.from_bytes(&encoded).unwrap()));
    });
    group.finish();
}

fn bench_model(c: &mut Criterion) {
    let (path, data) = load_region();
    let voxels = grid_voxels(&data);
    eprintln!(
        "model bench data: {} grid voxels from {}",
        voxels,
        path.display()
    );
    bench_encode(c, &data, voxels);
    bench_decode(c, &data, voxels);
}

criterion_group!(benches, bench_model);
criterion_main!(benches);
