mod fixtures;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use zvcr::definitions::{SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use zvcr::{
    ExperimentalReader, ExperimentalWriter, Reader, Writer, ZSTD_COMPRESSION_LEVEL_DEFAULT,
};
use zvcr::raw::RegionData;

const KEPT_SEGMENTS: usize = 8;

fn load_region() -> (PathBuf, RegionData, usize) {
    let path = fixtures::discover_region_file("ZVCR_MODEL_BENCH_FILE");
    trim_to_bench_region(&path).unwrap_or_else(|e| panic!("{e}"))
}

fn trim_to_bench_region(path: &Path) -> Result<(PathBuf, RegionData, usize), String> {
    let mut data = fixtures::decode_region(path)?;
    let mut kept = 0usize;
    let mut voxels = 0usize;
    for slot in data.segments.iter_mut() {
        let Some(segment) = slot else {
            continue;
        };
        if kept == KEPT_SEGMENTS {
            *slot = None;
            continue;
        }
        kept += 1;
        for section in &segment.block_sections {
            if !section.snapshots().is_empty() {
                voxels += SECTION_SIZE_BLOCKS;
            }
        }
    }
    if kept == 0 || voxels == 0 {
        return Err(format!("no voxel data found in {path:?}"));
    }
    Ok((path.to_path_buf(), data, voxels))
}

fn bench_encode(c: &mut Criterion, data: &RegionData, voxels: usize) {
    let writer = ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT);
    let mut group = c.benchmark_group("model");
    group.throughput(Throughput::Elements(voxels as u64));
    group.sample_size(20);
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
    group.sample_size(20);
    group.bench_function("decode_region", |b| {
        b.iter(|| black_box(reader.from_bytes(&encoded).unwrap()));
    });
    group.finish();
}

fn bench_model(c: &mut Criterion) {
    let (path, data, voxels) = load_region();
    eprintln!(
        "model bench data: {} voxels from {} (kept {KEPT_SEGMENTS} of {SEGMENTS_PER_REGION} segments)",
        voxels,
        path.display()
    );
    bench_encode(c, &data, voxels);
    bench_decode(c, &data, voxels);
}

criterion_group!(benches, bench_model);
criterion_main!(benches);
