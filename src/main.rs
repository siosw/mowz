use std::{
    env,
    io::{self, Write},
    path::Path,
};

use eyre::{Result, bail};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let (Some(project), Some(query), None) = (args.next(), args.next(), args.next()) else {
        bail!("usage: ctx <project> <query>");
    };

    let config = ctx::Config::load(Path::new(".ctx.toml"))?;
    let entries = ctx::query_project(&config, &project, &query, &reqwest::Client::new()).await?;
    let mut stdout = io::stdout().lock();
    for entry in entries {
        serde_json::to_writer(&mut stdout, &entry)?;
        writeln!(stdout)?;
    }
    Ok(())
}
