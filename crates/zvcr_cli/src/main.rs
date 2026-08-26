use clap::{Parser, Subcommand};
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use zvcr::bench::discover::discover;
use zvcr::io::serialize::types::{Reader, Writer};
use zvcr::{ExperimentalWriter, ReferenceReader, ZSTD_COMPRESSION_LEVEL_DEFAULT};

mod anvil;
mod export;
mod nbt;
mod packing;
mod registry;

#[derive(Parser)]
#[command(
    name = "zvcr-cli",
    version,
    about = "Convert zvcr3d region files and export them to Minecraft worlds"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Migrate an old zvcr file to the new zvcr.rs format")]
    Migrate {
        #[arg(help = "Path to the old format region file or directory of region files")]
        input: std::path::PathBuf,
        #[arg(help = "Output file or directory path")]
        output: std::path::PathBuf,
    },
    #[command(about = "Export zvcr.rs region files to Minecraft Anvil (.mca) region files")]
    Export {
        #[arg(
            long,
            value_parser = export::parse_dim,
            help = "Target dimension: overworld, nether, or end (minecraft: prefix and aliases accepted)",
        )]
        dim: zvcr::dimension::DimensionType,
        #[arg(
            long = "in",
            required = true,
            help = "Directory containing zvcr.rs region files"
        )]
        in_dir: std::path::PathBuf,
        #[arg(
            long = "out",
            required = true,
            help = "Directory where the Minecraft world is written"
        )]
        out_dir: std::path::PathBuf,
        #[arg(
            long = "registries",
            required = true,
            help = "Directory containing the Minecraft registry JSON files"
        )]
        registries: std::path::PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Migrate { input, output } => run_migrate(&input, &output),
        Command::Export {
            dim,
            in_dir,
            out_dir,
            registries,
        } => export::run_export(dim, &in_dir, &out_dir, &registries),
    }
}

fn run_migrate(input: &Path, output: &Path) -> ExitCode {
    if !input.exists() {
        eprintln!("input path does not exist: {}", input.display());
        return ExitCode::FAILURE;
    }

    if input.is_dir() {
        run_migrate_dir(input, output)
    } else {
        run_migrate_file(input, output)
    }
}

fn run_migrate_file(input: &Path, output: &Path) -> ExitCode {
    if let Some(parent) = output.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "failed to create output directory {}: {error}",
            parent.display()
        );
        return ExitCode::FAILURE;
    }

    let data = match ReferenceReader::new(0).read(input) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("failed to read {}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let bytes = match ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT).write(&data, output) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to write {}: {error}", output.display());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "migrated {} -> {} ({} bytes)",
        input.display(),
        output.display(),
        bytes
    );
    ExitCode::SUCCESS
}

enum MigrateOutcome {
    Written,
    Skipped,
    Failed(String),
}

fn run_migrate_dir(input: &Path, output: &Path) -> ExitCode {
    let input_abs = match std::fs::canonicalize(input) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to resolve input path {}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let files = discover(&input_abs, None);
    if files.is_empty() {
        eprintln!("no region files found in {}", input_abs.display());
        return ExitCode::FAILURE;
    }

    let total = files.len() as u64;
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "[{bar:30.cyan/blue}] {pos}/{len} ({percent}%) | {msg} | ETA {eta} | {elapsed}",
        )
        .expect("valid progress template")
        .progress_chars("=> "),
    );

    let start = Instant::now();
    let bytes_in_total = AtomicU64::new(0);
    let bytes_out_total = AtomicU64::new(0);
    let failed_count = AtomicUsize::new(0);
    let skipped_count = AtomicUsize::new(0);

    let results: Vec<(std::path::PathBuf, MigrateOutcome)> = files
        .par_iter()
        .progress_with(pb.clone())
        .map(|file| {
            let relative = file.strip_prefix(&input_abs).unwrap_or(file);
            let out = output.join(relative);
            if out.exists() {
                skipped_count.fetch_add(1, Ordering::Relaxed);
                update_migrate_message(
                    &pb,
                    &start,
                    &bytes_in_total,
                    &bytes_out_total,
                    &failed_count,
                    &skipped_count,
                );
                return (file.clone(), MigrateOutcome::Skipped);
            }

            let bytes_in = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            bytes_in_total.fetch_add(bytes_in, Ordering::Relaxed);

            if let Some(parent) = out.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                failed_count.fetch_add(1, Ordering::Relaxed);
                let message = format!(
                    "failed to create output directory {}: {error}",
                    parent.display()
                );
                update_migrate_message(
                    &pb,
                    &start,
                    &bytes_in_total,
                    &bytes_out_total,
                    &failed_count,
                    &skipped_count,
                );
                return (file.clone(), MigrateOutcome::Failed(message));
            }

            let data = match ReferenceReader::new(0).read(file) {
                Ok(data) => data,
                Err(error) => {
                    failed_count.fetch_add(1, Ordering::Relaxed);
                    let message = format!("failed to read {}: {error}", file.display());
                    update_migrate_message(
                        &pb,
                        &start,
                        &bytes_in_total,
                        &bytes_out_total,
                        &failed_count,
                        &skipped_count,
                    );
                    return (file.clone(), MigrateOutcome::Failed(message));
                }
            };

            let result = match ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT)
                .write(&data, &out)
            {
                Ok(bytes) => {
                    bytes_out_total.fetch_add(bytes as u64, Ordering::Relaxed);
                    MigrateOutcome::Written
                }
                Err(error) => {
                    failed_count.fetch_add(1, Ordering::Relaxed);
                    MigrateOutcome::Failed(format!("failed to write {}: {error}", out.display()))
                }
            };

            update_migrate_message(
                &pb,
                &start,
                &bytes_in_total,
                &bytes_out_total,
                &failed_count,
                &skipped_count,
            );
            (file.clone(), result)
        })
        .collect();

    pb.finish_with_message("migration complete");

    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for (_, outcome) in &results {
        match outcome {
            MigrateOutcome::Written => ok += 1,
            MigrateOutcome::Skipped => skipped += 1,
            MigrateOutcome::Failed(_) => failed += 1,
        }
    }

    let input_bytes = bytes_in_total.load(Ordering::Relaxed);
    let output_bytes = bytes_out_total.load(Ordering::Relaxed);
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
        println!("nothing to migrate, all {} outputs already present", total);
    }

    let failures: Vec<(std::path::PathBuf, String)> = results
        .iter()
        .filter_map(|(path, outcome)| match outcome {
            MigrateOutcome::Failed(error) => Some((path.clone(), error.clone())),
            _ => None,
        })
        .collect();

    for (path, error) in &failures {
        eprintln!("failed to migrate {}: {error}", path.display());
    }

    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn update_migrate_message(
    pb: &ProgressBar,
    start: &Instant,
    bytes_in_total: &AtomicU64,
    bytes_out_total: &AtomicU64,
    failed_count: &AtomicUsize,
    _skipped_count: &AtomicUsize,
) {
    let elapsed = start.elapsed().as_secs_f64();
    let bytes_in = bytes_in_total.load(Ordering::Relaxed);
    let bytes_out = bytes_out_total.load(Ordering::Relaxed);
    let failed = failed_count.load(Ordering::Relaxed);
    let mbps = if elapsed > 0.0 {
        (bytes_in as f64 / 1e6) / elapsed
    } else {
        0.0
    };
    let stats = format!(
        "{} read | {} written | {mbps:.1} MB/s",
        fmt_bytes(bytes_in),
        fmt_bytes(bytes_out)
    );
    if failed > 0 {
        pb.set_message(format!("{failed}f | {stats}"));
    } else {
        pb.set_message(stats);
    }
}

fn fmt_bytes(bytes: u64) -> String {
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

fn fmt_count(n: u64) -> String {
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

fn fmt_ratio_line(label: &str, before: u64, after: u64) -> String {
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
