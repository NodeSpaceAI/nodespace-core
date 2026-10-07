//! `nodespace session ...` subcommands — PTY agent session management via gRPC.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use clap::builder::NonEmptyStringValueParser;
use clap::{Args, Subcommand};
use crossterm::terminal;
use nodespace_agent::agent_types::AgentType;
use nodespace_agent::pty::SESSION_ENV_VAR;
use nodespace_daemon::nodespace::{
    LaunchSessionRequest, ListSessionsRequest, ReportHarnessSessionRequest, StreamOutputRequest,
    TerminateSessionRequest, WriteInputRequest,
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
    /// Tell NodeSpace the agent's own id for the conversation running in a
    /// launched session. An agent's plugin runs this when the session starts.
    #[command(name = "report-harness-session")]
    ReportHarnessSession(ReportHarnessSessionArgs),
}

/// The ids `session launch` takes: the agents the daemon can launch.
fn agent_ids() -> Vec<&'static str> {
    AgentType::ALL.map(AgentType::id).to_vec()
}

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Agent to launch: claude-code, codex, antigravity, pi, opencode
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(agent_ids()))]
    pub agent: String,

    /// Initial prompt passed to the agent at launch time.
    #[arg(long)]
    pub prompt: Option<String>,

    /// Id of the project to launch the session for. The session runs in that
    /// project's folder on this machine. Without it the session runs in a
    /// private folder of its own.
    #[arg(long)]
    pub project: Option<String>,

    /// The project's folder on this machine: the absolute path of its
    /// checkout. Needed the first time a session is launched for a project,
    /// and remembered on this machine from then on.
    #[arg(long, requires = "project")]
    pub folder: Option<String>,

    /// Id of the task to launch the session for. The agent's plugin opens
    /// with that task's context.
    #[arg(long)]
    pub task: Option<String>,

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

#[derive(Args, Debug)]
pub struct ReportHarnessSessionArgs {
    /// The agent's own id for the conversation: the one its resume flag takes.
    pub harness_session_id: String,

    /// The launched session to report for. A launched session's environment
    /// names it in `NODESPACE_SESSION`. An empty value is no session: a
    /// harness that can only blank the variable for the commands it runs (it
    /// cannot remove it) leaves an empty one, which is not a launch.
    #[arg(long, env = SESSION_ENV_VAR, value_parser = NonEmptyStringValueParser::new())]
    pub session: String,
}

pub async fn run(client: &mut SessionClient, action: SessionAction, json: bool) -> Result<()> {
    match action {
        SessionAction::Launch(args) => launch(client, args).await,
        SessionAction::Attach(args) => attach(client, args).await,
        SessionAction::List(_) => list(client).await,
        SessionAction::Kill(args) => kill(client, args).await,
        SessionAction::ReportHarnessSession(args) => {
            report_harness_session(client, args, json).await
        }
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
            // CLI-launched sessions are not tied to an ai-chat-pty node, so
            // nothing is written when the session ends.
            node_id: None,
            project_id: args.project,
            project_folder: args.folder,
            task_id: args.task,
        }))
        .await
        .context("LaunchSession RPC failed")?
        .into_inner();

    // Print session ID to stderr so scripts can capture it separately from output.
    eprintln!("session: {}", resp.session_id);

    stream_bridge(client, resp.session_id).await
}

async fn report_harness_session(
    client: &mut SessionClient,
    args: ReportHarnessSessionArgs,
    json: bool,
) -> Result<()> {
    let resp = client
        .report_harness_session(Request::new(ReportHarnessSessionRequest {
            session_id: args.session.clone(),
            harness_session_id: args.harness_session_id,
        }))
        .await
        .context("ReportHarnessSession RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::json!({ "session_id": args.session, "node_id": resp.node_id })
        );
    } else {
        println!("Recorded for session {}.", args.session);
    }
    Ok(())
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
/// so it does not splice into a partially written line. Keep the wording in
/// step with the desktop terminal's `formatDroppedNotice` (`pty-output.ts`).
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

#[cfg(test)]
mod tests {
    use super::dropped_notice;
    use super::{LaunchArgs, ReportHarnessSessionArgs, SESSION_ENV_VAR};
    use clap::{CommandFactory, Parser};
    use nodespace_agent::agent_types::AgentType;

    #[derive(Parser)]
    struct Launch {
        #[command(flatten)]
        args: LaunchArgs,
    }

    #[derive(Parser)]
    struct Report {
        #[command(flatten)]
        args: ReportHarnessSessionArgs,
    }

    /// `session launch` lists and takes exactly the ids the daemon accepts.
    #[test]
    fn launch_takes_the_daemons_agent_ids_and_no_others() {
        let help = Launch::command().render_long_help().to_string();
        for agent in AgentType::ALL {
            assert!(help.contains(agent.id()), "help omits {}", agent.id());
            assert!(Launch::try_parse_from(["launch", agent.id()]).is_ok());
        }
        for other in ["open-code", "antigravity-cli", "gemini-cli"] {
            assert!(!help.contains(other), "help lists {other}");
            assert!(Launch::try_parse_from(["launch", other]).is_err());
        }
    }

    #[test]
    fn launch_takes_a_project_a_folder_and_a_task() {
        let launch = Launch::try_parse_from([
            "launch",
            "claude-code",
            "--project",
            "p1",
            "--folder",
            "/work/core",
            "--task",
            "t1",
        ])
        .unwrap()
        .args;
        assert_eq!(launch.project.as_deref(), Some("p1"));
        assert_eq!(launch.folder.as_deref(), Some("/work/core"));
        assert_eq!(launch.task.as_deref(), Some("t1"));

        // A folder belongs to a project.
        assert!(Launch::try_parse_from(["launch", "codex", "--folder", "/work/core"]).is_err());
    }

    /// The session is named by flag here: the variable a launch sets is
    /// process-global, and other tests share the process.
    #[test]
    fn a_report_names_its_session() {
        let report = Report::try_parse_from(["report", "h-1", "--session", "s-1"])
            .unwrap()
            .args;
        assert_eq!(report.harness_session_id, "h-1");
        assert_eq!(report.session, "s-1");
        assert_eq!(SESSION_ENV_VAR, "NODESPACE_SESSION");
    }

    /// An empty session is no session, whether it came from a flag or from
    /// the blanked variable a harness leaves in the commands it runs.
    #[test]
    fn a_report_refuses_an_empty_session() {
        assert!(Report::try_parse_from(["report", "h-1", "--session", ""]).is_err());
    }

    #[test]
    fn dropped_notice_starts_on_a_fresh_line_and_pluralises() {
        assert_eq!(
            dropped_notice(1),
            "\r\n\x1b[33m[output truncated: 1 chunk dropped]\x1b[0m\r\n"
        );
        assert!(dropped_notice(42).contains("[output truncated: 42 chunks dropped]"));
    }
}
