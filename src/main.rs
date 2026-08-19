use std::{
    io::{self, Write},
    path::Path,
};

use clap::Parser;
use eyre::Result;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[arg(long, default_value = "now-1h")]
    from: String,
    #[arg(long, default_value = "now")]
    to: String,
    #[arg(allow_hyphen_values = true)]
    project: String,
    #[arg(allow_hyphen_values = true)]
    query: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = mowz::Config::load(Path::new(".mowz.toml"))?;
    let time_range = mowz::TimeRange::new(cli.from, cli.to);
    let entries = mowz::query_project(
        &config,
        &cli.project,
        &cli.query,
        &time_range,
        &reqwest::Client::new(),
    )
    .await?;
    let mut stdout = io::stdout().lock();
    for entry in entries {
        serde_json::to_writer(&mut stdout, &entry)?;
        writeln!(stdout)?;
    }
    Ok(())
}
