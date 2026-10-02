//! Session end for a terminal chat: marks the session's `ai-chat-pty` node
//! ended and, when capture is enabled, saves what the session left behind.
//!
//! A terminal chat's node exists before its session launches (ADR-088,
//! ADR-034): it is created up front and its id is passed through
//! `LaunchSession`. At session end that node is **backfilled**; a new node is
//! never minted.
//!
//! [`finalize_capture`] is called by the agent session handler after the PTY
//! process exits. It always records the session's own state (`session_status`,
//! `session_id`, `exit_code`, `last_active`): a chat left `active` would read
//! as a session still running. Capture is the opt-in part, and is about the
//! session's content: with `capture.enabled = true` the node also gets, by
//! content level, the summary and the transcript.
//!
//! What capture can record is deliberately limited: NodeSpace only sees the
//! terminal's raw output stream, which has no recoverable turn structure, so
//! a terminal chat has no messages.
//!
//! The two content fields are different things (ADR-061 §7):
//!
//! - the **transcript** is the raw scrollback. It may hold secrets and cannot
//!   be scrubbed, so it is `local_only`;
//! - the **summary** is not `local_only`, so it is never output. It is prose a
//!   [`SessionSummarizer`] derives on this machine from the output with its
//!   escape sequences removed. When there is no summarizer the chat has no
//!   summary.
//!
//! `session_id` is the id the harness gave its own conversation, the one its
//! resume flag takes (ADR-061 §6), not the PTY session's: that one names a
//! process that no longer exists.
//!
//! The call is fire-and-forget from the session lifecycle perspective: any
//! error is logged but does not surface to the user or block teardown.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nodespace_agent::pty::{ExitStatus, SessionCapture};
use nodespace_core::models::{AiChatSessionStatus, CoreNodeType, NodeUpdate};
use nodespace_core::services::{NodeService as CoreNodeService, NodeServiceError};
use serde_json::json;
use uuid::Uuid;

use crate::services::settings_service::{CaptureConfig, CaptureContentSetting};

/// How many times the write is attempted when another writer changes the node
/// between the read of its version and the write.
const MAX_WRITE_ATTEMPTS: usize = 5;

/// Derives the prose summary of a finished session.
#[async_trait]
pub trait SessionSummarizer: Send + Sync {
    /// Summarize a session from the plain text of what it printed.
    ///
    /// `None` when there is nothing to derive a summary with or from; the
    /// chat then keeps no summary.
    async fn summarize(&self, plain_text: &str) -> Option<String>;
}

/// Parameters describing a completed PTY session.
pub struct CompletedSession {
    /// The PTY session's id. It identifies the session in logs; it is not
    /// stored, since the process it named is gone.
    pub id: Uuid,
    /// ID of the `ai-chat-pty` node this session is a view onto. The node is
    /// created up front (before launch). `None` for a session launched
    /// without a node (from the CLI), in which case nothing is written.
    pub node_id: Option<String>,
    pub ended_at: DateTime<Utc>,
    pub exit_status: ExitStatus,
    /// The id the harness recorded for its own conversation, which its resume
    /// flag takes. `None` for a harness that records none, or one that exited
    /// before starting a conversation.
    pub harness_session_id: Option<String>,
}

/// Record the end of a session on its existing `ai-chat-pty` node.
///
/// Returns `Ok(Some(node_id))` if the node was written, `Ok(None)` if no
/// `node_id` was associated with the session, or `Err` on an ops failure.
/// Callers should log errors and continue — a failed write must not affect
/// session teardown.
///
/// The caller is responsible for reading `CaptureConfig` once at session-launch
/// time and passing the snapshot in here, so this function doesn't re-read
/// daemon.toml on every session end.
///
/// Only a terminal chat is written: `LaunchSession` takes any node id, and a
/// node of another type is refused here rather than left to its schema, which
/// for a user-defined type would accept the session fields.
///
/// The write goes through the normal validated update, at the node's current
/// version: the fields are checked against the closed `ai-chat-pty` schema
/// like any other write. A viewer may edit the same node while the session
/// ends (a rename, say), so a version conflict is an ordinary race and is
/// retried against the fresh version rather than dropping the write.
///
/// The summary is a second write. Deriving it takes a model seconds, and
/// waits for that model to be idle, and the session must not read as running
/// for that long. So the session's end is recorded first, and the summary
/// follows when `summarizer` produces one.
pub async fn finalize_capture(
    session: &CompletedSession,
    capture: &SessionCapture,
    node_service: &Arc<CoreNodeService>,
    config: &CaptureConfig,
    summarizer: &dyn SessionSummarizer,
) -> anyhow::Result<Option<String>> {
    let Some(node_id) = session.node_id.as_deref() else {
        tracing::warn!(
            session_id = %session.id,
            "session end: no node_id associated with session, nothing to write"
        );
        return Ok(None);
    };

    let properties = build_session_end_properties(session, capture, config);
    write_at_current_version(node_service, node_id, properties, || {
        current_version(node_service, node_id)
    })
    .await?;

    tracing::info!(
        session_id = %session.id,
        node_id = %node_id,
        captured = config.enabled,
        "session end: wrote ai-chat-pty node"
    );

    if saves_summary(config) {
        if let Some(summary) = summarizer.summarize(&capture.plain_text()).await {
            write_at_current_version(node_service, node_id, json!({ "summary": summary }), || {
                current_version(node_service, node_id)
            })
            .await?;
        }
    }
    Ok(Some(node_id.to_string()))
}

/// Whether capture saves a summary of the session.
fn saves_summary(config: &CaptureConfig) -> bool {
    config.enabled
        && matches!(
            config.content,
            CaptureContentSetting::Summary | CaptureContentSetting::Full
        )
}

/// The terminal chat's version, as the validated update expects it. A node
/// that is not a terminal chat (or of a type extending one) is refused.
async fn current_version(
    node_service: &Arc<CoreNodeService>,
    node_id: &str,
) -> anyhow::Result<i64> {
    let node = node_service
        .get_node(node_id)
        .await
        .map_err(|e| anyhow::anyhow!("session end: failed to read ai-chat-pty node: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("session end: ai-chat-pty node {node_id} not found"))?;
    let is_terminal_chat = node_service
        .type_is_a(&node.node_type, CoreNodeType::AiChatPty)
        .await
        .map_err(|e| anyhow::anyhow!("session end: failed to resolve the node's type: {e}"))?;
    if !is_terminal_chat {
        return Err(anyhow::anyhow!(
            "session end: node {node_id} is a '{}', not a terminal chat",
            node.node_type
        ));
    }
    Ok(node.version)
}

/// Write `properties` through the validated update at the version
/// `read_version` reports, re-reading and retrying when another writer got
/// there first. Any other refusal fails the same way every attempt, so it is
/// returned at once.
///
/// `read_version` is a parameter so a test can hand back a version that has
/// already been overtaken, which is the race this loop exists for.
async fn write_at_current_version<F, Fut>(
    node_service: &Arc<CoreNodeService>,
    node_id: &str,
    properties: serde_json::Value,
    mut read_version: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<i64>>,
{
    for attempt in 0..MAX_WRITE_ATTEMPTS {
        let version = read_version().await?;
        let update = NodeUpdate::new().with_properties(properties.clone());
        match node_service.update_node(node_id, version, update).await {
            Ok(_) => return Ok(()),
            Err(NodeServiceError::VersionConflict { .. }) if attempt + 1 < MAX_WRITE_ATTEMPTS => {
                tracing::debug!(
                    node_id,
                    attempt,
                    "version conflict writing session end, retrying"
                );
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "session end: failed to write ai-chat-pty node: {e}"
                ))
            }
        }
    }
    Err(anyhow::anyhow!(
        "session end: failed to write ai-chat-pty node {node_id} after {MAX_WRITE_ATTEMPTS} attempts"
    ))
}

/// Build the properties a finished session writes to its terminal chat.
///
/// Every key is a field the chat's schema chain declares, under its bare
/// name: `session_status`, `session_id`, `transcript` and `exit_code` are
/// `ai-chat-pty`'s, and `summary` and `last_active` the `ai-chat` base's. The
/// update pipeline places each in its declaring schema's bucket.
///
/// The session's own state is always written: it is marked `ended`, with its
/// exit code and the harness's id for the conversation. A node can host one
/// session after another, so a harness that left no id clears the one the
/// session before it left.
///
/// The summary and the transcript are the session's content, and are written
/// only when capture is enabled, at its content level. The transcript is the
/// raw scrollback. The summary is derived afterwards (see
/// [`finalize_capture`]); here it is cleared, so the node never shows the
/// previous session's summary against this session.
///
/// When the session started and ended are the node's own `created_at` and
/// `last_active`, and who ran it is the node's `agent`, set when the session
/// launched; none of them is written again under a second name.
///
/// Extracted so tests can verify property construction without a NodeService.
fn build_session_end_properties(
    session: &CompletedSession,
    capture: &SessionCapture,
    config: &CaptureConfig,
) -> serde_json::Value {
    let mut properties = json!({
        "session_status": AiChatSessionStatus::Ended,
        "last_active": session.ended_at.to_rfc3339(),
        "exit_code": session.exit_status.code,
        "session_id": session.harness_session_id,
    });

    if saves_summary(config) {
        properties["summary"] = serde_json::Value::Null;
    }

    if config.enabled && config.content == CaptureContentSetting::Full {
        properties["transcript"] = json!(capture.transcript());
    }

    properties
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use nodespace_agent::pty::OutputChunk;
    use nodespace_core::models::AiChatPtyNode;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// The id a harness gave its conversation, as opposed to the PTY
    /// session's (`Uuid::nil()` in these tests).
    const HARNESS_SESSION_ID: &str = "0c5a8c1e-7d0b-4b5e-9f3a-2f1d6f0f8a01";

    fn make_session() -> CompletedSession {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        CompletedSession {
            id: Uuid::nil(),
            node_id: Some("ai-chat-node-1".to_string()),
            ended_at: ts,
            exit_status: ExitStatus {
                code: 0,
                success: true,
            },
            harness_session_id: Some(HARNESS_SESSION_ID.to_string()),
        }
    }

    /// A summarizer that answers with a fixed summary (or none) and records
    /// the text it was asked to summarize.
    struct RecordingSummarizer {
        summary: Option<&'static str>,
        asked: Mutex<Vec<String>>,
    }

    impl RecordingSummarizer {
        fn answering(summary: &'static str) -> Self {
            Self {
                summary: Some(summary),
                asked: Mutex::new(Vec::new()),
            }
        }

        /// No summarizer is available on this machine.
        fn unavailable() -> Self {
            Self {
                summary: None,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SessionSummarizer for RecordingSummarizer {
        async fn summarize(&self, plain_text: &str) -> Option<String> {
            self.asked.lock().unwrap().push(plain_text.to_string());
            self.summary.map(str::to_string)
        }
    }

    fn assert_no_control_sequences(text: &str) {
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
    }

    fn make_capture_with(text: &str) -> SessionCapture {
        let mut c = SessionCapture::new();
        c.push(OutputChunk {
            data: text.as_bytes().to_vec(),
            timestamp: Utc::now(),
        });
        c
    }

    fn capturing(content: CaptureContentSetting) -> CaptureConfig {
        CaptureConfig {
            enabled: true,
            content,
        }
    }

    fn not_capturing() -> CaptureConfig {
        CaptureConfig {
            enabled: false,
            content: CaptureContentSetting::Full,
        }
    }

    fn keys(properties: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = properties
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    async fn test_node_service(tmp: &tempfile::TempDir) -> Arc<CoreNodeService> {
        let mut store = Arc::new(
            nodespace_core::db::SqliteStore::new(tmp.path().join("capture.db"))
                .await
                .unwrap(),
        );
        Arc::new(CoreNodeService::new(&mut store).await.unwrap())
    }

    async fn create_chat(
        node_service: &Arc<CoreNodeService>,
        node_type: &str,
        agent: &str,
    ) -> String {
        node_service
            .create_node(nodespace_core::models::Node::new(
                node_type.to_string(),
                "Terminal session".to_string(),
                json!({ "agent": agent }),
            ))
            .await
            .unwrap()
    }

    /// The whole write, with capture off and at each content level: bare
    /// names only, and nothing that duplicates the node's own timestamps or
    /// agent.
    #[test]
    fn a_finished_session_writes_exactly_the_declared_bare_names() {
        let session = make_session();
        let capture = make_capture_with("hello world");
        let build =
            |config: &CaptureConfig| build_session_end_properties(&session, &capture, config);

        // Capture off withholds only the content. At the metadata level it
        // is on, and still saves none.
        let off = build(&not_capturing());
        assert_eq!(
            keys(&off),
            ["exit_code", "last_active", "session_id", "session_status"]
        );

        let metadata = build(&capturing(CaptureContentSetting::MetadataOnly));
        assert_eq!(
            keys(&metadata),
            ["exit_code", "last_active", "session_id", "session_status"]
        );

        let summary = build(&capturing(CaptureContentSetting::Summary));
        assert_eq!(
            keys(&summary),
            [
                "exit_code",
                "last_active",
                "session_id",
                "session_status",
                "summary"
            ]
        );

        let full = build(&capturing(CaptureContentSetting::Full));
        assert_eq!(
            keys(&full),
            [
                "exit_code",
                "last_active",
                "session_id",
                "session_status",
                "summary",
                "transcript"
            ]
        );
        assert_eq!(full["transcript"], "hello world");
        // The summary is derived after this write, never copied from output.
        assert_eq!(summary["summary"], serde_json::Value::Null);
        assert_eq!(full["summary"], serde_json::Value::Null);
        assert_eq!(full["exit_code"], 0);
        // The harness's id for the conversation, not the PTY session's.
        assert_eq!(full["session_id"], HARNESS_SESSION_ID);
        assert_ne!(full["session_id"], session.id.to_string());

        for properties in [&off, &metadata, &summary, &full] {
            // A finished session is `ended`. `archived` is governance's word.
            assert_eq!(properties["session_status"], "ended");
            for key in keys(properties) {
                assert!(!key.contains(':'), "'{key}' is prefixed");
            }
            for retired in ["started_at", "ended_at", "agent_type", "agent"] {
                assert!(properties.get(retired).is_none(), "'{retired}' is written");
            }
        }
    }

    /// Every key the write carries is declared by the `ai-chat-pty` schema
    /// chain, so the closed buckets accept it.
    #[test]
    fn every_key_is_declared_by_the_terminal_chats_schema_chain() {
        let props = build_session_end_properties(
            &make_session(),
            &make_capture_with("hello world"),
            &capturing(CaptureContentSetting::Full),
        );
        let chain: Vec<&str> = CoreNodeType::AiChatPty
            .chain()
            .into_iter()
            .map(CoreNodeType::as_str)
            .collect();
        let declared: Vec<String> = nodespace_core::models::core_schemas::get_core_schemas()
            .into_iter()
            .filter(|s| chain.contains(&s.id.as_str()))
            .flat_map(|s| s.fields)
            .map(|f| f.name)
            .collect();
        for key in keys(&props) {
            assert!(
                declared.iter().any(|d| d == key),
                "'{key}' is not declared by {chain:?}"
            );
        }
    }

    /// The backfill lands on a real terminal chat through the validated
    /// update: the path a finished terminal session takes.
    #[tokio::test]
    async fn finalize_capture_backfills_a_real_terminal_chat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "claude-code").await;
        let created = node_service.get_node(&node_id).await.unwrap().unwrap();

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        let capture = make_capture_with("hello world");
        let summarizer = RecordingSummarizer::answering("Greeted the world.");

        let backfilled = finalize_capture(
            &session,
            &capture,
            &node_service,
            &capturing(CaptureContentSetting::Full),
            &summarizer,
        )
        .await
        .expect("the backfill must be accepted");
        assert_eq!(backfilled.as_deref(), Some(node_id.as_str()));

        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert!(
            node.version > created.version,
            "the validated update advances the version"
        );
        // Each field sits in the bucket of the schema that declares it.
        assert_eq!(
            node.properties["ai-chat-pty"],
            json!({
                "session_status": "ended",
                "session_id": HARNESS_SESSION_ID,
                "transcript": "hello world",
                "exit_code": 0
            })
        );
        assert_eq!(
            node.properties["ai-chat"],
            json!({
                "agent": "claude-code",
                "summary": "Greeted the world.",
                "last_active": session.ended_at.to_rfc3339()
            })
        );

        let chat = AiChatPtyNode::from_node(node).unwrap();
        assert_eq!(chat.session_status, AiChatSessionStatus::Ended);
        assert_eq!(chat.base.agent, "claude-code", "set at launch, and kept");
        assert_eq!(chat.base.summary.as_deref(), Some("Greeted the world."));
        assert_eq!(chat.session_id.as_deref(), Some(HARNESS_SESSION_ID));
        assert_eq!(chat.exit_code, Some(0));
    }

    /// The summary is derived from what a reader of the terminal saw: the
    /// summarizer is handed no escape sequence, and the stored summary holds
    /// none. The transcript, which stays on the machine, keeps the stream as
    /// it was.
    #[tokio::test]
    async fn the_summary_of_a_session_with_escape_sequences_holds_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "claude-code").await;

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        let raw = "\x1b]0;claude\x07\x1b[1;32mEdited parser.rs\x1b[0m\r\n\x1b[2K\x1b[1A\x1b[38;5;208mAll 42 tests pass\x1b[0m\r\n";
        let summarizer = RecordingSummarizer::answering("Fixed the parser; its tests pass.");

        finalize_capture(
            &session,
            &make_capture_with(raw),
            &node_service,
            &capturing(CaptureContentSetting::Full),
            &summarizer,
        )
        .await
        .unwrap();

        assert_eq!(summarizer.asked(), ["Edited parser.rs\nAll 42 tests pass"]);
        let chat =
            AiChatPtyNode::from_node(node_service.get_node(&node_id).await.unwrap().unwrap())
                .unwrap();
        let summary = chat.base.summary.expect("a summary");
        assert_eq!(summary, "Fixed the parser; its tests pass.");
        assert_no_control_sequences(&summary);
        assert_eq!(chat.transcript.as_deref(), Some(raw));
    }

    /// A long session overflows the capture buffer, which then starts at an
    /// arbitrary point in the stream: here, inside a colour sequence. Nothing
    /// of that fragment reaches the summarizer.
    #[tokio::test]
    async fn the_summary_of_a_session_that_overflowed_the_buffer_holds_no_fragment() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "codex").await;

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        let mut capture = SessionCapture::with_max_bytes(64);
        for data in [
            &b"the start of the session, long gone\r\n\x1b[3"[..],
            &b"8;5;208mtail of a line\x1b[0m\r\n"[..],
            &b"\x1b[32mTests pass\x1b[0m\r\n"[..],
        ] {
            capture.push(OutputChunk {
                data: data.to_vec(),
                timestamp: Utc::now(),
            });
        }
        assert!(capture.overflowed());
        let summarizer = RecordingSummarizer::answering("The tests pass.");

        finalize_capture(
            &session,
            &capture,
            &node_service,
            &capturing(CaptureContentSetting::Summary),
            &summarizer,
        )
        .await
        .unwrap();

        assert_eq!(summarizer.asked(), ["Tests pass"]);
        let chat =
            AiChatPtyNode::from_node(node_service.get_node(&node_id).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(chat.base.summary.as_deref(), Some("The tests pass."));
        assert_eq!(chat.transcript, None, "the summary level saves no transcript");
    }

    /// With no summarizer on the machine the chat has no summary: never the
    /// session's output in its place, and not the summary a previous session
    /// on the same node left.
    #[tokio::test]
    async fn without_a_summarizer_the_chat_has_no_summary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "claude-code").await;

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        finalize_capture(
            &session,
            &make_capture_with("first session"),
            &node_service,
            &capturing(CaptureContentSetting::Summary),
            &RecordingSummarizer::answering("The first session."),
        )
        .await
        .unwrap();

        finalize_capture(
            &session,
            &make_capture_with("\x1b[31msecond session\x1b[0m"),
            &node_service,
            &capturing(CaptureContentSetting::Summary),
            &RecordingSummarizer::unavailable(),
        )
        .await
        .unwrap();

        // A cleared field is stored null and read as absent.
        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.properties["ai-chat"]["summary"], serde_json::Value::Null);
        let typed = nodespace_core::models::node_to_typed_value(node.clone()).unwrap();
        assert!(typed.get("summary").is_none(), "{typed}");
        assert_eq!(AiChatPtyNode::from_node(node).unwrap().base.summary, None);
    }

    /// Below the summary level nothing is summarized: the summarizer is not
    /// asked, and the session's output goes nowhere.
    #[tokio::test]
    async fn a_session_is_not_summarized_below_the_summary_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;

        for config in [
            not_capturing(),
            capturing(CaptureContentSetting::MetadataOnly),
        ] {
            let node_id = create_chat(&node_service, "ai-chat-pty", "claude-code").await;
            let mut session = make_session();
            session.node_id = Some(node_id.clone());
            let summarizer = RecordingSummarizer::answering("Should not be asked.");

            finalize_capture(
                &session,
                &make_capture_with("some output"),
                &node_service,
                &config,
                &summarizer,
            )
            .await
            .unwrap();

            assert!(summarizer.asked().is_empty());
            let chat =
                AiChatPtyNode::from_node(node_service.get_node(&node_id).await.unwrap().unwrap())
                    .unwrap();
            assert_eq!(chat.base.summary, None);
        }
    }

    /// The stored session id is the one the harness recorded in its own
    /// session store, for each harness that has one.
    #[tokio::test]
    async fn the_harnesss_own_session_id_is_stored_for_claude_code_and_codex() {
        use nodespace_agent::agent_types::AgentType;
        use nodespace_agent::pty::find_harness_session_id;

        const CODEX_SESSION_ID: &str = "7b1e2a44-3c9d-4f6a-8e21-5a0c9d3e7b02";

        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;

        // A home directory holding each harness's session store, with one
        // conversation recorded for the session's working directory.
        let home = tmp.path().canonicalize().unwrap().join("home");
        let session_dir = home.join(".nodespace").join("agent-sessions").join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let claude_project: String = session_dir
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let claude_dir = home.join(".claude").join("projects").join(claude_project);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(format!("{HARNESS_SESSION_ID}.jsonl")),
            "{}\n",
        )
        .unwrap();
        let codex_dir = home.join(".codex").join("sessions").join("2026").join("01").join("01");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join(format!("rollout-2026-01-01T11-00-00-{CODEX_SESSION_ID}.jsonl")),
            format!(
                "{}\n",
                json!({
                    "type": "session_meta",
                    "payload": { "id": CODEX_SESSION_ID, "cwd": session_dir }
                })
            ),
        )
        .unwrap();
        let started_at = Utc::now() - chrono::Duration::minutes(5);

        for (agent_type, agent, expected) in [
            (AgentType::ClaudeCode, "claude-code", HARNESS_SESSION_ID),
            (AgentType::Codex, "codex", CODEX_SESSION_ID),
        ] {
            let node_id = create_chat(&node_service, "ai-chat-pty", agent).await;
            let mut session = make_session();
            session.node_id = Some(node_id.clone());
            session.harness_session_id =
                find_harness_session_id(agent_type, &home, &session_dir, started_at);

            finalize_capture(
                &session,
                &SessionCapture::new(),
                &node_service,
                &not_capturing(),
                &RecordingSummarizer::unavailable(),
            )
            .await
            .unwrap();

            let node = node_service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node.properties["ai-chat-pty"]["session_id"], expected);
        }

        // The field the id is stored in never leaves the machine.
        let session_id_field = nodespace_core::models::core_schemas::get_core_schemas()
            .into_iter()
            .find(|schema| schema.id == CoreNodeType::AiChatPty.as_str())
            .and_then(|schema| schema.fields.into_iter().find(|f| f.name == "session_id"))
            .expect("ai-chat-pty declares session_id");
        assert!(session_id_field.local_only);
    }

    /// A harness that recorded no conversation leaves no id, and clears the
    /// one a previous session on the same node left: that id would resume
    /// the wrong conversation.
    #[tokio::test]
    async fn a_session_without_a_harness_id_clears_the_previous_one() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "claude-code").await;
        let finalize = |session: CompletedSession| {
            let node_service = node_service.clone();
            async move {
                finalize_capture(
                    &session,
                    &SessionCapture::new(),
                    &node_service,
                    &not_capturing(),
                    &RecordingSummarizer::unavailable(),
                )
                .await
                .unwrap();
            }
        };

        let mut first = make_session();
        first.node_id = Some(node_id.clone());
        finalize(first).await;
        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(
            node.properties["ai-chat-pty"]["session_id"],
            HARNESS_SESSION_ID
        );

        let mut second = make_session();
        second.node_id = Some(node_id.clone());
        second.harness_session_id = None;
        finalize(second).await;
        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        let typed = nodespace_core::models::node_to_typed_value(node.clone()).unwrap();
        assert!(typed.get("sessionId").is_none(), "{typed}");
        assert_eq!(AiChatPtyNode::from_node(node).unwrap().session_id, None);
    }

    /// With capture off, the session still ends on its node: only the
    /// session's content is withheld.
    #[tokio::test]
    async fn a_session_ends_on_its_node_even_when_capture_is_off() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "codex").await;

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        session.exit_status = ExitStatus {
            code: 130,
            success: false,
        };

        let written = finalize_capture(
            &session,
            &make_capture_with("secret output"),
            &node_service,
            &not_capturing(),
            &RecordingSummarizer::unavailable(),
        )
        .await
        .unwrap();
        assert_eq!(written.as_deref(), Some(node_id.as_str()));

        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(
            node.properties["ai-chat-pty"],
            json!({
                "session_status": "ended",
                "session_id": HARNESS_SESSION_ID,
                "exit_code": 130
            })
        );
        let chat = AiChatPtyNode::from_node(node).unwrap();
        assert_eq!(chat.session_status, AiChatSessionStatus::Ended);
        assert_eq!(chat.transcript, None);
        assert_eq!(chat.base.summary, None);
    }

    /// Only a terminal chat is written. A native chat is refused, and so is a
    /// node of a user-defined type, whose open schema would otherwise take
    /// the session fields; each is left as it was.
    #[tokio::test]
    async fn finalize_capture_refuses_a_node_that_is_not_a_terminal_chat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        nodespace_core::schema::handle_create_schema(
            &node_service,
            json!({ "name": "Invoice", "fields": [] }),
        )
        .await
        .expect("a user-defined type");
        let invoice = node_service
            .create_node(nodespace_core::models::Node::new(
                "invoice".to_string(),
                "INV-1".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        let native = create_chat(&node_service, "ai-chat-native", "nodespace").await;

        for (node_id, node_type) in [(native, "ai-chat-native"), (invoice, "invoice")] {
            let before = node_service.get_node(&node_id).await.unwrap().unwrap();

            let mut session = make_session();
            session.node_id = Some(node_id.clone());

            let error = finalize_capture(
                &session,
                &SessionCapture::new(),
                &node_service,
                &not_capturing(),
                &RecordingSummarizer::unavailable(),
            )
            .await
            .expect_err("only a terminal chat is written");
            assert!(
                error
                    .to_string()
                    .contains(&format!("is a '{node_type}', not a terminal chat")),
                "{error}"
            );

            let after = node_service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(after.version, before.version, "{node_type}");
            assert_eq!(after.properties, before.properties, "{node_type}");
        }
    }

    /// A type extending `ai-chat-pty` is a terminal chat, and its session ends
    /// on it like any other.
    #[tokio::test]
    async fn a_type_extending_the_terminal_chat_is_written() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        nodespace_core::schema::handle_create_schema(
            &node_service,
            json!({ "name": "Pairing Session", "extends": "ai-chat-pty", "fields": [] }),
        )
        .await
        .expect("a subtype of the terminal chat");
        let node_id = create_chat(&node_service, "pairing_session", "codex").await;

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        finalize_capture(
            &session,
            &SessionCapture::new(),
            &node_service,
            &not_capturing(),
            &RecordingSummarizer::unavailable(),
        )
        .await
        .expect("a subtype of the terminal chat is a terminal chat");

        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.properties["ai-chat-pty"]["session_status"], "ended");
    }

    /// A viewer renames the chat between the read of its version and the
    /// write. The write is retried at the fresh version: the session still
    /// ends on the node, and the rename survives.
    #[tokio::test]
    async fn a_version_conflict_is_retried_and_the_other_write_survives() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;
        let node_id = create_chat(&node_service, "ai-chat-pty", "codex").await;

        let stale = node_service
            .get_node(&node_id)
            .await
            .unwrap()
            .unwrap()
            .version;
        node_service
            .update_node(
                &node_id,
                stale,
                NodeUpdate::new().with_content("Renamed meanwhile".to_string()),
            )
            .await
            .unwrap();

        let mut session = make_session();
        session.node_id = Some(node_id.clone());
        let properties =
            build_session_end_properties(&session, &SessionCapture::new(), &not_capturing());

        // The first read hands back the version the rename has overtaken.
        let reads = AtomicUsize::new(0);
        write_at_current_version(&node_service, &node_id, properties, || {
            let first = reads.fetch_add(1, Ordering::SeqCst) == 0;
            let node_service = node_service.clone();
            let node_id = node_id.clone();
            async move {
                if first {
                    Ok(stale)
                } else {
                    current_version(&node_service, &node_id).await
                }
            }
        })
        .await
        .expect("the conflict is retried");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "one conflict, then one write at the fresh version"
        );

        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.content, "Renamed meanwhile");
        assert_eq!(node.properties["ai-chat-pty"]["session_status"], "ended");
    }

    #[tokio::test]
    async fn nothing_is_written_for_a_session_without_a_node() {
        let tmp = tempfile::TempDir::new().unwrap();
        let node_service = test_node_service(&tmp).await;

        let mut session = make_session();
        session.node_id = None;
        let result = finalize_capture(
            &session,
            &SessionCapture::new(),
            &node_service,
            &capturing(CaptureContentSetting::Full),
            &RecordingSummarizer::unavailable(),
        )
        .await
        .unwrap();
        assert_eq!(result, None);
    }
}
