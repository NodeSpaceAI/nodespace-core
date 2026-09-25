//! `nodespace session ...` subcommands — PTY agent session management via gRPC.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use clap::{Args, Subcommand};
use crossterm::terminal;
use nodespace_daemon::nodespace::{
    LaunchSessionRequest, ListSessionsRequest, StreamOutputRequest, TerminateSessionRequest,
    WriteInputRequest,
};
use tokio::io::AsyncReadExt;
use tonic::Request;

use crate::terminal::{write_stdout, RawMode};
use crate::SessionClient;

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// Launch a new agent session and stream its output to stdout.
    Launch(LaunchArgs),
    /// Attach to an existing session's output stream.
    Attach(AttachArgs),
    /// List active agent sessions.
    #[command(name = "list")]
    List(ListArgs),
    /// Terminate a running session.
    Kill(KillArgs),
}

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Agent to launch: claude-code, codex, antigravity, pi, opencode
    pub agent: String,

    /// Initial prompt passed to the agent at launch time.
    #[arg(long)]
    pub prompt: Option<String>,

    /// Terminal width in columns (defaults to current terminal width).
    #[arg(long)]
    pub cols: Option<u32>,

    /// Terminal height in rows (defaults to current terminal height).
    #[arg(long)]
    pub rows: Option<u32>,
}

#[derive(Args, Debug)]
pub struct AttachArgs {
    /// Session ID to attach to.
    pub session_id: String,
}

#[derive(Args, Debug)]
pub struct ListArgs {}

#[derive(Args, Debug)]
pub struct KillArgs {
    /// Session ID to terminate.
    pub session_id: String,
}

pub async fn run(client: &mut SessionClient, action: SessionAction, _json: bool) -> Result<()> {
    match action {
        SessionAction::Launch(args) => launch(client, args).await,
        SessionAction::Attach(args) => attach(client, args).await,
        SessionAction::List(_) => list(client).await,
        SessionAction::Kill(args) => kill(client, args).await,
    }
}

async fn launch(client: &mut SessionClient, args: LaunchArgs) -> Result<()> {
    let (cols, rows) = detect_terminal_size(args.cols, args.rows);

    let resp = client
        .launch_session(Request::new(LaunchSessionRequest {
            agent_type: args.agent,
            prompt: args.prompt,
            cols,
            rows,
            // CLI-launched sessions are not tied to an ai-chat node, so there
            // is nothing to backfill — capture skips when node_id is absent.
            node_id: None,
        }))
        .await
        .context("LaunchSession RPC failed")?
        .into_inner();

    // Print session ID to stderr so scripts can capture it separately from output.
    eprintln!("session: {}", resp.session_id);

    stream_bridge(client, resp.session_id).await
}

async fn attach(client: &mut SessionClient, args: AttachArgs) -> Result<()> {
    stream_bridge(client, args.session_id).await
}

async fn list(client: &mut SessionClient) -> Result<()> {
    let resp = client
        .list_sessions(Request::new(ListSessionsRequest {}))
        .await
        .context("ListSessions RPC failed")?
        .into_inner();

    if resp.sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    println!("{:<38}  {:<12}  STARTED", "SESSION ID", "AGENT");
    for s in &resp.sessions {
        let started = format_unix_time(s.started_at);
        println!("{:<38}  {:<12}  {}", s.session_id, s.agent_type, started);
    }
    Ok(())
}

async fn kill(client: &mut SessionClient, args: KillArgs) -> Result<()> {
    let resp = client
        .terminate_session(Request::new(TerminateSessionRequest {
            session_id: args.session_id.clone(),
        }))
        .await
        .context("TerminateSession RPC failed")?
        .into_inner();

    if resp.was_running {
        println!("Session {} terminated.", args.session_id);
    } else {
        println!(
            "Session {} was not running (already cleaned up).",
            args.session_id
        );
    }
    Ok(())
}

/// Concurrently streams output from the session to stdout and forwards raw
/// stdin to the session's PTY input. Runs until the output stream ends,
/// the user presses Ctrl+D, or the user presses Ctrl+C (detach, no kill).
async fn stream_bridge(client: &mut SessionClient, session_id: String) -> Result<()> {
    let _raw = RawMode::enter()?;

    // Open the output stream.
    let mut output_stream = client
        .stream_output(Request::new(StreamOutputRequest {
            session_id: session_id.clone(),
        }))
        .await
        .context("StreamOutput RPC failed")?
        .into_inner();

    // Clone the channel for the input writer task.
    let mut input_client = client.clone();
    let input_session = session_id.clone();

    // Spawn stdin → WriteInput task.
    let input_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 256];
        loop {
            let n = match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let data = buf[..n].to_vec();

            // Ctrl+D (0x04) and Ctrl+C (0x03) detach without killing the session.
            // Filter them out before forwarding so the agent's PTY does not receive
            // SIGINT or EOF — we are detaching the CLI, not signalling the agent.
            let should_detach = data.iter().any(|&b| b == 0x04 || b == 0x03);
            let forwarded: Vec<u8> = data
                .into_iter()
                .filter(|&b| b != 0x03 && b != 0x04)
                .collect();

            if !forwarded.is_empty() {
                let _ = input_client
                    .write_input(Request::new(WriteInputRequest {
                        session_id: input_session.clone(),
                        data: forwarded,
                    }))
                    .await;
            }

            if should_detach {
                break;
            }
        }
    });

    // Drive the output stream to stdout.
    loop {
        match output_stream.message().await {
            Ok(Some(chunk)) if chunk.dropped_chunks > 0 => {
                write_stdout(dropped_notice(chunk.dropped_chunks).as_bytes())?;
            }
            Ok(Some(chunk)) => {
                write_stdout(&chunk.data)?;
            }
            Ok(None) => break, // stream ended cleanly
            Err(e) => {
                input_task.abort();
                let _ = input_task.await;
                return Err(e).context("StreamOutput error");
            }
        }
    }

    // Abort the stdin task and wait for it to exit before raw mode is restored.
    input_task.abort();
    let _ = input_task.await;
    Ok(())
}

/// Text written in place of output the daemon dropped because this stream fell
/// behind a burst. Starts on a fresh line (raw mode needs the explicit `\r`)
/// so it does not splice into a partially written line.
fn dropped_notice(dropped_chunks: u64) -> String {
    let unit = if dropped_chunks == 1 {
        "chunk"
    } else {
        "chunks"
    };
    format!("\r\n\x1b[33m[output truncated: {dropped_chunks} {unit} dropped]\x1b[0m\r\n")
}

fn detect_terminal_size(cols_override: Option<u32>, rows_override: Option<u32>) -> (u32, u32) {
    let (detected_cols, detected_rows) = terminal::size().unwrap_or((80, 24));
    let cols = cols_override.unwrap_or(detected_cols as u32);
    let rows = rows_override.unwrap_or(detected_rows as u32);
    (cols, rows)
}

fn format_unix_time(unix_secs: i64) -> String {
    if unix_secs == 0 {
        return "unknown".to_string();
    }
    match Local.timestamp_opt(unix_secs, 0).single() {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => "unknown".to_string(),
    }
}
