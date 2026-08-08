use crate::{
    ExperimentalReader, ExperimentalWriter, Reader, ReferenceReader, Writer,
    ZSTD_COMPRESSION_LEVEL_DEFAULT,
};
use rayon::prelude::*;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct FileResult {
    path: PathBuf,
    ok: bool,
    step: &'static str,
    error: Option<String>,
    integrity_ok: bool,
    original_bytes: u64,
    encoded_bytes: u64,
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

pub fn run(root: &Path) {
    let paths = discover(root);
    if paths.is_empty() {
        eprintln!("No region files found in {}", root.display());
        return;
    }

    let total = paths.len() as u64;

    println!("Discovered {total} region files");

    let ref_arc = Arc::new(ReferenceReader::new(0));
    let exp_w_arc = Arc::new(ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT, 1));
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
    let reporter_handle = std::thread::spawn(move || reporter(reporter_progress, reporter_stop));

    let ref_arc = Arc::clone(&ref_arc);
    let exp_w_arc = Arc::clone(&exp_w_arc);
    let exp_r_arc = Arc::clone(&exp_r_arc);

    let results: Vec<FileResult> = paths
        .par_iter()
        .map(|p| bench_one(p, &ref_arc, &exp_w_arc, &exp_r_arc, &progress))
        .collect();

    stop.store(true, Ordering::Relaxed);
    reporter_handle.join().unwrap();
    println!();

    print_summary(&results, &progress);
}

fn discover(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension() == Some(OsStr::new("zvcr3d")) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(dur: Duration) -> String {
    let secs = dur.as_secs();
    if secs >= 60 {
        format!("{:02}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{:.2}s", dur.as_secs_f64())
    }
}

fn reporter(progress: Arc<Progress>, stop: Arc<AtomicBool>) {
    let mut stdout = std::io::stdout();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let done = progress.done.load(Ordering::Relaxed);
        let failed = progress.failed.load(Ordering::Relaxed);
        let bytes_in = progress.bytes_in.load(Ordering::Relaxed);
        let total = progress.total;
        let elapsed = progress.start.elapsed().as_secs_f64();
        let mbps = if elapsed > 0.0 {
            bytes_in as f64 / 1e6 / elapsed
        } else {
            0.0
        };
        let eta = if done > 0 {
            (elapsed / done as f64) * (total - done) as f64
        } else {
            0.
        };
        write!(
            stdout,
            "\r\x1B[K[{done}/{total}] failed={failed} | {} processed | {mbps:.1} MB/s | ETA: {}",
            format_bytes(bytes_in),
            format_duration(Duration::from_secs_f64(eta))
        )
        .ok();
        stdout.flush().ok();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn bench_one(
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

fn print_summary(results: &[FileResult], progress: &Progress) {
    let total = results.len();
    let ok = results.iter().filter(|r| r.ok).count();
    let failed = results.iter().filter(|r| !r.ok).count();
    let integrity_ok = results.iter().filter(|r| r.integrity_ok).count();
    let input_bytes: u64 = results.iter().map(|r| r.original_bytes).sum();
    let output_bytes: u64 = results.iter().map(|r| r.encoded_bytes).sum();
    let wall = progress.start.elapsed().as_secs_f64();

    let ref_read_ns: u128 = results.iter().map(|r| r.ref_read_ns).sum();
    let exp_write_ns: u128 = results.iter().map(|r| r.exp_write_ns).sum();
    let exp_read_ns: u128 = results.iter().map(|r| r.exp_read_ns).sum();

    println!("\n\n================================================================");
    println!("Files     : {total} processed, {ok} ok, {failed} failed");
    println!("Integrity : {integrity_ok} / {total}");
    println!("Input     : {} ({input_bytes})", format_bytes(input_bytes));
    println!(
        "Output    : {} ({output_bytes})",
        format_bytes(output_bytes)
    );
    if input_bytes > 0 {
        let pct = output_bytes as f64 / input_bytes as f64 * 100.0;
        let ratio = if output_bytes > 0 {
            format!("{:.2}:1", input_bytes as f64 / output_bytes as f64)
        } else {
            "n/a".to_string()
        };
        println!("Ratio     : {pct:.2}%  ({ratio})");
    }
    println!("Time      : {wall:.1} s");
    println!("Throughput:");
    println!(
        "{}",
        phase_line(" - reference read    :", input_bytes, ref_read_ns)
    );
    println!(
        "{}",
        phase_line(" - experimental write:", input_bytes, exp_write_ns)
    );
    println!(
        "{}",
        phase_line(" - experimental read :", input_bytes, exp_read_ns)
    );

    if failed > 0 {
        println!("Failures:");
        for result in results.iter().filter(|r| !r.ok).take(10) {
            let message = result.error.as_deref().unwrap_or("unknown");
            println!(
                "{} : {} failed: {}",
                result.path.display(),
                result.step,
                message
            );
        }
    }
}

fn phase_line(label: &str, input_bytes: u64, ns: u128) -> String {
    let agg_ms = ns as f64 / 1e6;
    if ns == 0 {
        format!("{label} n/a  ({agg_ms:.0} ms aggregate)")
    } else {
        let mbps = (input_bytes as f64 / (ns as f64 / 1e9)) / 1e6;
        format!("{label} {mbps:>5.1} MB/s ({agg_ms:.0} ms aggregate)")
    }
}
