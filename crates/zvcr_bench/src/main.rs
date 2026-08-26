use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    no_verify: bool,

    #[arg(long, num_args = 0..=1, default_missing_value = "128")]
    sample: Option<usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let verify = !cli.no_verify;
    zvcr::bench::run(std::path::Path::new("test_files"), verify, cli.sample);
    Ok(())
}
