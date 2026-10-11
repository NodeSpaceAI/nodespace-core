use clap::Parser;
use nodespace_cli::commands::wait::WaitEnded;
use nodespace_cli::{exit_if_broken_pipe, install_broken_pipe_handler, run, Cli};

#[tokio::main]
async fn main() {
    install_broken_pipe_handler();
    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        exit_if_broken_pipe(&error);
        // A wait that ended without its condition exits with its own status.
        if let Some(ended) = error.downcast_ref::<WaitEnded>() {
            eprintln!("{}", ended.message);
            std::process::exit(ended.code);
        }
        // Same report `fn main() -> Result<()>` printed: `Error: ...` + causes, exit 1.
        eprintln!("Error: {error:?}");
        std::process::exit(1);
    }
}
