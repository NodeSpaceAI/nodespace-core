use clap::Parser;
use nodespace_cli::{exit_if_broken_pipe, install_broken_pipe_handler, run, Cli};

#[tokio::main]
async fn main() {
    install_broken_pipe_handler();
    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        exit_if_broken_pipe(&error);
        // Same report `fn main() -> Result<()>` printed: `Error: ...` + causes, exit 1.
        eprintln!("Error: {error:?}");
        std::process::exit(1);
    }
}
