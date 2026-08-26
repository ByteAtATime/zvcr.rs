use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Default)]
pub struct RunStats {
    bytes_in_total: AtomicU64,
    bytes_out_total: AtomicU64,
    failed_count: AtomicUsize,
    skipped_count: AtomicUsize,
}

impl RunStats {
    pub fn record_written(&self, bytes_in: u64, bytes_out: u64) {
        self.bytes_in_total.fetch_add(bytes_in, Ordering::Relaxed);
        self.bytes_out_total.fetch_add(bytes_out, Ordering::Relaxed);
    }

    pub fn record_skip(&self) {
        self.skipped_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fail(&self) {
        self.failed_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn bytes_in(&self) -> u64 {
        self.bytes_in_total.load(Ordering::Relaxed)
    }

    pub fn bytes_out(&self) -> u64 {
        self.bytes_out_total.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed_count.load(Ordering::Relaxed)
    }

    pub fn skipped(&self) -> usize {
        self.skipped_count.load(Ordering::Relaxed)
    }
}

pub struct Written {
    pub bytes_in: u64,
    pub bytes_out: u64,
}

pub enum Outcome {
    Written(Written),
    Skipped,
    Failed(String),
}

pub fn run_all(
    verb: &str,
    noun: &str,
    sources: &[PathBuf],
    work: impl Fn(&Path) -> Outcome + Send + Sync,
) -> ExitCode {
    let total = sources.len() as u64;
    let pb = progress_bar(total);
    let start = Instant::now();
    let stats = RunStats::default();

    let failures: Vec<(PathBuf, Option<String>)> = sources
        .par_iter()
        .progress_with(pb.clone())
        .map(|path| {
            let outcome = work(path);
            match &outcome {
                Outcome::Written(w) => stats.record_written(w.bytes_in, w.bytes_out),
                Outcome::Skipped => stats.record_skip(),
                Outcome::Failed(_) => stats.record_fail(),
            }
            update_message(&pb, &start, &stats);
            let message = match outcome {
                Outcome::Failed(message) => Some(message),
                _ => None,
            };
            (path.to_path_buf(), message)
        })
        .collect();

    report(verb, noun, &pb, start, &stats, total, &failures)
}

fn report(
    verb: &str,
    noun: &str,
    pb: &ProgressBar,
    start: Instant,
    stats: &RunStats,
    total: u64,
    failures: &[(PathBuf, Option<String>)],
) -> ExitCode {
    pb.finish_with_message(format!("{noun} complete"));

    let failed = stats.failed();
    let skipped = stats.skipped();
    let ok = total as usize - failed - skipped;
    let input_bytes = stats.bytes_in();
    let output_bytes = stats.bytes_out();
    let wall = start.elapsed().as_secs_f64();
    let rate = if wall > 0.0 {
        (input_bytes as f64 / 1e6) / wall
    } else {
        0.0
    };

    println!();
    println!(
        "{:<7}: {} processed, {} ok, {} failed, {} skipped",
        "Files", total, ok, failed, skipped
    );
    println!(
        "{:<7}: {} ({})",
        "Input",
        fmt_bytes(input_bytes),
        fmt_count(input_bytes)
    );
    println!(
        "{:<7}: {} ({})",
        "Output",
        fmt_bytes(output_bytes),
        fmt_count(output_bytes)
    );
    println!("{}", fmt_ratio_line("Ratio", input_bytes, output_bytes));
    println!("{:<7}: {:.1} s", "Time", wall);
    println!("{:<7}: {:.1} MB/s", "Rate", rate);

    if ok == 0 && failed == 0 && skipped == total as usize {
        println!("nothing to {verb}, all {total} outputs already present");
    }

    for (path, message) in failures {
        if let Some(message) = message {
            eprintln!("failed to {verb} {}: {message}", path.display());
        }
    }

    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn progress_bar(len: u64) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(
            "[{bar:30.cyan/blue}] {pos}/{len} ({percent}%) | {msg} | ETA {eta} | {elapsed}",
        )
        .expect("valid progress template")
        .progress_chars("=> "),
    );
    pb
}

fn update_message(pb: &ProgressBar, start: &Instant, stats: &RunStats) {
    let elapsed = start.elapsed().as_secs_f64();
    let bytes_in = stats.bytes_in();
    let bytes_out = stats.bytes_out();
    let failed = stats.failed();
    let mbps = if elapsed > 0.0 {
        (bytes_in as f64 / 1e6) / elapsed
    } else {
        0.0
    };
    let line = format!(
        "{} read | {} written | {mbps:.1} MB/s",
        fmt_bytes(bytes_in),
        fmt_bytes(bytes_out)
    );
    if failed > 0 {
        pb.set_message(format!("{failed}f | {line}"));
    } else {
        pb.set_message(line);
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} kB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let b = s.as_bytes();
    let len = b.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

pub fn fmt_ratio_line(label: &str, before: u64, after: u64) -> String {
    if before == 0 {
        return format!("{label:<7}: n/a");
    }
    let delta = after as i128 - before as i128;
    let sign = if delta >= 0 { "+" } else { "-" };
    let pct = (delta as f64 / before as f64) * 100.0;
    let bytes_str = format!("{}{}", sign, fmt_bytes(delta.unsigned_abs() as u64));
    let pct_str = format!("({}{:.1}%)", sign, pct.abs());
    format!("{label:<7}: {} {}", bytes_str, pct_str)
}
