//! The remote-tool approvals this process has given, in memory, and, when the
//! daemon installs one, in a store of the team (see [`install_persistence`]).
//!
//! An approval tag (`approved_binding`) says what was approved; this record
//! says that it is still in effect. The tag alone is not enough: a node can be
//! turned off, edited, archived, retyped or deleted, and then put back exactly
//! as it was approved (the old tag included) by anything that can write it.
//!
//! So a record is a one-shot token, and nothing about it rests on a version
//! number a writer could choose:
//!
//! - The approval opens it as **pending** ([`begin`]) right before its own
//!   write, and makes that write inside [`as_own_write`], which marks it as the
//!   approval's own for this task alone. Only the node service's
//!   `write_tool_approval` does; nothing outside this crate can.
//! - The first write the node service reports for the node inside that mark
//!   **consumes** the token: the record becomes **live**, holding the version
//!   and creation time that write left. Any other reported write to the node
//!   (an update outside the mark, a second update inside it, a create, a
//!   delete, a write reported by the store's notifier, a change to a schema of
//!   the database) ends the record outright.
//! - A live record counts only while the node has exactly that version and
//!   creation time. A node at a later version, or with another creation time,
//!   ends it. A node at an earlier version was read before the approval's
//!   write landed (a turn's offer or a preview that raced it) or was put back
//!   below it behind the node service's back: it does not count, and it leaves
//!   the record, since every reported write since the approval's own has ended
//!   the record already. A pending record never counts.
//! - The record is taken back when the approval's write does not land, however
//!   it ends (a failure, a conflict, a caller that gave up), by the guard
//!   [`begin`] returns.
//!
//! **The rule for every end:** an approval is ended, and the end saved
//! ([`install_persistence`]), BEFORE the write that causes it. A kill between
//! the two then cannot leave an approval live that the write made wrong, and a
//! write that fails after its end has only ended an approval needlessly (the
//! person approves again). Where the cause is reported after it has happened
//! (the node service's report of an update), the node's version check covers a
//! restart; a cause that changes no node version (a retype, a schema write) must
//! end the record first.
//!
//! Writes made where they cannot be told apart from another end the record as
//! they are made ([`written`]): a write through the store itself, before the
//! read-back that reports it (which can fail, find the node gone or be
//! cancelled), and a merge, under the write guard, so no approval lands
//! between the two. A write of a schema ends every record of its database
//! ([`forget_scope`]), and logs how many, so the re-approvals it causes can be
//! explained.
//!
//! A write that never reaches the node service (to the database file directly)
//! is not seen; it can only put back the state that is live now, since any
//! reported write since the approval has ended the record.
//!
//! Records are kept per database (by the database file's path) and node: a
//! copy of an approved node in another database is not approved. Without a
//! persistence they are lost when the process stops, like the key the tags are
//! made under. With one, the live records are written to it whenever one is
//! made or ended and read back when it is installed; a restored record is as
//! strict as a fresh one (the node must have exactly the recorded version and
//! creation time, and the tag, made under the same key, must match). At most
//! [`MAX_RECORDS`] are kept per database; the oldest is dropped first, which
//! only ends that approval.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::Node;

/// Most approvals recorded at once in one database.
pub const MAX_RECORDS: usize = 4096;

/// An approval in effect, as [`live`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    /// The tag written to `approved_binding`.
    pub tag: String,
    /// The node's version as the approval's own write left it.
    pub version: i64,
    pub created_at: DateTime<Utc>,
    /// SHA-256 of the program file the approval pins (a `tool-cli` program, a
    /// stdio `tool-mcp` command), as lowercase hex; `None` for a tool that
    /// starts no program.
    pub program_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Opened by the approval; its own write has not been reported yet.
    Pending,
    /// Consumed by the approval's own write.
    Live {
        version: i64,
        created_at: DateTime<Utc>,
    },
}

struct Entry {
    tag: String,
    program_sha256: Option<String>,
    state: State,
    /// Insertion order, for dropping the oldest at the cap.
    order: u64,
}

#[derive(Default)]
struct Ledger {
    entries: HashMap<(String, String), Entry>,
    next: u64,
    /// What changed in the live records since the last save.
    change: Change,
}

/// What a mutation did to the live records, as far as saving is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Change {
    #[default]
    None,
    /// A record went live: not saving it only means it does not outlive the
    /// process.
    Made,
    /// A live record ended: not saving that would let it come back live after
    /// a restart.
    Ended,
}

impl Ledger {
    /// Ends the record at `k`; marks the ledger for saving if it was live.
    fn remove(&mut self, k: &(String, String)) {
        if let Some(entry) = self.entries.remove(k) {
            if matches!(entry.state, State::Live { .. }) {
                self.change = Change::Ended;
            }
        }
    }

    /// A record went live.
    fn made_live(&mut self) {
        if self.change == Change::None {
            self.change = Change::Made;
        }
    }
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

/// One live approval as the persistence keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRecord {
    pub scope: String,
    pub node_id: String,
    pub tag: String,
    pub program_sha256: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

/// Where the live records are kept so that they outlive the process. The
/// daemon installs one that writes to the team keychain; a failure to save or
/// load is logged and never makes an approval succeed or fail.
pub trait RecordSink: Send + Sync {
    fn load(&self) -> Result<Vec<PersistedRecord>, String>;
    fn save(&self, records: &[PersistedRecord]) -> Result<(), String>;
    /// Removes everything saved. Used when the end of an approval could not be
    /// saved: nothing is then restored at the next start, so the person
    /// approves again, which is safe, instead of an ended approval coming back.
    fn clear(&self) -> Result<(), String>;
}

/// How often a save is tried before the saved records are cleared instead.
const SAVE_ATTEMPTS: u32 = 3;

/// The end of an approval could neither be saved nor cleared away. Until a
/// later save works, a restart would bring back what had ended.
static UNSAVED_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether an ended approval is known to be still saved as live.
pub fn saved_approvals_may_be_stale() -> bool {
    UNSAVED_END.load(std::sync::atomic::Ordering::SeqCst)
}

static SINK: Mutex<Option<Arc<dyn RecordSink>>> = Mutex::new(None);
/// Serialises saves, so the last one written is the latest state.
static SAVE: Mutex<()> = Mutex::new(());

fn sink() -> Option<Arc<dyn RecordSink>> {
    SINK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn snapshot(ledger: &Ledger) -> Vec<PersistedRecord> {
    let mut records: Vec<PersistedRecord> = ledger
        .entries
        .iter()
        .filter_map(|((scope, node_id), entry)| match entry.state {
            State::Live {
                version,
                created_at,
            } => Some(PersistedRecord {
                scope: scope.clone(),
                node_id: node_id.clone(),
                tag: entry.tag.clone(),
                program_sha256: entry.program_sha256.clone(),
                version,
                created_at,
            }),
            State::Pending => None,
        })
        .collect();
    records.sort_by(|a, b| (&a.scope, &a.node_id).cmp(&(&b.scope, &b.node_id)));
    records
}

/// Installs `sink` and reads the records it holds into the ledger, as live
/// records that count only while the node still has exactly the recorded
/// version and creation time. Call once, at start, before any database is
/// opened. A sink that cannot be read leaves the ledger as it is.
pub fn install_persistence(sink: Arc<dyn RecordSink>) {
    match sink.load() {
        Ok(records) => {
            let mut per_scope: HashMap<String, usize> = HashMap::new();
            let mut restored = 0;
            with(|ledger| {
                for record in records {
                    let seen = per_scope.entry(record.scope.clone()).or_default();
                    if *seen >= MAX_RECORDS {
                        continue;
                    }
                    *seen += 1;
                    restored += 1;
                    let order = ledger.next;
                    ledger.next += 1;
                    ledger.entries.insert(
                        key(&record.scope, &record.node_id),
                        Entry {
                            tag: record.tag,
                            program_sha256: record.program_sha256,
                            state: State::Live {
                                version: record.version,
                                created_at: record.created_at,
                            },
                            order,
                        },
                    );
                }
                ledger.change = Change::None;
            });
            tracing::info!(
                restored,
                "restored the remote-tool approvals of an earlier run"
            );
        }
        Err(why) => {
            tracing::warn!(%why, "the saved remote-tool approvals could not be read; none are restored");
        }
    }
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
}

fn with<T>(f: impl FnOnce(&mut Ledger) -> T) -> T {
    let (out, change) = {
        let mut guard = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
        let ledger = guard.get_or_insert_with(Ledger::default);
        let out = f(ledger);
        (out, std::mem::take(&mut ledger.change))
    };
    if change != Change::None {
        // On the caller's thread, before it goes on: an approval that ended is
        // saved as ended before whatever ended it returns. A multi-threaded
        // runtime is told the thread blocks, so the keychain does not hold up
        // its other tasks.
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => {
                tokio::task::block_in_place(|| persist(change));
            }
            _ => persist(change),
        }
    }
    out
}

/// Writes the live records to the sink, from the ledger as it is now.
///
/// A save that fails is tried again. If the change was the end of a live
/// approval and the save still fails, the saved records are cleared instead, so
/// that a restart restores nothing: the approvals that were live then need the
/// person again, and the ended one cannot come back. If even that fails the
/// state is flagged ([`saved_approvals_may_be_stale`]) and logged as an error,
/// and every later change tries again.
fn persist(change: Change) {
    let Some(sink) = sink() else {
        return;
    };
    let _serial = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let unsaved_end = UNSAVED_END.load(std::sync::atomic::Ordering::SeqCst);
    let ended = change == Change::Ended || unsaved_end;
    let records = {
        let mut guard = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
        snapshot(guard.get_or_insert_with(Ledger::default))
    };
    save_with_policy(&*sink, &records, ended, &UNSAVED_END);
}

/// Saves `records` to `sink` as [`persist`] says: tried again on failure, and
/// for the end of an approval cleared away if it will not save. `unsaved` is
/// the flag [`saved_approvals_may_be_stale`] reads.
fn save_with_policy(
    sink: &dyn RecordSink,
    records: &[PersistedRecord],
    ended: bool,
    unsaved: &std::sync::atomic::AtomicBool,
) {
    let mut last = String::new();
    for _ in 0..SAVE_ATTEMPTS {
        match sink.save(records) {
            Ok(()) => {
                unsaved.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            Err(why) => last = why,
        }
    }
    if !ended {
        tracing::warn!(why = %last, "the remote-tool approvals could not be saved; they will not survive a restart");
        return;
    }
    match sink.clear() {
        Ok(()) => {
            unsaved.store(false, std::sync::atomic::Ordering::SeqCst);
            tracing::warn!(why = %last, "the end of a remote-tool approval could not be saved; the saved approvals were cleared, so every approval needs the person again after a restart");
        }
        Err(clear) => {
            unsaved.store(true, std::sync::atomic::Ordering::SeqCst);
            tracing::error!(why = %last, %clear, "the end of a remote-tool approval could be neither saved nor cleared away: a restart can bring an ended approval back");
        }
    }
}

fn key(scope: &str, node_id: &str) -> (String, String) {
    (scope.to_string(), node_id.to_string())
}

/// The write an approval is making, as [`as_own_write`] marks it.
#[derive(Clone)]
struct OwnWrite {
    scope: String,
    node_id: String,
}

tokio::task_local! {
    static OWN_WRITE: OwnWrite;
}

/// Runs `write` as the approval's own write of `node_id` in the database
/// `scope`: a write reported inside it, on this task, consumes the pending
/// record. Nothing outside it can.
pub(crate) async fn as_own_write<F: Future>(scope: &str, node_id: &str, write: F) -> F::Output {
    OWN_WRITE
        .scope(
            OwnWrite {
                scope: scope.to_string(),
                node_id: node_id.to_string(),
            },
            write,
        )
        .await
}

/// Whether the write being reported on this task is the approval's own write
/// of `node_id` in `scope`.
fn is_own_write(scope: &str, node_id: &str) -> bool {
    OWN_WRITE
        .try_with(|own| own.scope == scope && own.node_id == node_id)
        .unwrap_or(false)
}

/// An approval opened by [`begin`]. Dropping it takes the approval's record
/// back, pending or consumed, unless [`PendingApproval::landed`] said the
/// approval's write landed: a write that failed (even after it was reported,
/// in a transaction that then did not commit) or was given up leaves nothing.
#[must_use = "dropping the guard takes the record back"]
pub(crate) struct PendingApproval {
    scope: String,
    node_id: String,
    tag: String,
    landed: bool,
}

impl PendingApproval {
    /// The approval's write landed: its record stays (live if the write was
    /// reported as the approval's own, else pending, which never counts).
    pub(crate) fn landed(mut self) {
        self.landed = true;
    }
}

impl Drop for PendingApproval {
    fn drop(&mut self) {
        if !self.landed {
            forget_tag(&self.scope, &self.node_id, &self.tag);
        }
    }
}

/// Opens a pending record of the approval `tag` of `node_id` in the database
/// `scope`, replacing any earlier record of the node. It counts only once the
/// approval's own write, made inside [`as_own_write`], is reported.
pub(crate) fn begin(
    scope: &str,
    node_id: &str,
    tag: &str,
    program_sha256: Option<String>,
) -> PendingApproval {
    with(|ledger| {
        let in_scope = ledger.entries.keys().filter(|(s, _)| s == scope).count();
        if in_scope >= MAX_RECORDS && !ledger.entries.contains_key(&key(scope, node_id)) {
            if let Some(oldest) = ledger
                .entries
                .iter()
                .filter(|((s, _), _)| s == scope)
                .min_by_key(|(_, entry)| entry.order)
                .map(|(k, _)| k.clone())
            {
                ledger.remove(&oldest);
            }
        }
        let order = ledger.next;
        ledger.next += 1;
        // A new approval replaces any record of the node; a live one ending
        // is a change to save.
        ledger.remove(&key(scope, node_id));
        ledger.entries.insert(
            key(scope, node_id),
            Entry {
                tag: tag.to_string(),
                program_sha256,
                state: State::Pending,
                order,
            },
        );
    });
    PendingApproval {
        scope: scope.to_string(),
        node_id: node_id.to_string(),
        tag: tag.to_string(),
        landed: false,
    }
}

/// A write to `node` the node service reports as an update. Inside the
/// approval's own write (and only the first time) it consumes the pending
/// record; any other update ends the record.
pub(crate) fn reported_update(scope: &str, node: &Node) {
    let own = is_own_write(scope, &node.id);
    with(|ledger| {
        let k = key(scope, &node.id);
        match ledger.entries.get_mut(&k) {
            Some(entry) if own && entry.state == State::Pending => {
                entry.state = State::Live {
                    version: node.version,
                    created_at: node.created_at,
                };
                ledger.made_live();
            }
            Some(_) => {
                ledger.remove(&k);
            }
            None => {}
        }
    });
}

/// Ends the record of `node_id`, whatever it holds: a create, a delete, or a
/// write reported where it cannot be told apart.
pub(crate) fn forget(scope: &str, node_id: &str) {
    with(|ledger| {
        ledger.remove(&key(scope, node_id));
    });
}

/// Ends every record of the database `scope`: a write of the schema
/// `schema_id`, whose fields or what it extends can reach any tool of it.
/// Logs how many records it ended (naming the schema, nothing else of the
/// database), so the re-approvals it causes can be explained. Returns that
/// count.
pub(crate) fn forget_scope(scope: &str, schema_id: &str) -> usize {
    let ended = with(|ledger| {
        let ended: Vec<_> = ledger
            .entries
            .keys()
            .filter(|(s, _)| s == scope)
            .cloned()
            .collect();
        for k in &ended {
            ledger.remove(k);
        }
        ended.len()
    });
    if ended > 0 {
        tracing::info!(
            schema = %schema_id,
            ended,
            "a write of a schema ended the remote-tool approvals of its database: those tools are off until they are approved again"
        );
    }
    ended
}

/// A write of `node_id`, of type `node_type` as written, where it cannot be
/// told apart from another: it ends the node's record, or, for a schema,
/// every record of the database ([`forget_scope`]).
pub(crate) fn written(scope: &str, node_id: &str, node_type: &str) {
    if crate::models::CoreNodeType::Schema.is_exactly(node_type) {
        forget_scope(scope, node_id);
    } else {
        forget(scope, node_id);
    }
}

/// Ends the record of `node_id` only if it is the approval `tag` (an
/// approval whose write did not land takes back its own record and no newer
/// one).
fn forget_tag(scope: &str, node_id: &str, tag: &str) {
    with(|ledger| {
        let k = key(scope, node_id);
        if ledger.entries.get(&k).is_some_and(|e| e.tag == tag) {
            ledger.remove(&k);
        }
    });
}

/// The approval `tag` of `node` as it is stored now, if it is in effect: a
/// live record for this database and node, the same tag, and exactly the
/// version and creation time the approval's write left. A node at a later
/// version, or with another creation time, ends the record; one at an earlier
/// version (read before the approval's write landed) leaves it. A pending
/// record is left and does not count.
pub(crate) fn live(scope: &str, node: &Node, tag: &str) -> Option<Issued> {
    with(|ledger| {
        let k = key(scope, &node.id);
        let entry = ledger.entries.get(&k)?;
        let State::Live {
            version,
            created_at,
        } = entry.state
        else {
            return None;
        };
        // Read before the approval's write landed, or put back below it
        // behind the service's back: not in effect, and nothing to end (every
        // reported write since the approval's own ended the record already).
        if node.version < version {
            return None;
        }
        if node.version != version || created_at != node.created_at {
            ledger.remove(&k);
            return None;
        }
        (entry.tag == tag).then(|| Issued {
            tag: entry.tag.clone(),
            version,
            created_at,
            program_sha256: entry.program_sha256.clone(),
        })
    })
}

/// Whether `node_id` has a record (pending or live) in the database `scope`.
pub(crate) fn recorded(scope: &str, node_id: &str) -> bool {
    with(|ledger| ledger.entries.contains_key(&key(scope, node_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(version: i64) -> Node {
        let mut node = Node::new("tool-rest".into(), "t".into(), serde_json::json!({}));
        node.version = version;
        node
    }

    fn node_with(n: &Node, version: i64) -> Node {
        let mut n = n.clone();
        n.version = version;
        n
    }

    /// Opens and consumes an approval of `n`, as its own write would.
    async fn approve(scope: &str, n: &Node, tag: &str) {
        let pending = begin(scope, &n.id, tag, None);
        as_own_write(scope, &n.id, async { reported_update(scope, n) }).await;
        pending.landed();
    }

    #[tokio::test]
    async fn an_approval_is_live_only_for_its_tag_version_creation_and_database() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        approve(&scope, &n, "t1").await;
        assert!(live(&scope, &n, "t1").is_some());
        assert!(live(&scope, &n, "t2").is_none());
        assert!(live("another-database", &n, "t1").is_none());
        // The node is past what the approval left: over, for good.
        approve(&scope, &n, "t1").await;
        assert!(live(&scope, &node_with(&n, 3), "t1").is_none());
        assert!(!recorded(&scope, &n.id));
        assert!(live(&scope, &n, "t1").is_none());
        // Before it (read before the approval's write landed): not in effect,
        // and the record is left.
        approve(&scope, &n, "t1").await;
        assert!(live(&scope, &node_with(&n, 1), "t1").is_none());
        assert!(live(&scope, &n, "t1").is_some());
        // Made again under the same id, at the approved version: over.
        let mut made_again = n.clone();
        made_again.created_at = n.created_at + chrono::Duration::microseconds(1);
        assert!(live(&scope, &made_again, "t1").is_none());
        assert!(!recorded(&scope, &n.id));
    }

    /// A pending record never counts, and a look at the node before the
    /// approval's write is reported (a turn's offer, a preview) leaves it: the
    /// approval is in effect once that write is reported.
    #[tokio::test]
    async fn a_pending_record_never_counts_and_a_look_leaves_it() {
        let scope = uuid::Uuid::new_v4().to_string();
        let before = node(1);
        let after = node_with(&before, 2);
        let pending = begin(&scope, &before.id, "t", None);
        for look in [&before, &after] {
            assert!(live(&scope, look, "t").is_none());
            assert!(recorded(&scope, &before.id));
        }
        as_own_write(&scope, &before.id, async {
            reported_update(&scope, &after)
        })
        .await;
        pending.landed();
        assert!(live(&scope, &after, "t").is_some());
    }

    /// The token is consumed once, by a write inside the approval's own mark;
    /// a write outside it, or a second one inside it, ends the record.
    #[tokio::test]
    async fn a_pending_record_is_consumed_once_and_only_by_the_approvals_own_write() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        // Outside the mark: ends it.
        let pending = begin(&scope, &n.id, "t", None);
        reported_update(&scope, &n);
        assert!(!recorded(&scope, &n.id));
        drop(pending);
        // Inside the mark for another node: ends it.
        let pending = begin(&scope, &n.id, "t", None);
        as_own_write(&scope, "another-node", async {
            reported_update(&scope, &n)
        })
        .await;
        assert!(!recorded(&scope, &n.id));
        drop(pending);
        // Inside the mark: consumed; a second write inside it ends it.
        let pending = begin(&scope, &n.id, "t", None);
        as_own_write(&scope, &n.id, async {
            reported_update(&scope, &n);
            assert!(live(&scope, &n, "t").is_some());
            reported_update(&scope, &node_with(&n, 3));
        })
        .await;
        assert!(!recorded(&scope, &n.id));
        drop(pending);
        // Live, then any later update, at any version, ends it.
        for version in [2, 3, 1] {
            approve(&scope, &n, "t").await;
            reported_update(&scope, &node_with(&n, version));
            assert!(!recorded(&scope, &n.id), "{version}");
        }
    }

    /// An approval whose write did not land (a failure, a conflict, a caller
    /// that gave up, a transaction that was reported and then not committed)
    /// takes its record back, pending or consumed, and no newer one; one that
    /// landed keeps it.
    #[tokio::test]
    async fn an_approval_whose_write_did_not_land_takes_its_record_back() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(1);
        let pending = begin(&scope, &n.id, "t", None);
        assert!(recorded(&scope, &n.id));
        drop(pending);
        assert!(!recorded(&scope, &n.id));
        let pending = begin(&scope, &n.id, "t", None);
        as_own_write(&scope, &n.id, async { reported_update(&scope, &n) }).await;
        assert!(live(&scope, &n, "t").is_some());
        drop(pending);
        assert!(!recorded(&scope, &n.id));
        let old = begin(&scope, &n.id, "old", None);
        approve(&scope, &n, "new").await;
        drop(old);
        assert!(live(&scope, &n, "new").is_some());
        approve(&scope, &n, "t").await;
        assert!(recorded(&scope, &n.id));
    }

    #[tokio::test]
    async fn a_failed_approval_takes_back_only_its_own_record() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        approve(&scope, &n, "new").await;
        forget_tag(&scope, &n.id, "old");
        assert!(recorded(&scope, &n.id));
        forget_tag(&scope, &n.id, "new");
        assert!(!recorded(&scope, &n.id));
        approve(&scope, &n, "t").await;
        forget(&scope, &n.id);
        assert!(!recorded(&scope, &n.id));
    }

    #[tokio::test]
    async fn a_schema_change_ends_every_record_of_its_database_only() {
        let (scope, other) = (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        );
        let (a, b) = (node(1), node(1));
        approve(&scope, &a, "t").await;
        approve(&scope, &b, "t").await;
        approve(&other, &a, "t").await;
        written(&scope, "a-schema", "schema");
        assert!(!recorded(&scope, &a.id) && !recorded(&scope, &b.id));
        assert!(recorded(&other, &a.id));
        // A write of any other node ends only its own record.
        approve(&scope, &a, "t").await;
        approve(&scope, &b, "t").await;
        written(&scope, &a.id, "text");
        assert!(!recorded(&scope, &a.id) && recorded(&scope, &b.id));
    }

    /// A tiny log sink for one test.
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A schema write that ends records says so in the log, with how many,
    /// naming the schema and nothing else of the database; one that ends
    /// none says nothing.
    #[tokio::test]
    async fn a_schema_write_that_ends_records_says_how_many() {
        let scope = format!("/Users/me/{}/nodespace.db", uuid::Uuid::new_v4());
        let (a, b) = (node(1), node(1));
        approve(&scope, &a, "t").await;
        approve(&scope, &b, "t").await;
        let logs = Capture::default();
        {
            let sink = logs.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .finish();
            let _guard = tracing::subscriber::set_default(subscriber);
            assert_eq!(forget_scope(&scope, "remote-tool"), 2);
            assert_eq!(forget_scope(&scope, "remote-tool"), 0);
        }
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = logs.lines().collect();
        assert_eq!(lines.len(), 1, "{logs}");
        assert!(lines[0].contains("INFO"), "{logs}");
        assert!(lines[0].contains("schema=remote-tool"), "{logs}");
        assert!(lines[0].contains("ended=2"), "{logs}");
        assert!(!logs.contains(&scope), "{logs}");
    }

    /// Through the node service: a schema written in a database ends its
    /// approvals, and the log names that schema.
    #[tokio::test]
    async fn a_schema_written_through_the_service_is_named_in_the_log() {
        let (svc, _tmp) = service().await;
        let id = text_node(&svc, "a").await;
        let scope = svc.approval_scope();
        let n = svc.get_node(&id).await.unwrap().unwrap();
        approve(&scope, &n, "t").await;
        let logs = Capture::default();
        {
            let sink = logs.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .finish();
            let _guard = tracing::subscriber::set_default(subscriber);
            crate::schema::handle_create_schema(
                &svc,
                serde_json::json!({ "name": "Logged Kind", "fields": [] }),
            )
            .await
            .unwrap();
        }
        assert!(!recorded(&scope, &id));
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let ended: Vec<&str> = logs
            .lines()
            .filter(|l| l.contains("remote-tool approvals"))
            .collect();
        assert_eq!(ended.len(), 1, "{logs}");
        assert!(ended[0].contains("schema=logged-kind"), "{logs}");
        assert!(ended[0].contains("ended=1"), "{logs}");
    }

    /// A look that read the node before the approval's write landed (a turn's
    /// offer or a preview, racing a re-approval of a tool that is still
    /// enabled) finds it not in effect yet, and leaves it: the approval is in
    /// effect once its write lands, whenever that look gets to the ledger.
    #[tokio::test]
    async fn a_look_that_read_the_node_before_the_approvals_write_landed_leaves_it() {
        let scope = uuid::Uuid::new_v4().to_string();
        let before = node(1);
        let after = node_with(&before, 2);
        let pending = begin(&scope, &before.id, "new", None);
        as_own_write(&scope, &before.id, async {
            // Reported in the write's transaction, before it commits.
            reported_update(&scope, &after);
            // A look meanwhile reads the committed node, its old tag and all.
            assert!(live(&scope, &before, "old").is_none());
        })
        .await;
        pending.landed();
        // One that read it then and gets here only now.
        assert!(live(&scope, &before, "old").is_none());
        assert!(live(&scope, &after, "new").is_some());
    }

    async fn service() -> (
        std::sync::Arc<crate::services::NodeService>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut store = std::sync::Arc::new(
            crate::db::SqliteStore::new(tmp.path().join("t.db"))
                .await
                .unwrap(),
        );
        let svc = crate::services::NodeService::new(&mut store).await.unwrap();
        (std::sync::Arc::new(svc), tmp)
    }

    async fn text_node(svc: &crate::services::NodeService, content: &str) -> String {
        svc.create_node(Node::new(
            "text".into(),
            content.into(),
            serde_json::json!({}),
        ))
        .await
        .unwrap()
    }

    /// Through the real node service: a look that reads the committed node
    /// while the approval's write has been reported in its transaction but
    /// not committed leaves the approval, which is in effect once the write
    /// lands.
    #[tokio::test]
    async fn a_look_between_the_approvals_write_and_its_commit_leaves_it() {
        let (svc, _tmp) = service().await;
        let id = text_node(&svc, "before").await;
        let scope = svc.approval_scope();
        let pending = begin(&scope, &id, "t", None);
        let (looker, at) = (std::sync::Arc::clone(&svc), id.clone());
        as_own_write(
            &scope,
            &id,
            svc.with_transaction(move |tx| {
                Box::pin(async move {
                    looker
                        .update_node_in_tx(
                            tx,
                            &at,
                            crate::models::NodeUpdate::new().with_content("after".into()),
                        )
                        .await?;
                    let committed = looker.get_node(&at).await?.expect("the node");
                    assert_eq!(committed.content, "before");
                    assert!(looker.live_tool_approval(&committed, "t").is_none());
                    Ok(())
                })
            }),
        )
        .await
        .unwrap();
        pending.landed();
        let stored = svc.get_node(&id).await.unwrap().unwrap();
        assert_eq!(stored.content, "after");
        assert!(svc.live_tool_approval(&stored, "t").is_some());
    }

    /// A merge ends the approvals of both nodes under the write guard, where
    /// no approval can land before the merge does: one that lands while the
    /// merge waits for the guard does not survive it.
    #[tokio::test]
    async fn an_approval_landing_while_a_merge_waits_for_the_guard_is_ended_by_it() {
        let (svc, _tmp) = service().await;
        let survivor = text_node(&svc, "survivor").await;
        let loser = text_node(&svc, "loser").await;
        let scope = svc.approval_scope();
        let guard = svc.store().write().await;
        let merging = {
            let svc = std::sync::Arc::clone(&svc);
            let (survivor, loser) = (survivor.clone(), loser.clone());
            tokio::spawn(async move { svc.merge_nodes(&survivor, &loser, None).await })
        };
        // The merge runs up to the guard and waits there.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(!merging.is_finished());
        for id in [&survivor, &loser] {
            let n = svc.get_node(id).await.unwrap().unwrap();
            let pending = begin(&scope, id, "t", None);
            as_own_write(&scope, id, async { reported_update(&scope, &n) }).await;
            pending.landed();
            assert!(recorded(&scope, id));
        }
        drop(guard);
        merging.await.unwrap().unwrap();
        assert!(!recorded(&scope, &survivor), "the survivor");
        assert!(!recorded(&scope, &loser), "the loser");
    }

    #[test]
    fn the_records_are_bounded_and_the_oldest_goes_first() {
        let scope = uuid::Uuid::new_v4().to_string();
        let first = node(1);
        let _kept = begin(&scope, &first.id, "t", None);
        let mut guards = Vec::new();
        for _ in 0..MAX_RECORDS {
            let n = node(1);
            guards.push(begin(&scope, &n.id, "t", None));
        }
        assert!(!recorded(&scope, &first.id));
        let in_scope = with(|ledger| ledger.entries.keys().filter(|(s, _)| *s == scope).count());
        assert_eq!(in_scope, MAX_RECORDS);
    }

    /// Keeps what it is given; can be told to fail.
    #[derive(Default)]
    struct MemorySink {
        saved: Mutex<Vec<PersistedRecord>>,
        fail_load: std::sync::atomic::AtomicBool,
        fail_save: std::sync::atomic::AtomicBool,
        fail_clear: std::sync::atomic::AtomicBool,
        /// Saves that fail before one works.
        save_failures: std::sync::atomic::AtomicU32,
        saves: std::sync::atomic::AtomicU32,
        clears: std::sync::atomic::AtomicU32,
    }

    impl RecordSink for MemorySink {
        fn load(&self) -> Result<Vec<PersistedRecord>, String> {
            if self.fail_load.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("unreadable".into());
            }
            Ok(self.saved.lock().unwrap().clone())
        }
        fn save(&self, records: &[PersistedRecord]) -> Result<(), String> {
            use std::sync::atomic::Ordering::SeqCst;
            self.saves.fetch_add(1, SeqCst);
            if self.fail_save.load(SeqCst) {
                return Err("unwritable".into());
            }
            if self
                .save_failures
                .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("busy".into());
            }
            *self.saved.lock().unwrap() = records.to_vec();
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            use std::sync::atomic::Ordering::SeqCst;
            self.clears.fetch_add(1, SeqCst);
            if self.fail_clear.load(SeqCst) {
                return Err("undeletable".into());
            }
            self.saved.lock().unwrap().clear();
            Ok(())
        }
    }

    impl MemorySink {
        fn of(&self, scope: &str) -> Vec<PersistedRecord> {
            self.saved
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.scope == scope)
                .cloned()
                .collect()
        }
    }

    /// Drops a database's records from memory without saving: what a restart
    /// does.
    fn lose_memory_of(scope: &str) {
        with(|ledger| {
            ledger.entries.retain(|(s, _), _| s != scope);
            ledger.change = Change::None;
        });
    }

    /// One test, because the sink is process-wide.
    #[tokio::test]
    async fn live_records_are_saved_restored_and_as_strict_as_fresh_ones() {
        let _turn = SERIAL.lock().await;
        let sink = Arc::new(MemorySink::default());
        install_persistence(sink.clone());
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);

        // A pending record is not saved; the approval's own write makes it live
        // and saves it.
        let pending = begin(&scope, &n.id, "t1", Some("abc".into()));
        assert!(sink.of(&scope).is_empty());
        as_own_write(&scope, &n.id, async { reported_update(&scope, &n) }).await;
        pending.landed();
        let saved = sink.of(&scope);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].tag, "t1");
        assert_eq!(saved[0].version, 2);
        assert_eq!(saved[0].program_sha256.as_deref(), Some("abc"));

        // A restart: the record is read back and counts for the same node.
        lose_memory_of(&scope);
        assert!(live(&scope, &n, "t1").is_none());
        install_persistence(sink.clone());
        assert_eq!(live(&scope, &n, "t1").map(|i| i.tag), Some("t1".into()));
        // ... and only for the same tag, version and creation time.
        assert!(live(&scope, &n, "other").is_none());
        let mut made_again = n.clone();
        made_again.created_at = n.created_at + chrono::Duration::microseconds(1);
        assert!(live(&scope, &made_again, "t1").is_none());
        assert!(sink.of(&scope).is_empty(), "ending a record is saved");

        // Every way a live record ends is saved.
        for end in 0..4 {
            approve(&scope, &n, "t1").await;
            assert_eq!(sink.of(&scope).len(), 1, "end {end}");
            match end {
                0 => forget(&scope, &n.id),
                1 => {
                    forget_scope(&scope, "schema");
                }
                2 => reported_update(&scope, &node_with(&n, 3)),
                _ => assert!(live(&scope, &node_with(&n, 3), "t1").is_none()),
            }
            assert!(sink.of(&scope).is_empty(), "end {end}");
        }
        // Replacing a live record with a new pending one ends the saved one.
        approve(&scope, &n, "t1").await;
        let pending = begin(&scope, &n.id, "t2", None);
        assert!(sink.of(&scope).is_empty());
        drop(pending);

        // A failure to save or to load neither panics nor changes what counts.
        sink.fail_save
            .store(true, std::sync::atomic::Ordering::SeqCst);
        approve(&scope, &n, "t1").await;
        assert!(live(&scope, &n, "t1").is_some());
        sink.fail_save
            .store(false, std::sync::atomic::Ordering::SeqCst);
        lose_memory_of(&scope);
        sink.fail_load
            .store(true, std::sync::atomic::Ordering::SeqCst);
        install_persistence(sink.clone());
        assert!(live(&scope, &n, "t1").is_none());
        *SINK.lock().unwrap() = None;
    }

    use std::sync::atomic::Ordering::SeqCst;

    /// The sink is process-wide: the tests that install one take turns.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// An approval that ended while its end could not be saved does not come
    /// back live after a restart, whatever ended it: the saved records are
    /// cleared, so nothing is restored. One test, because the sink is
    /// process-wide.
    #[tokio::test]
    async fn an_end_that_could_not_be_saved_does_not_come_back_after_a_restart() {
        let _turn = SERIAL.lock().await;
        let sink = Arc::new(MemorySink::default());
        install_persistence(sink.clone());
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        for end in 0..4 {
            approve(&scope, &n, "t1").await;
            assert_eq!(sink.of(&scope).len(), 1, "end {end}");
            sink.fail_save.store(true, SeqCst);
            match end {
                // No node version changes with these two: the saved record
                // alone would still look live.
                0 => forget(&scope, &n.id),
                1 => {
                    forget_scope(&scope, "schema");
                }
                2 => reported_update(&scope, &node_with(&n, 3)),
                _ => assert!(live(&scope, &node_with(&n, 3), "t1").is_none()),
            }
            sink.fail_save.store(false, SeqCst);
            assert!(!recorded(&scope, &n.id), "end {end}");
            // The restart: nothing of it is live.
            lose_memory_of(&scope);
            install_persistence(sink.clone());
            assert!(live(&scope, &n, "t1").is_none(), "end {end}");
            assert!(sink.of(&scope).is_empty(), "end {end}");
        }
        *SINK.lock().unwrap() = None;
    }

    #[test]
    fn a_save_is_tried_again_and_an_end_is_cleared_when_it_will_not_save() {
        let unsaved = std::sync::atomic::AtomicBool::new(false);
        let record = PersistedRecord {
            scope: "s".into(),
            node_id: "n".into(),
            tag: "t".into(),
            program_sha256: None,
            version: 1,
            created_at: Utc::now(),
        };

        // A busy store that answers on the third try: saved, nothing cleared.
        let sink = MemorySink::default();
        sink.save_failures.store(2, SeqCst);
        save_with_policy(&sink, std::slice::from_ref(&record), true, &unsaved);
        assert_eq!(sink.saves.load(SeqCst), 3);
        assert_eq!(sink.clears.load(SeqCst), 0);
        assert_eq!(sink.saved.lock().unwrap().len(), 1);

        // One that never does: the end clears what was saved.
        let sink = MemorySink::default();
        sink.saved.lock().unwrap().push(record.clone());
        sink.fail_save.store(true, SeqCst);
        save_with_policy(&sink, &[], true, &unsaved);
        assert_eq!(sink.saves.load(SeqCst), SAVE_ATTEMPTS);
        assert_eq!(sink.clears.load(SeqCst), 1);
        assert!(sink.saved.lock().unwrap().is_empty());
        assert!(!unsaved.load(SeqCst));

        // A new live record that will not save is not worth clearing for: the
        // older saved records stay and the new one only does not outlive the
        // process.
        let sink = MemorySink::default();
        sink.saved.lock().unwrap().push(record.clone());
        sink.fail_save.store(true, SeqCst);
        save_with_policy(&sink, &[record.clone(), record.clone()], false, &unsaved);
        assert_eq!(sink.clears.load(SeqCst), 0);
        assert_eq!(sink.saved.lock().unwrap().len(), 1);

        // Neither saved nor cleared: flagged, and a later save that works
        // unflags it.
        let sink = MemorySink::default();
        sink.fail_save.store(true, SeqCst);
        sink.fail_clear.store(true, SeqCst);
        save_with_policy(&sink, &[], true, &unsaved);
        assert!(unsaved.load(SeqCst));
        sink.fail_save.store(false, SeqCst);
        save_with_policy(&sink, &[], false, &unsaved);
        assert!(!unsaved.load(SeqCst));
    }

    #[tokio::test]
    async fn a_restore_keeps_at_most_the_cap_per_database() {
        let _turn = SERIAL.lock().await;
        let scope = uuid::Uuid::new_v4().to_string();
        let sink = Arc::new(MemorySink::default());
        {
            let mut saved = sink.saved.lock().unwrap();
            for i in 0..MAX_RECORDS + 5 {
                saved.push(PersistedRecord {
                    scope: scope.clone(),
                    node_id: format!("n{i}"),
                    tag: "t".into(),
                    program_sha256: None,
                    version: 1,
                    created_at: Utc::now(),
                });
            }
        }
        install_persistence(sink);
        let kept = with(|l| l.entries.keys().filter(|(s, _)| *s == scope).count());
        assert_eq!(kept, MAX_RECORDS);
        forget_scope(&scope, "schema");
        *SINK.lock().unwrap() = None;
    }
}
