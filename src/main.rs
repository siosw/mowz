use std::{
    io::{self, Write},
    path::Path,
};

use clap::{Args, Parser, Subcommand};
use eyre::Result;
use serde::Serialize;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Query(QueryArgs),
    Projects,
}

#[derive(Args)]
struct QueryArgs {
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

    match cli.command {
        Command::Query(args) => query(args).await,
        Command::Projects => projects(),
    }
}

async fn query(args: QueryArgs) -> Result<()> {
    let config = mowz::Config::load(Path::new(".mowz.toml"))?;
    let time_range = mowz::TimeRange::new(args.from, args.to);
    let entries = mowz::query_project(
        &config,
        &args.project,
        &args.query,
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

fn projects() -> Result<()> {
    #[derive(Serialize)]
    struct Project<'a> {
        project: &'a str,
        backend: &'static str,
    }

    let config = mowz::Config::load(Path::new(".mowz.toml"))?;
    let mut stdout = io::stdout().lock();
    for (project, backend) in config.projects() {
        serde_json::to_writer(&mut stdout, &Project { project, backend })?;
        writeln!(stdout)?;
    }
    Ok(())
}
