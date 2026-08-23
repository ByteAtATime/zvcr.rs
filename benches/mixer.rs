use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::fs;
use std::path::{Path, PathBuf};
use zvcr::{Reader, ReferenceReader};
use zvcr::io::serialize::experimental::mixer;

const COUNTER_TABLE_BITS: usize = 12;
const COUNTER_TABLE_SIZE: usize = 1 << COUNTER_TABLE_BITS;
const COUNTER_TABLE_MASK: usize = COUNTER_TABLE_SIZE - 1;
const PRIMARY_SEED_WEIGHT: i32 = mixer::PRIMARY_MIXER_SEED_WEIGHT;
const TREE_SEED_WEIGHT: i32 = mixer::TREE_MIXER_SEED_WEIGHT;
const TREE_ROWS: usize = mixer::MAX_BIT_DEPTH + 1;
const MAX_BENCH_VOXELS: usize = 1 << 20;
const WEIGHT_ROWS: usize = TREE_ROWS * mixer::CONF_BUCKETS * 8;
const PROB_HALF: u16 = mixer::PROB_HALF;
const PROB_MAX: i32 = mixer::PROB_MAX;

fn collect_region_files(dir: &Path, out: &mut Vec<(u64, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_region_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "zvcr3d") {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            out.push((len, path));
        }
    }
}

fn load_voxels() -> (PathBuf, Vec<u16>) {
    if let Ok(path) = std::env::var("ZVCR_MIXER_BENCH_FILE") {
        let path = PathBuf::from(path);
        return voxels_from_file(&path).unwrap_or_else(|e| panic!("{e}"));
    }
    let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
    collect_region_files(Path::new("test_files"), &mut candidates);
    assert!(!candidates.is_empty(), "no .zvcr3d files under test_files");
    candidates.sort_unstable();
    candidates
        .iter()
        .find_map(|(_, path)| voxels_from_file(path).ok())
        .unwrap_or_else(|| panic!("no decodable .zvcr3d files under test_files"))
}

fn voxels_from_file(path: &Path) -> Result<(PathBuf, Vec<u16>), String> {
    let bytes = fs::read(path)
        .map_err(|e| format!("failed to read {path:?}: {e}"))?;
    let data = ReferenceReader::new(0)
        .from_bytes(&bytes)
        .map_err(|e| format!("failed to decode {path:?}: {e}"))?;
    let mut voxels = Vec::new();
    for segment in &data.segments {
        let Some(segment) = segment else {
            continue;
        };
        for section in &segment.block_sections {
            let snapshots = section.snapshots();
            if snapshots.is_empty() {
                continue;
            }
            voxels.extend_from_slice(&snapshots[0].data.unpack());
        }
    }
    if voxels.is_empty() {
        return Err(format!("no voxel data found in {path:?}"));
    }
    voxels.truncate(MAX_BENCH_VOXELS);
    Ok((path.to_path_buf(), voxels))
}

fn counter_slot(value: u16, input: usize) -> usize {
    (value as usize).wrapping_mul(input + 1) & COUNTER_TABLE_MASK
}

fn bench_primary_mix(c: &mut Criterion, voxels: &[u16]) {
    let mut group = c.benchmark_group("mixer");
    group.throughput(Throughput::Elements(voxels.len() as u64));
    group.bench_function("primary_mix_cycle", |b| {
        let mut counters = vec![[PROB_HALF; COUNTER_TABLE_SIZE]; mixer::MIX_INPUTS];
        let mut weights = vec![[PRIMARY_SEED_WEIGHT; mixer::MIX_INPUTS]; WEIGHT_ROWS];
        b.iter(|| {
            let mut prev = voxels[0];
            let mut checksum = 0u64;
            for &v in voxels.iter() {
                let bit = (v == prev) as u32;
                prev = v;
                let mut probs = [0u32; mixer::MIX_INPUTS];
                for (k, counter_row) in counters.iter().enumerate() {
                    probs[k] = counter_row[counter_slot(v, k)] as u32;
                }
                let stretched = mixer::stretch_probs(black_box(&probs));
                let row = (v as usize) % WEIGHT_ROWS;
                let mixed = mixer::mix_stretched(&weights[row], &stretched);
                let target = if bit != 0 { PROB_MAX } else { 0 };
                for k in 0..mixer::MIX_INPUTS {
                    let slot = counter_slot(v, k);
                    let current = counters[k][slot] as i32;
                    counters[k][slot] = (current + ((target - current) >> mixer::HEAD_ADAPT_SHIFT))
                        as u16;
                }
                mixer::adapt_weights_stretched(&mut weights[row], &stretched, bit, mixed);
                checksum = checksum.wrapping_add(mixed as u64);
            }
            black_box(checksum);
        });
    });
    group.finish();
}

fn bench_tree_mix(c: &mut Criterion, voxels: &[u16]) {
    let mut group = c.benchmark_group("mixer");
    group.throughput(Throughput::Elements(voxels.len() as u64));
    group.bench_function("tree_mix_cycle", |b| {
        let mut spatial = [PROB_HALF; COUNTER_TABLE_SIZE];
        let mut bitpos = [PROB_HALF; COUNTER_TABLE_SIZE];
        let mut band = [PROB_HALF; COUNTER_TABLE_SIZE];
        let mut weights = vec![[TREE_SEED_WEIGHT; mixer::TREE_INPUTS]; TREE_ROWS];
        b.iter(|| {
            let mut prev = voxels[0];
            let mut checksum = 0u64;
            for &v in voxels.iter() {
                let bit = (v == prev) as u32;
                prev = v;
                let probs = [
                    spatial[(v as usize) & COUNTER_TABLE_MASK] as u32,
                    bitpos[((v >> 4) as usize) & COUNTER_TABLE_MASK] as u32,
                    band[((v >> 8) as usize) & COUNTER_TABLE_MASK] as u32,
                ];
                let row = (v as usize) % TREE_ROWS;
                let mixed = mixer::mix_logits(&weights[row], black_box(&probs));
                let target = if bit != 0 { PROB_MAX } else { 0 };
                for (table, slot) in [
                    (&mut spatial, (v as usize) & COUNTER_TABLE_MASK),
                    (&mut bitpos, ((v >> 4) as usize) & COUNTER_TABLE_MASK),
                    (&mut band, ((v >> 8) as usize) & COUNTER_TABLE_MASK),
                ] {
                    let current = table[slot] as i32;
                    table[slot] = (current + ((target - current) >> mixer::ADAPT_RATE_SHIFT)) as u16;
                }
                mixer::adapt_weights(&mut weights[row], &probs, bit, mixed);
                checksum = checksum.wrapping_add(mixed as u64);
            }
            black_box(checksum);
        });
    });
    group.finish();
}

fn bench_mixer(c: &mut Criterion) {
    let (path, voxels) = load_voxels();
    eprintln!("mixer bench data: {} voxels from {}", voxels.len(), path.display());
    bench_primary_mix(c, &voxels);
    bench_tree_mix(c, &voxels);
}

criterion_group!(benches, bench_mixer);
criterion_main!(benches);
