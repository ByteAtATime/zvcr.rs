use super::format::{format_bytes, format_duration};
use super::{FileResult, Progress};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(super) fn reporter(progress: Arc<Progress>, stop: Arc<AtomicBool>) {
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

pub(super) fn print_summary(results: &[FileResult], progress: &Progress, verify: bool) {
    let total = results.len();
    let ok = results.iter().filter(|r| r.ok).count();
    let failed = results.iter().filter(|r| !r.ok).count();
    let integrity_ok = results.iter().filter(|r| r.integrity_ok == Some(true)).count();
    let input_bytes: u64 = results.iter().map(|r| r.original_bytes).sum();
    let output_bytes: u64 = results.iter().map(|r| r.encoded_bytes).sum();
    let wall = progress.start.elapsed().as_secs_f64();

    let ref_read_ns: u128 = results.iter().map(|r| r.ref_read_ns).sum();
    let exp_write_ns: u128 = results.iter().map(|r| r.exp_write_ns).sum();
    let exp_read_ns: u128 = results.iter().map(|r| r.exp_read_ns).sum();

    println!("\n\n================================================================");
    println!("Files     : {total} processed, {ok} ok, {failed} failed");
    if !verify {
        println!("Integrity : skipped");
    } else {
        println!("Integrity : {integrity_ok} / {total}");
    }
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
        let delta = output_bytes as i128 - input_bytes as i128;
        let sign = if delta >= 0 { "+" } else { "-" };
        println!(
            "Ratio     : {pct:.2}%  ({ratio})  {sign}{}",
            format_bytes(delta.unsigned_abs() as u64)
        );
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
