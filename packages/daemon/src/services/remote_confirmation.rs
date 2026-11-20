//! The daemon's side of asking the user to confirm a remote-tool call.
//!
//! A remote tool whose calls need confirmation (every `tool-cli` and `tool-mcp`
//! call, and `tool-rest` calls that are not a bare `GET`) is offered only when a
//! [`CallConfirmer`] is installed. This module is that confirmer
//! ([`DaemonCallConfirmer`]), the table of questions waiting for an answer
//! ([`PendingConfirmations`]) and the answer entry point.
//!
//! **Not installed yet.** Nothing here is wired into a turn: the card that shows
//! the question and records the answer is a [`ConfirmationUi`] a later change
//! supplies, so every remote tool that needs confirmation is still not offered.
//!
//! The rules, each one tested:
//!
//! - **Default no.** Only an explicit yes for THIS question runs the call. A
//!   timeout, a cancelled turn, a database shutdown, a card that could not be
//!   shown and a dropped future are all no.
//! - **One question, one id.** Each question gets a random 244-bit id (two UUIDv4s,
//!   from the OS random source), unguessable and never reused. The entry binds
//!   the chat, the turn, the tool name and the digest of what the user was shown
//!   ([`nodespace_agent::local_agent::remote_tools::confirmation_digest`]).
//!   Calls in one tool-loop round each get their own id, so one yes cannot cover
//!   two.
//! - **An answer must match exactly:** the id, the chat and the digest. The digest
//!   is mandatory, for a no as well as a yes. A wrong chat, a wrong digest or an
//!   unknown id is refused and leaves the question pending (a stray answer cannot
//!   cancel it). A refusal says nothing about why.
//! - **Single use, and no late yes.** The first valid answer REMOVES the entry,
//!   under the same lock, and delivers the answer while holding it. A timeout,
//!   cancel, close or shutdown removes the entry BEFORE it does anything else, so
//!   an answer that arrives afterwards finds nothing. An answer that won the race
//!   by a moment is honoured (it was accepted).
//! - **At most [`MAX_PENDING_PER_CHAT`] questions per chat at once**; another is
//!   a no without being shown.
//! - **Not reachable from a tool.** The answer is a gRPC call on the daemon's
//!   same-user socket. A program the daemon spawned for a tool call (cli, or an
//!   MCP server) runs as the same user and could dial that socket, so
//!   [`DaemonTreeGate`] refuses an answer from any peer process that is the
//!   daemon or one of its descendants, and from a peer whose identity cannot be
//!   read (fail closed; that includes platforms where the transport carries no
//!   peer credentials). A process that detaches from the daemon's tree is
//!   indistinguishable from any other program of the same user: this narrows
//!   the channel, it is not a sandbox.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nodespace_agent::local_agent::remote_tools::{CallConfirmer, ConfirmationRequest};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// How long a question waits for an answer before it is a no.
pub const DEFAULT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(120);
/// Most questions waiting at once in one chat.
pub const MAX_PENDING_PER_CHAT: usize = 4;
/// How long the card may take to record how a question ended.
const RESOLVE_LIMIT: Duration = Duration::from_secs(5);

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationOutcome {
    Allowed,
    Denied,
    /// No answer in time.
    Expired,
    /// The turn was cancelled, the database shut down, or the question could not
    /// be shown.
    Cancelled,
}

/// A question as it is put to the user.
#[derive(Debug, Clone)]
pub struct ConfirmationAsk {
    pub request_id: String,
    pub chat_node_id: String,
    pub turn_id: String,
    pub tool_name: String,
    /// `tool-cli`, `tool-mcp` or `tool-rest`.
    pub kind: String,
    /// Credentials first, then where the call goes.
    pub summary: String,
    pub arguments_json: String,
    pub arguments_bytes: usize,
    pub digest: String,
}

/// Where a question is shown and recorded: the chat card.
#[async_trait]
pub trait ConfirmationUi: Send + Sync {
    /// Shows the question. `false` means it could not be shown, which is a no.
    async fn present(&self, ask: &ConfirmationAsk) -> bool;
    /// Records how the question ended (best effort).
    async fn resolved(&self, ask: &ConfirmationAsk, outcome: ConfirmationOutcome);
}

struct Pending {
    chat_node_id: String,
    turn_id: String,
    digest: String,
    tx: oneshot::Sender<bool>,
}

/// An answer to a question.
#[derive(Debug, Clone)]
pub struct Answer {
    pub request_id: String,
    pub chat_node_id: String,
    pub digest: String,
    pub allow: bool,
}

/// Why an answer was refused. Internal: a caller sees only that it was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerResult {
    Accepted,
    /// No such question: never asked, already answered, or no longer waiting.
    Unknown,
    /// The question belongs to another chat.
    ForeignChat,
    /// The digest is not the one of what the user was shown.
    DigestMismatch,
}

/// The questions waiting for an answer, for one database.
#[derive(Clone, Default)]
pub struct PendingConfirmations {
    table: Arc<Mutex<HashMap<String, Pending>>>,
}

fn same(a: &str, b: &str) -> bool {
    // Constant time in the length of the shorter, so a guess learns nothing from
    // timing. The ids and digests are public-length hex.
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn new_request_id() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

impl PendingConfirmations {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        // A poisoned table is still a table: the entries are plain data.
        self.table.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// How many questions wait.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Answers a question. The entry is removed and the answer delivered under
    /// one lock, so a question is answered at most once and a removal by a
    /// timeout or cancel cannot interleave.
    pub fn answer(&self, answer: &Answer) -> AnswerResult {
        let mut table = self.lock();
        let Some(pending) = table.get(&answer.request_id) else {
            return AnswerResult::Unknown;
        };
        if !same(&pending.chat_node_id, &answer.chat_node_id) {
            return AnswerResult::ForeignChat;
        }
        if answer.digest.is_empty() || !same(&pending.digest, &answer.digest) {
            return AnswerResult::DigestMismatch;
        }
        let pending = table.remove(&answer.request_id).expect("checked above");
        // The receiver may be gone (the question just ended); that is not an
        // acceptance anyone relies on.
        match pending.tx.send(answer.allow) {
            Ok(()) => AnswerResult::Accepted,
            Err(_) => AnswerResult::Unknown,
        }
    }

    /// Removes every question the turn asked and has not had answered. Called
    /// when the turn ends.
    pub fn clear_turn(&self, chat_node_id: &str, turn_id: &str) {
        self.lock()
            .retain(|_, p| !(p.chat_node_id == chat_node_id && p.turn_id == turn_id));
    }

    fn remove(&self, request_id: &str) -> bool {
        self.lock().remove(request_id).is_some()
    }
}

/// Removes its entry when dropped, so a question never outlives the await that
/// asked it (a turn dropped mid-question included).
struct EntryGuard {
    pending: PendingConfirmations,
    request_id: String,
}

impl Drop for EntryGuard {
    fn drop(&mut self) {
        self.pending.remove(&self.request_id);
    }
}

/// The confirmer one turn uses.
pub struct DaemonCallConfirmer {
    pending: PendingConfirmations,
    ui: Arc<dyn ConfirmationUi>,
    chat_node_id: String,
    turn_id: String,
    turn_cancel: CancellationToken,
    shutdown: CancellationToken,
    timeout: Duration,
}

impl DaemonCallConfirmer {
    pub fn new(
        pending: PendingConfirmations,
        ui: Arc<dyn ConfirmationUi>,
        chat_node_id: String,
        turn_id: String,
        turn_cancel: CancellationToken,
        shutdown: CancellationToken,
        timeout: Duration,
    ) -> Self {
        Self {
            pending,
            ui,
            chat_node_id,
            turn_id,
            turn_cancel,
            shutdown,
            timeout,
        }
    }
}

#[async_trait]
impl CallConfirmer for DaemonCallConfirmer {
    async fn confirm(&self, request: &ConfirmationRequest) -> bool {
        // An answer is bound to the digest, so a request without one cannot be
        // asked.
        if request.digest.is_empty() {
            return false;
        }
        let request_id = new_request_id();
        let (tx, mut rx) = oneshot::channel();
        {
            let mut table = self.pending.lock();
            let waiting = table
                .values()
                .filter(|p| p.chat_node_id == self.chat_node_id)
                .count();
            if waiting >= MAX_PENDING_PER_CHAT {
                return false;
            }
            table.insert(
                request_id.clone(),
                Pending {
                    chat_node_id: self.chat_node_id.clone(),
                    turn_id: self.turn_id.clone(),
                    digest: request.digest.clone(),
                    tx,
                },
            );
        }
        let _guard = EntryGuard {
            pending: self.pending.clone(),
            request_id: request_id.clone(),
        };
        let ask = ConfirmationAsk {
            request_id: request_id.clone(),
            chat_node_id: self.chat_node_id.clone(),
            turn_id: self.turn_id.clone(),
            tool_name: request.tool_name.clone(),
            kind: request.kind.bucket().to_string(),
            summary: request.destination.clone(),
            arguments_json: request.arguments_json.clone(),
            arguments_bytes: request.arguments_bytes,
            digest: request.digest.clone(),
        };
        if !self.ui.present(&ask).await {
            // Nothing was asked: the entry goes (guard) and the call is a no.
            return false;
        }
        let waited = tokio::select! {
            answer = &mut rx => match answer {
                Ok(true) => ConfirmationOutcome::Allowed,
                Ok(false) => ConfirmationOutcome::Denied,
                Err(_) => ConfirmationOutcome::Cancelled,
            },
            _ = tokio::time::sleep(self.timeout) => ConfirmationOutcome::Expired,
            _ = self.turn_cancel.cancelled() => ConfirmationOutcome::Cancelled,
            _ = self.shutdown.cancelled() => ConfirmationOutcome::Cancelled,
        };
        // The entry goes FIRST: from here a late answer finds nothing. If it was
        // already gone, an answer took it, and that answer stands.
        let outcome = if self.pending.remove(&request_id) {
            waited
        } else {
            match rx.try_recv() {
                Ok(true) => ConfirmationOutcome::Allowed,
                Ok(false) => ConfirmationOutcome::Denied,
                Err(_) => waited,
            }
        };
        let _ = tokio::time::timeout(RESOLVE_LIMIT, self.ui.resolved(&ask, outcome)).await;
        outcome == ConfirmationOutcome::Allowed
    }
}

// ---------------------------------------------------------------------------
// Who may answer
// ---------------------------------------------------------------------------

/// Decides whether a peer of the daemon's socket may answer a question.
pub trait AnswerGate: Send + Sync {
    /// `peer_pid` is the process id the kernel reports for the connection, when
    /// the transport has one.
    fn may_answer(&self, peer_pid: Option<u32>) -> bool;
}

/// Refuses the daemon itself and every process descended from it (the programs
/// it runs for tool calls), and any peer it cannot identify.
pub struct DaemonTreeGate {
    daemon_pid: u32,
}

impl DaemonTreeGate {
    pub fn for_this_process() -> Self {
        Self {
            daemon_pid: std::process::id(),
        }
    }

    pub fn for_pid(daemon_pid: u32) -> Self {
        Self { daemon_pid }
    }
}

impl AnswerGate for DaemonTreeGate {
    fn may_answer(&self, peer_pid: Option<u32>) -> bool {
        match peer_pid.map(|pid| is_descendant_of(pid, self.daemon_pid)) {
            Some(Some(false)) => true,
            // A descendant, or a chain that cannot be read: refused.
            _ => false,
        }
    }
}

/// Whether `pid` is `ancestor` or descends from it. `None` when the chain cannot
/// be read.
pub fn is_descendant_of(pid: u32, ancestor: u32) -> Option<bool> {
    is_descendant_with(pid, ancestor, parent_pid)
}

/// [`is_descendant_of`] over a supplied parent lookup, so the walk can be driven
/// with a made-up process table.
fn is_descendant_with(
    pid: u32,
    ancestor: u32,
    parent_pid: impl Fn(u32) -> Option<u32>,
) -> Option<bool> {
    let mut current = pid;
    // A process tree is shallow; the bound only stops a loop in bad data.
    for _ in 0..256 {
        if current == ancestor {
            return Some(true);
        }
        if current <= 1 {
            return Some(false);
        }
        current = parent_pid(current)?;
    }
    None
}

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (comm) state ppid ...": comm may hold spaces and parentheses, so the
    // fields are read after the LAST closing parenthesis.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: u32) -> Option<u32> {
    // SAFETY: proc_pidinfo writes at most `size` bytes into the struct we own.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        (written == size).then_some(info.pbi_ppid)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_agent::local_agent::remote_tools::{confirmation_digest, RemoteToolKind};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeUi {
        shown: Mutex<Vec<ConfirmationAsk>>,
        resolved: Mutex<Vec<(String, ConfirmationOutcome)>>,
        refuse_to_show: bool,
        presented: AtomicUsize,
    }

    #[async_trait]
    impl ConfirmationUi for FakeUi {
        async fn present(&self, ask: &ConfirmationAsk) -> bool {
            self.presented.fetch_add(1, Ordering::SeqCst);
            if self.refuse_to_show {
                return false;
            }
            self.shown.lock().unwrap().push(ask.clone());
            true
        }
        async fn resolved(&self, ask: &ConfirmationAsk, outcome: ConfirmationOutcome) {
            self.resolved
                .lock()
                .unwrap()
                .push((ask.request_id.clone(), outcome));
        }
    }

    fn request(name: &str) -> ConfirmationRequest {
        let arguments = json!({"q": name});
        let arguments_json = serde_json::to_string(&arguments).unwrap();
        let destination = format!("run `{name}`");
        ConfirmationRequest {
            tool_name: name.to_string(),
            kind: RemoteToolKind::Cli,
            digest: confirmation_digest(&destination, &arguments_json),
            destination,
            arguments,
            arguments_bytes: arguments_json.len(),
            arguments_json,
        }
    }

    struct Rig {
        pending: PendingConfirmations,
        ui: Arc<FakeUi>,
        cancel: CancellationToken,
        shutdown: CancellationToken,
    }

    impl Rig {
        fn new() -> Self {
            Self::with_ui(FakeUi::default())
        }
        fn with_ui(ui: FakeUi) -> Self {
            Self {
                pending: PendingConfirmations::default(),
                ui: Arc::new(ui),
                cancel: CancellationToken::new(),
                shutdown: CancellationToken::new(),
            }
        }
        fn confirmer(&self, chat: &str, turn: &str, timeout: Duration) -> Arc<DaemonCallConfirmer> {
            Arc::new(DaemonCallConfirmer::new(
                self.pending.clone(),
                self.ui.clone(),
                chat.to_string(),
                turn.to_string(),
                self.cancel.clone(),
                self.shutdown.clone(),
                timeout,
            ))
        }
        /// Waits until `n` questions have been shown, returning them.
        async fn shown(&self, n: usize) -> Vec<ConfirmationAsk> {
            for _ in 0..500 {
                {
                    let shown = self.ui.shown.lock().unwrap();
                    if shown.len() >= n {
                        return shown.clone();
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("the question was not shown");
        }
        fn answer(&self, ask: &ConfirmationAsk, allow: bool) -> AnswerResult {
            self.pending.answer(&Answer {
                request_id: ask.request_id.clone(),
                chat_node_id: ask.chat_node_id.clone(),
                digest: ask.digest.clone(),
                allow,
            })
        }
    }

    const LONG: Duration = Duration::from_secs(30);

    #[tokio::test]
    async fn a_yes_for_the_exact_question_allows_once() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        assert_eq!(rig.answer(&ask, true), AnswerResult::Accepted);
        // Single use: the same answer again, at once, finds nothing.
        assert_eq!(rig.answer(&ask, true), AnswerResult::Unknown);
        assert!(task.await.unwrap());
        assert_eq!(
            rig.ui.resolved.lock().unwrap()[0].1,
            ConfirmationOutcome::Allowed
        );
        assert!(rig.pending.is_empty());
    }

    #[tokio::test]
    async fn a_no_denies() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        assert_eq!(rig.answer(&ask, false), AnswerResult::Accepted);
        assert!(!task.await.unwrap());
        assert_eq!(
            rig.ui.resolved.lock().unwrap()[0].1,
            ConfirmationOutcome::Denied
        );
    }

    #[tokio::test]
    async fn the_asked_text_and_the_ask_carry_what_the_request_carried() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let req = request("a");
        let task = tokio::spawn({
            let c = c.clone();
            let req = req.clone();
            async move { c.confirm(&req).await }
        });
        let ask = rig.shown(1).await.remove(0);
        assert_eq!(ask.summary, req.destination);
        assert_eq!(ask.arguments_json, req.arguments_json);
        assert_eq!(ask.digest, req.digest);
        assert_eq!(ask.tool_name, "a");
        assert_eq!(ask.kind, "tool-cli");
        assert_eq!(ask.chat_node_id, "chat");
        assert_eq!(ask.turn_id, "turn");
        rig.answer(&ask, false);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_timeout_is_a_no_and_a_late_yes_finds_nothing() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", Duration::from_millis(50));
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        assert!(!task.await.unwrap());
        assert!(
            rig.pending.is_empty(),
            "the entry is gone before anyone can answer"
        );
        assert_eq!(rig.answer(&ask, true), AnswerResult::Unknown);
        assert_eq!(
            rig.ui.resolved.lock().unwrap()[0].1,
            ConfirmationOutcome::Expired
        );
    }

    #[tokio::test]
    async fn a_cancelled_turn_is_a_no_and_a_late_yes_finds_nothing() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        rig.cancel.cancel();
        assert!(!task.await.unwrap());
        assert_eq!(rig.answer(&ask, true), AnswerResult::Unknown);
        assert_eq!(
            rig.ui.resolved.lock().unwrap()[0].1,
            ConfirmationOutcome::Cancelled
        );
    }

    #[tokio::test]
    async fn a_shutdown_is_a_no_and_a_late_yes_finds_nothing() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        rig.shutdown.cancel();
        assert!(!task.await.unwrap());
        assert_eq!(rig.answer(&ask, true), AnswerResult::Unknown);
    }

    #[tokio::test]
    async fn a_dropped_question_leaves_no_entry() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        task.abort();
        let _ = task.await;
        assert!(rig.pending.is_empty());
        assert_eq!(rig.answer(&ask, true), AnswerResult::Unknown);
    }

    #[tokio::test]
    async fn a_card_that_cannot_be_shown_is_a_no_and_leaves_no_entry() {
        let rig = Rig::with_ui(FakeUi {
            refuse_to_show: true,
            ..Default::default()
        });
        let c = rig.confirmer("chat", "turn", LONG);
        assert!(!c.confirm(&request("a")).await);
        assert!(rig.pending.is_empty());
    }

    #[tokio::test]
    async fn a_wrong_chat_a_wrong_digest_and_an_unknown_id_are_refused_and_the_question_stays() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        let mut other_chat = Answer {
            request_id: ask.request_id.clone(),
            chat_node_id: "another".into(),
            digest: ask.digest.clone(),
            allow: true,
        };
        assert_eq!(rig.pending.answer(&other_chat), AnswerResult::ForeignChat);
        other_chat.chat_node_id = "chat".into();
        other_chat.digest = confirmation_digest("something else", "{}");
        assert_eq!(
            rig.pending.answer(&other_chat),
            AnswerResult::DigestMismatch
        );
        // The digest is mandatory.
        other_chat.digest = String::new();
        assert_eq!(
            rig.pending.answer(&other_chat),
            AnswerResult::DigestMismatch
        );
        other_chat.digest = ask.digest.clone();
        other_chat.request_id = "0".repeat(64);
        assert_eq!(rig.pending.answer(&other_chat), AnswerResult::Unknown);
        assert_eq!(rig.pending.len(), 1, "none of those cancelled the question");
        assert_eq!(rig.answer(&ask, true), AnswerResult::Accepted);
        assert!(task.await.unwrap());
    }

    #[tokio::test]
    async fn a_no_also_needs_the_digest() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        let refused = rig.pending.answer(&Answer {
            request_id: ask.request_id.clone(),
            chat_node_id: "chat".into(),
            digest: "wrong".into(),
            allow: false,
        });
        assert_eq!(refused, AnswerResult::DigestMismatch);
        rig.answer(&ask, false);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn two_calls_in_one_round_get_their_own_ids_and_one_yes_covers_one() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let first = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let second = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("b")).await }
        });
        let asks = rig.shown(2).await;
        assert_ne!(asks[0].request_id, asks[1].request_id);
        assert!(asks[0].request_id.len() >= 64);
        // Both halves are random, not just the tail.
        assert_ne!(asks[0].request_id[..32], asks[1].request_id[..32]);
        assert_ne!(asks[0].request_id[32..], asks[1].request_id[32..]);
        assert!(asks[0].request_id.chars().all(|c| c.is_ascii_hexdigit()));
        let a = asks.iter().find(|a| a.tool_name == "a").unwrap();
        let b = asks.iter().find(|a| a.tool_name == "b").unwrap();
        // The yes for `a` carried with `b`'s id is refused: digests differ.
        let crossed = Answer {
            request_id: b.request_id.clone(),
            chat_node_id: "chat".into(),
            digest: a.digest.clone(),
            allow: true,
        };
        assert_eq!(rig.pending.answer(&crossed), AnswerResult::DigestMismatch);
        assert_eq!(rig.answer(a, true), AnswerResult::Accepted);
        assert!(first.await.unwrap());
        assert_eq!(rig.pending.len(), 1, "b is still waiting");
        assert_eq!(rig.answer(b, false), AnswerResult::Accepted);
        assert!(!second.await.unwrap());
    }

    #[tokio::test]
    async fn the_same_call_asked_twice_gets_two_ids() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let t1 = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let t2 = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let asks = rig.shown(2).await;
        assert_eq!(asks[0].digest, asks[1].digest);
        assert_ne!(asks[0].request_id, asks[1].request_id);
        // A yes to one does not allow the other.
        rig.answer(&asks[0], true);
        rig.answer(&asks[1], false);
        let (r1, r2) = (t1.await.unwrap(), t2.await.unwrap());
        assert_ne!(r1, r2);
    }

    #[tokio::test]
    async fn at_most_four_questions_wait_in_one_chat() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let mut tasks = Vec::new();
        for i in 0..MAX_PENDING_PER_CHAT {
            let c = c.clone();
            tasks.push(tokio::spawn(async move {
                c.confirm(&request(&format!("t{i}"))).await
            }));
        }
        rig.shown(MAX_PENDING_PER_CHAT).await;
        let before = rig.ui.presented.load(Ordering::SeqCst);
        assert!(!c.confirm(&request("extra")).await);
        assert_eq!(
            rig.ui.presented.load(Ordering::SeqCst),
            before,
            "not even shown"
        );
        // Another chat is not held up by this one.
        let other = rig.confirmer("chat-2", "turn", LONG);
        let t = tokio::spawn(async move { other.confirm(&request("x")).await });
        rig.shown(MAX_PENDING_PER_CHAT + 1).await;
        rig.cancel.cancel();
        for task in tasks {
            assert!(!task.await.unwrap());
        }
        assert!(!t.await.unwrap());
    }

    #[tokio::test]
    async fn clearing_a_turn_removes_only_its_questions() {
        let rig = Rig::new();
        let mine = rig.confirmer("chat", "turn-1", LONG);
        let theirs = rig.confirmer("chat", "turn-2", LONG);
        let t1 = tokio::spawn(async move { mine.confirm(&request("a")).await });
        let t2 = tokio::spawn(async move { theirs.confirm(&request("b")).await });
        let asks = rig.shown(2).await;
        rig.pending.clear_turn("chat", "turn-1");
        assert_eq!(rig.pending.len(), 1);
        let kept = asks.iter().find(|a| a.turn_id == "turn-2").unwrap();
        assert_eq!(rig.answer(kept, true), AnswerResult::Accepted);
        assert!(t2.await.unwrap());
        t1.abort();
    }

    #[tokio::test]
    async fn an_answer_taken_in_the_same_instant_as_a_cancel_stands() {
        // Both are ready when the confirmer wakes; it must honour the answer
        // that was accepted, whichever it looks at first.
        for _ in 0..40 {
            let rig = Rig::new();
            let c = rig.confirmer("chat", "turn", LONG);
            let task = tokio::spawn({
                let c = c.clone();
                async move { c.confirm(&request("a")).await }
            });
            let ask = rig.shown(1).await.remove(0);
            assert_eq!(rig.answer(&ask, true), AnswerResult::Accepted);
            rig.cancel.cancel();
            assert!(task.await.unwrap(), "an accepted yes was dropped");
        }
    }

    #[tokio::test]
    async fn a_request_with_no_digest_is_not_asked() {
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", LONG);
        let mut req = request("a");
        req.digest = String::new();
        assert!(!c.confirm(&req).await);
        assert_eq!(rig.ui.presented.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_answer_that_won_the_race_with_a_timeout_stands() {
        // The timeout fires only after the answer was taken: the confirmer sees
        // its entry already gone and reads the delivered answer.
        let rig = Rig::new();
        let c = rig.confirmer("chat", "turn", Duration::from_millis(80));
        let task = tokio::spawn({
            let c = c.clone();
            async move { c.confirm(&request("a")).await }
        });
        let ask = rig.shown(1).await.remove(0);
        assert_eq!(rig.answer(&ask, true), AnswerResult::Accepted);
        assert!(task.await.unwrap());
    }

    // -- who may answer ------------------------------------------------------

    #[test]
    fn a_peer_is_refused_when_it_is_the_daemon_or_descends_from_it_or_is_unknown() {
        let me = std::process::id();
        let gate = DaemonTreeGate::for_pid(me);
        assert!(!gate.may_answer(None), "unknown peers fail closed");
        assert!(!gate.may_answer(Some(me)), "the daemon itself");
        #[cfg(unix)]
        {
            let mut child = std::process::Command::new("sleep")
                .arg("5")
                .spawn()
                .unwrap();
            let pid = child.id();
            assert_eq!(is_descendant_of(pid, me), Some(true));
            assert!(!gate.may_answer(Some(pid)), "a program the daemon started");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[test]
    fn a_peer_outside_the_daemons_tree_may_answer_and_an_unreadable_chain_may_not() {
        // A made-up table: 500 -> 400 -> 300 -> 1, and 900 -> 1.
        let parents = |pid: u32| match pid {
            500 => Some(400),
            400 => Some(300),
            300 => Some(1),
            900 => Some(1),
            _ => None,
        };
        assert_eq!(is_descendant_with(500, 300, parents), Some(true));
        assert_eq!(is_descendant_with(300, 300, parents), Some(true));
        assert_eq!(is_descendant_with(900, 300, parents), Some(false));
        assert_eq!(is_descendant_with(300, 500, parents), Some(false));
        // A link that cannot be read: not known, so not allowed.
        assert_eq!(is_descendant_with(700, 300, parents), None);
        assert_eq!(is_descendant_with(1, 300, parents), Some(false));
        // A loop in the data ends instead of spinning.
        assert_eq!(
            is_descendant_with(5, 9, |p| Some(if p == 5 { 6 } else { 5 })),
            None
        );
    }

    #[test]
    fn a_grandchild_is_a_descendant_too() {
        #[cfg(unix)]
        {
            let me = std::process::id();
            // sh starts sleep: sleep is a grandchild of this process.
            let mut child = std::process::Command::new("sh")
                .args(["-c", "sleep 5 & echo $!; wait"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            use std::io::BufRead;
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let grandchild: u32 = line.trim().parse().unwrap();
            assert_eq!(is_descendant_of(grandchild, me), Some(true));
            assert!(!DaemonTreeGate::for_pid(me).may_answer(Some(grandchild)));
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::process::Command::new("kill")
                .arg(grandchild.to_string())
                .status();
        }
    }
}
