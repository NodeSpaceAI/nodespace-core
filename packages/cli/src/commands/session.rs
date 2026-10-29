//! `nodespace session ...` subcommands — PTY agent session management via gRPC.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use clap::builder::NonEmptyStringValueParser;
use clap::{Args, Subcommand};
use crossterm::terminal;
use nodespace_agent::agent_types::AgentType;
use nodespace_agent::pty::SESSION_ENV_VAR;
use nodespace_daemon::nodespace::{
    LaunchSessionRequest, ListSessionsRequest, OutputChunk, ReportHarnessSessionRequest,
    SessionInfo, StreamOutputRequest, TerminateSessionRequest, WriteInputRequest,
};
use tokio_stream::{Stream, StreamExt};
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

    /// Id of the software project to launch the session for. The session
    /// runs in that project's folder on this machine. A plain project has no
    /// folder and is refused. Without it the session runs in a private
    /// folder of its own.
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
        SessionAction::List(_) => list(client, json).await,
        SessionAction::Kill(args) => kill(client, args, json).await,
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

async fn list(client: &mut SessionClient, json: bool) -> Result<()> {
    let resp = client
        .list_sessions(Request::new(ListSessionsRequest {}))
        .await
        .context("ListSessions RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&sessions_json(&resp.sessions))?
        );
        return Ok(());
    }

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

/// The `session list --json` shape: `{"sessions": [...]}`, empty when nothing
/// is running. `started_at` is unix seconds, as the daemon reports it.
fn sessions_json(sessions: &[SessionInfo]) -> serde_json::Value {
    serde_json::json!({
        "sessions": sessions
            .iter()
            .map(|s| serde_json::json!({
                "session_id": s.session_id,
                "agent_type": s.agent_type,
                "started_at": s.started_at,
            }))
            .collect::<Vec<_>>(),
    })
}

async fn kill(client: &mut SessionClient, args: KillArgs, json: bool) -> Result<()> {
    let resp = client
        .terminate_session(Request::new(TerminateSessionRequest {
            session_id: args.session_id.clone(),
        }))
        .await
        .context("TerminateSession RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&kill_json(&args.session_id, resp.was_running))?
        );
    } else if resp.was_running {
        println!("Session {} terminated.", args.session_id);
    } else {
        println!(
            "Session {} was not running (already cleaned up).",
            args.session_id
        );
    }
    Ok(())
}

/// The `session kill --json` shape.
fn kill_json(session_id: &str, was_running: bool) -> serde_json::Value {
    serde_json::json!({ "session_id": session_id, "was_running": was_running })
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
    let mut stdin_chunks = spawn_stdin_reader(std::io::stdin());
    let input_task = tokio::spawn(async move {
        while let Some(chunk) = stdin_chunks.recv().await {
            let (forwarded, should_detach) = split_detach(&chunk);

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

    // Returns only once the stdin task has exited, i.e. before raw mode is restored.
    pump_output(&mut output_stream, input_task, write_stdout).await
}

/// Reads `reader` on a dedicated OS thread and hands each chunk to the
/// returned receiver. A blocking read cannot be cancelled; on a runtime
/// blocking-pool thread (as with `tokio::io::stdin`) it would hold the runtime
/// shutdown until a line arrived. This thread is never joined, so a read still
/// in flight when the session ends does not delay process exit.
fn spawn_stdin_reader<R>(mut reader: R) -> tokio::sync::mpsc::Receiver<Vec<u8>>
where
    R: std::io::Read + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    std::thread::Builder::new()
        .name("session-stdin".into())
        .spawn(move || {
            let mut buf = [0u8; 256];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .expect("spawn stdin reader thread");
    rx
}

/// Splits raw terminal input into the bytes to forward to the agent and
/// whether the user asked to detach. Ctrl+D (0x04) and Ctrl+C (0x03) detach
/// without killing the session; they are filtered out so the agent's PTY
/// receives neither SIGINT nor EOF -- we are detaching the CLI, not
/// signalling the agent.
fn split_detach(data: &[u8]) -> (Vec<u8>, bool) {
    // Bytes after the detach key in the same chunk are dropped (a pasted ^C/^D detaches).
    let should_detach = data.iter().any(|&b| b == 0x04 || b == 0x03);
    let forwarded = data
        .iter()
        .copied()
        .filter(|&b| b != 0x03 && b != 0x04)
        .collect();
    (forwarded, should_detach)
}

/// Writes the session output to `sink` until the stream ends, errors, or the
/// input task finishes (the user detached, or stdin closed). Owns the input
/// task and always leaves it finished: a task that already completed is joined
/// once (never polled again), otherwise it is aborted and awaited.
async fn pump_output<S, W>(
    output: &mut S,
    mut input_task: tokio::task::JoinHandle<()>,
    mut sink: W,
) -> Result<()>
where
    S: Stream<Item = std::result::Result<OutputChunk, tonic::Status>> + Unpin,
    W: FnMut(&[u8]) -> Result<()>,
{
    let result = loop {
        tokio::select! {
            joined = &mut input_task => {
                if let Err(e) = joined {
                    if !e.is_cancelled() {
                        // Raw mode is on: explicit carriage return.
                        eprint!("\r\n[input task failed: {e}]\r\n");
                    }
                }
                return Ok(());
            }
            msg = output.next() => match msg {
                Some(Ok(chunk)) if chunk.dropped_chunks > 0 => {
                    if let Err(e) = sink(dropped_notice(chunk.dropped_chunks).as_bytes()) {
                        break Err(e);
                    }
                }
                Some(Ok(chunk)) => {
                    if let Err(e) = sink(&chunk.data) {
                        break Err(e);
                    }
                }
                None => break Ok(()), // stream ended cleanly
                Some(Err(e)) => break Err(e).context("StreamOutput error"),
            },
        }
    };
    input_task.abort();
    let _ = input_task.await;
    result
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
    use super::{dropped_notice, pump_output, spawn_stdin_reader, split_detach};
    use super::{kill_json, sessions_json};
    use super::{LaunchArgs, ReportHarnessSessionArgs, SESSION_ENV_VAR};
    use clap::{CommandFactory, Parser};
    use nodespace_agent::agent_types::AgentType;
    use nodespace_daemon::nodespace::OutputChunk;
    use nodespace_daemon::nodespace::SessionInfo;

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

    #[test]
    fn ctrl_c_and_ctrl_d_detach_and_are_not_forwarded() {
        assert_eq!(split_detach(b"ab"), (b"ab".to_vec(), false));
        assert_eq!(split_detach(&[b'a', 0x03, b'b']), (b"ab".to_vec(), true));
        assert_eq!(split_detach(&[0x04]), (Vec::new(), true));
    }

    fn chunk(data: &[u8]) -> OutputChunk {
        OutputChunk {
            data: data.to_vec(),
            ..Default::default()
        }
    }

    /// A live, idle session never ends its stream: the pump must still return
    /// once the input side finishes (the user detached).
    #[tokio::test]
    async fn pump_returns_when_input_task_finishes_while_output_is_idle() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<OutputChunk, tonic::Status>>(4);
        let mut stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        tx.send(Ok(chunk(b"hello"))).await.unwrap();
        let input = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        let mut written = Vec::new();
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pump_output(&mut stream, input, |d| {
                written.extend_from_slice(d);
                Ok(())
            }),
        )
        .await
        .expect("pump must not hang after detach");
        assert!(res.is_ok());
        assert_eq!(written, b"hello");
        drop(tx);
    }

    #[tokio::test]
    async fn pump_returns_when_output_stream_ends() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<OutputChunk, tonic::Status>>(4);
        let mut stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let input = tokio::spawn(std::future::pending::<()>());
        drop(tx);
        let res = pump_output(&mut stream, input, |_| Ok(())).await;
        assert!(res.is_ok());
    }

    /// Detach runs the real cleanup: the finished handle is joined once, not re-polled.
    #[tokio::test]
    async fn detach_cleanup_does_not_repoll_the_finished_input_task() {
        let (_tx, rx) = tokio::sync::mpsc::channel::<Result<OutputChunk, tonic::Status>>(4);
        let mut stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let input = tokio::spawn(async {});
        pump_output(&mut stream, input, |_| Ok(())).await.unwrap();
    }

    /// When the stream ends first, the still-running input task is aborted.
    #[tokio::test]
    async fn stream_end_aborts_a_running_input_task() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<OutputChunk, tonic::Status>>(4);
        let mut stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        drop(tx);
        let input = tokio::spawn(std::future::pending::<()>());
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pump_output(&mut stream, input, |_| Ok(())),
        )
        .await
        .unwrap()
        .unwrap();
    }

    /// A stdin read still blocked when the session ends must not hold the
    /// runtime's shutdown (the process would hang until Enter).
    #[test]
    fn runtime_shutdown_does_not_wait_for_a_blocked_stdin_read() {
        struct BlockedForever(std::sync::mpsc::Receiver<()>);
        impl std::io::Read for BlockedForever {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                let _ = self.0.recv(); // never signalled while the test runs
                Ok(0)
            }
        }

        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<OutputChunk, tonic::Status>>(4);
                let mut stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                let mut chunks = spawn_stdin_reader(BlockedForever(blocked));
                let input = tokio::spawn(async move { while chunks.recv().await.is_some() {} });
                drop(tx); // the session ended on its own
                pump_output(&mut stream, input, |_| Ok(())).await.unwrap();
            });
            drop(rt);
            let _ = done_tx.send(());
        });
        let finished = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(release);
        assert!(finished.is_ok(), "runtime shutdown hung on a blocked read");
    }

    #[tokio::test]
    async fn stdin_reader_delivers_chunks_then_closes_at_eof() {
        let mut rx = spawn_stdin_reader(std::io::Cursor::new(b"hello".to_vec()));
        assert_eq!(rx.recv().await.unwrap(), b"hello");
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn session_list_json_has_a_stable_sessions_array() {
        assert_eq!(sessions_json(&[]), serde_json::json!({ "sessions": [] }));
        let one = SessionInfo {
            session_id: "s-1".into(),
            agent_type: "claude-code".into(),
            started_at: 1_700_000_000,
            node_id: None,
        };
        assert_eq!(
            sessions_json(&[one]),
            serde_json::json!({ "sessions": [
                { "session_id": "s-1", "agent_type": "claude-code", "started_at": 1_700_000_000i64 }
            ] })
        );
    }

    #[test]
    fn session_kill_json_reports_whether_it_was_running() {
        assert_eq!(
            kill_json("s-1", false),
            serde_json::json!({ "session_id": "s-1", "was_running": false })
        );
    }
}
