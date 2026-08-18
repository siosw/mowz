use std::{env, io, path::Path};

use eyre::{Result, bail};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let (Some(project), Some(query), None) = (args.next(), args.next(), args.next()) else {
        bail!("usage: ctx <project> <query>");
    };

    let config = ctx::Config::load(Path::new(".ctx.yaml"))?;
    let output = ctx::query_project(&config, &project, &query, &reqwest::Client::new()).await?;
    serde_json::to_writer(io::stdout().lock(), &output)?;
    println!();
    Ok(())
}
