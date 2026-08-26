use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::ExitCode;
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
    #[command(about = "Convert an old zvcr file to the new zvcr.rs format")]
    Convert {
        #[arg(
            help = "Path to the old format region file to convert",
            value_name = "INPUT"
        )]
        input: std::path::PathBuf,
        #[arg(
            help = "Path where the new zvcr.rs region file is written",
            value_name = "OUTPUT"
        )]
        output: std::path::PathBuf,
    },
    #[command(about = "Export zvcr.rs region files to Minecraft Anvil (.mca) region files")]
    Export {
        #[arg(
            long,
            value_parser = export::parse_dim,
            help = "Target dimension: overworld, nether, or end (minecraft: prefix and aliases accepted)",
            value_name = "DIM"
        )]
        dim: zvcr::dimension::DimensionType,
        #[arg(
            long = "in",
            required = true,
            help = "Directory containing zvcr.rs region files",
            value_name = "INPUT_DIR"
        )]
        in_dir: std::path::PathBuf,
        #[arg(
            long = "out",
            required = true,
            help = "Directory where the Minecraft world is written",
            value_name = "WORLD_DIR"
        )]
        out_dir: std::path::PathBuf,
        #[arg(
            long = "registries",
            required = true,
            help = "Directory containing the Minecraft registry JSON files",
            value_name = "REGISTRIES_DIR"
        )]
        registries: std::path::PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert { input, output } => run_convert(&input, &output),
        Command::Export {
            dim,
            in_dir,
            out_dir,
            registries,
        } => export::run_export(dim, &in_dir, &out_dir, &registries),
    }
}

fn run_convert(input: &Path, output: &Path) -> ExitCode {
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
        "converted {} -> {} ({} bytes)",
        input.display(),
        output.display(),
        bytes
    );
    ExitCode::SUCCESS
}
