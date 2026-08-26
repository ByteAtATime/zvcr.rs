use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use zvcr::io::serialize::types::{Reader, Writer};
use zvcr::{ExperimentalWriter, ReferenceReader, ZSTD_COMPRESSION_LEVEL_DEFAULT};

#[derive(Parser)]
#[command(name = "zvcr-cli", about = "Convert a reference-format region file to the experimental format")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Convert { input: PathBuf, output: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert { input, output } => run_convert(&input, &output),
    }
}

fn run_convert(input: &Path, output: &Path) -> ExitCode {
    if let Some(parent) = output.parent()
        && let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create output directory {}: {error}", parent.display());
            return ExitCode::FAILURE;
        }

    let data = match ReferenceReader::new(0).read(input) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("Failed to read {}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let bytes = match ExperimentalWriter::new(ZSTD_COMPRESSION_LEVEL_DEFAULT).write(&data, output) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("Failed to write {}: {error}", output.display());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "Converted {} -> {} ({} bytes)",
        input.display(),
        output.display(),
        bytes
    );
    ExitCode::SUCCESS
}
