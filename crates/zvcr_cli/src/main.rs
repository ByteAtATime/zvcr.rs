use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::ExitCode;
use zvcr::bench::discover::discover;
use zvcr::io::serialize::types::{Reader, Writer};
use zvcr::{ExperimentalWriter, ReferenceReader, ZSTD_COMPRESSION_LEVEL_DEFAULT};

mod anvil;
mod export;
mod nbt;
mod packing;
mod progress;
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

fn migrate_one(input_root: &Path, output_root: &Path, file: &Path) -> progress::Outcome {
    let relative = match file.strip_prefix(input_root) {
        Ok(relative) => relative,
        Err(_) => return progress::Outcome::Failed(format!("unresolved path: {}", file.display())),
    };
    let out = output_root.join(relative);
    if out.exists() {
        return progress::Outcome::Skipped;
    }

    let bytes_in = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    if let Some(parent) = out.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return progress::Outcome::Failed(format!(
            "failed to create output directory {}: {error}",
            parent.display()
        ));
    }

    let data = match ReferenceReader::new(0).read(file) {
        Ok(data) => data,
        Err(error) => {
            return progress::Outcome::Failed(format!(
                "failed to read {}: {error}",
                file.display()
            ));
        }
    };

    match ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT).write(&data, &out) {
        Ok(bytes) => progress::Outcome::Written(progress::Written {
            bytes_in,
            bytes_out: bytes as u64,
        }),
        Err(error) => {
            progress::Outcome::Failed(format!("failed to write {}: {error}", out.display()))
        }
    }
}

fn run_migrate_dir(input: &Path, output: &Path) -> ExitCode {
    let input_abs = match std::fs::canonicalize(input) {
        Ok(abs) => abs,
        Err(error) => {
            eprintln!("failed to resolve {}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let files = discover(&input_abs, None);
    if files.is_empty() {
        eprintln!("no region files found in {}", input.display());
        return ExitCode::FAILURE;
    }

    progress::run_all("migrate", "migration", &files, |file| {
        migrate_one(&input_abs, output, file)
    })
}
