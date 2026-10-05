use anyhow::Result;
use clap::Parser;
use nodespace_cli::{install_broken_pipe_handler, run, Cli};

#[tokio::main]
async fn main() -> Result<()> {
    install_broken_pipe_handler();
    let cli = Cli::parse();
    run(cli).await
}
