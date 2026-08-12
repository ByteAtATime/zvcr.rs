use super::{FileResult, Progress};
use crate::definitions::SECTION_SIZE_BLOCKS;
use crate::io::file_location::EXTENSION;
use crate::raw::RegionData;
use crate::region::delta::PackedDeltaData;
use crate::{ExperimentalReader, ExperimentalWriter, Reader, ReferenceReader, Writer};
use std::path::Path;
use std::time::Instant;

fn snapshots_equal<const N: usize>(a: &PackedDeltaData<N>, b: &PackedDeltaData<N>) -> bool {
    let sa = a.snapshots();
    let sb = b.snapshots();
    if sa.len() != sb.len() {
        return false;
    }
    sa.iter()
        .zip(sb.iter())
        .all(|(x, y)| x.timestamp == y.timestamp && x.data.unpack() == y.data.unpack())
}

fn semantically_equal(a: &RegionData, b: &RegionData) -> bool {
    if a.segments.len() != b.segments.len() {
        return false;
    }
    a.segments
        .iter()
        .zip(b.segments.iter())
        .all(|(sa, sb)| match (sa, sb) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                x.block_sections.len() == y.block_sections.len()
                    && x.biome_sections.len() == y.biome_sections.len()
                    && x.block_sections
                        .iter()
                        .zip(y.block_sections.iter())
                        .all(|(p, q)| snapshots_equal(p, q))
                    && x.biome_sections
                        .iter()
                        .zip(y.biome_sections.iter())
                        .all(|(p, q)| snapshots_equal(p, q))
                    && x.states == y.states
                    && x.tile_entities == y.tile_entities
            }
            _ => false,
        })
}

fn count_voxels(data: &RegionData) -> u64 {
    data.segments
        .iter()
        .flatten()
        .map(|s| s.block_sections.len() as u64 * SECTION_SIZE_BLOCKS as u64)
        .sum()
}

pub(super) fn bench_one(
    path: &Path,
    ref_reader: &ReferenceReader,
    exp_writer: &ExperimentalWriter,
    exp_reader: &ExperimentalReader,
    progress: &Progress,
    verify: bool,
) -> FileResult {
    let t0 = Instant::now();
    let file_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return finish(
                FileResult {
                    path: path.to_path_buf(),
                    ok: false,
                    step: "reference read",
                    error: Some(format!("Failed to read file from disk: {e}")),
                    integrity_ok: None,
                    original_bytes: 0,
                    original_raw_bytes: 0,
                    encoded_bytes: 0,
                    encoded_raw_bytes: 0,
                    voxels: 0,
                    ref_read_ns: 0,
                    exp_write_ns: 0,
                    exp_read_ns: 0,
                },
                true,
                progress,
            );
        }
    };
    let ref_data = match ref_reader.from_bytes(&file_bytes) {
        Ok(data) => data,
        Err(e) => {
            let original_bytes = file_bytes.len() as u64;
            return finish(
                FileResult {
                    path: path.to_path_buf(),
                    ok: false,
                    step: "reference read",
                    error: Some(e),
                    integrity_ok: None,
                    original_bytes,
                    original_raw_bytes: uncompressed_file_size(&file_bytes),
                    encoded_bytes: 0,
                    encoded_raw_bytes: 0,
                    voxels: 0,
                    ref_read_ns: 0,
                    exp_write_ns: 0,
                    exp_read_ns: 0,
                },
                true,
                progress,
            );
        }
    };
    let ref_read_ns = t0.elapsed().as_nanos();

    let original_bytes = file_bytes.len() as u64;
    let original_raw_bytes = uncompressed_file_size(&file_bytes);
    let voxels = count_voxels(&ref_data);

    let t1 = Instant::now();
    let encoded = match exp_writer.to_bytes(&ref_data) {
        Ok(bytes) => bytes,
        Err(e) => {
            return finish(
                FileResult {
                    path: path.to_path_buf(),
                    ok: false,
                    step: "experimental write",
                    error: Some(e),
                    integrity_ok: None,
                    original_bytes,
                    original_raw_bytes,
                    encoded_bytes: 0,
                    encoded_raw_bytes: 0,
                    voxels,
                    ref_read_ns,
                    exp_write_ns: 0,
                    exp_read_ns: 0,
                },
                true,
                progress,
            );
        }
    };
    let exp_write_ns = t1.elapsed().as_nanos();

    let encoded_bytes = encoded.len() as u64;
    let encoded_raw_bytes = uncompressed_file_size(&encoded);

    let t2 = Instant::now();
    let exp_data = match exp_reader.from_bytes(&encoded) {
        Ok(data) => data,
        Err(e) => {
            return finish(
                FileResult {
                    path: path.to_path_buf(),
                    ok: false,
                    step: "experimental decode",
                    error: Some(e),
                    integrity_ok: None,
                    original_bytes,
                    original_raw_bytes,
                    encoded_bytes,
                    encoded_raw_bytes,
                    voxels,
                    ref_read_ns,
                    exp_write_ns,
                    exp_read_ns: 0,
                },
                true,
                progress,
            );
        }
    };
    let exp_read_ns = t2.elapsed().as_nanos();

    if !verify {
        return finish(
            FileResult {
                path: path.to_path_buf(),
                ok: true,
                step: "decode",
                error: None,
                integrity_ok: None,
                original_bytes,
                original_raw_bytes,
                encoded_bytes,
                encoded_raw_bytes,
                voxels,
                ref_read_ns,
                exp_write_ns,
                exp_read_ns,
            },
            false,
            progress,
        );
    }

    let integrity_ok = Some(semantically_equal(&ref_data, &exp_data));
    let error = if integrity_ok == Some(true) {
        None
    } else {
        Some("decoded data does not match reference".to_string())
    };

    finish(
        FileResult {
            path: path.to_path_buf(),
            ok: integrity_ok == Some(true),
            step: "integrity",
            error,
            integrity_ok,
            original_bytes,
            original_raw_bytes,
            encoded_bytes,
            encoded_raw_bytes,
            voxels,
            ref_read_ns,
            exp_write_ns,
            exp_read_ns,
        },
        integrity_ok != Some(true),
        progress,
    )
}

fn uncompressed_file_size(bytes: &[u8]) -> u64 {
    const HEADER: usize = EXTENSION.len() + 4;
    let Some(body) = bytes.get(HEADER..) else {
        return bytes.len() as u64;
    };
    match zstd::zstd_safe::get_frame_content_size(body) {
        Ok(Some(n)) => n + HEADER as u64,
        _ => bytes.len() as u64,
    }
}

fn finish(result: FileResult, failed: bool, progress: &Progress) -> FileResult {
    if failed {
        progress.inc_failed();
    }
    progress.inc_done();
    progress.add_bytes(result.original_bytes);
    progress.add_voxels(result.voxels);
    result
}
