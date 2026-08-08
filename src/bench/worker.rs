use super::{FileResult, Progress};
use crate::{
    ExperimentalReader, ExperimentalWriter, Reader, ReferenceReader, Writer,
};
use std::path::Path;
use std::time::Instant;

pub(super) fn bench_one(
    path: &Path,
    ref_reader: &ReferenceReader,
    exp_writer: &ExperimentalWriter,
    exp_reader: &ExperimentalReader,
    progress: &Progress,
) -> FileResult {
    let t0 = Instant::now();
    let ref_data = match ref_reader.read(path) {
        Ok(data) => data,
        Err(e) => {
            return finish(
                FileResult {
                    path: path.to_path_buf(),
                    ok: false,
                    step: "reference read",
                    error: Some(e),
                    integrity_ok: false,
                    original_bytes: 0,
                    encoded_bytes: 0,
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

    let original_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

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
                    integrity_ok: false,
                    original_bytes,
                    encoded_bytes: 0,
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
                    integrity_ok: false,
                    original_bytes,
                    encoded_bytes,
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

    let integrity_ok = ref_data == exp_data;
    let error = if integrity_ok {
        None
    } else {
        Some("decoded data does not match reference".to_string())
    };

    finish(
        FileResult {
            path: path.to_path_buf(),
            ok: integrity_ok,
            step: "integrity",
            error,
            integrity_ok,
            original_bytes,
            encoded_bytes,
            ref_read_ns,
            exp_write_ns,
            exp_read_ns,
        },
        !integrity_ok,
        progress,
    )
}

fn finish(result: FileResult, failed: bool, progress: &Progress) -> FileResult {
    if failed {
        progress.inc_failed();
    }
    progress.inc_done();
    progress.add_bytes(result.original_bytes);
    result
}
