mod discover;
mod format;
mod report;
mod worker;

use crate::{
    ExperimentalReader, ExperimentalWriter, ReferenceReader, ZSTD_COMPRESSION_LEVEL_DEFAULT,
};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

struct FileResult {
    path: PathBuf,
    ok: bool,
    step: &'static str,
    error: Option<String>,
    integrity_ok: Option<bool>,
    original_bytes: u64,
    encoded_bytes: u64,
    original_raw_bytes: u64,
    encoded_raw_bytes: u64,
    ref_read_ns: u128,
    exp_write_ns: u128,
    exp_read_ns: u128,
}

struct Progress {
    done: AtomicU64,
    failed: AtomicU64,
    bytes_in: AtomicU64,
    total: u64,
    start: Instant,
}

impl Progress {
    fn inc_done(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    fn add_bytes(&self, n: u64) {
        self.bytes_in.fetch_add(n, Ordering::Relaxed);
    }
}

pub fn run(root: &Path, verify: bool, sample: Option<usize>) {
    let paths = discover::discover(root, sample);
    if paths.is_empty() {
        eprintln!("No region files found in {}", root.display());
        return;
    }

    let total = paths.len() as u64;

    println!("Discovered {total} region files");

    let ref_arc = Arc::new(ReferenceReader::new(0));
    let exp_w_arc = Arc::new(ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT));
    let exp_r_arc = Arc::new(ExperimentalReader::new(0));

    let progress = Arc::new(Progress {
        done: AtomicU64::new(0),
        failed: AtomicU64::new(0),
        bytes_in: AtomicU64::new(0),
        total,
        start: Instant::now(),
    });

    let stop = Arc::new(AtomicBool::new(false));

    let reporter_progress = Arc::clone(&progress);
    let reporter_stop = Arc::clone(&stop);
    let reporter_handle =
        std::thread::spawn(move || report::reporter(reporter_progress, reporter_stop));

    let ref_arc = Arc::clone(&ref_arc);
    let exp_w_arc = Arc::clone(&exp_w_arc);
    let exp_r_arc = Arc::clone(&exp_r_arc);

    let results: Vec<FileResult> = paths
        .par_iter()
        .map(|p| worker::bench_one(p, &ref_arc, &exp_w_arc, &exp_r_arc, &progress, verify))
        .collect();

    stop.store(true, Ordering::Relaxed);
    reporter_handle.join().unwrap();
    println!();

    report::print_summary(&results, &progress, verify);
}
