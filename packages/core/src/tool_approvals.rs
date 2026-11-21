//! The remote-tool approvals this process has given, in memory only.
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
//!   approval's own for this task alone.
//! - The first write the node service reports for the node inside that mark
//!   **consumes** the token: the record becomes **live**, holding the version
//!   and creation time that write left. Any other reported write to the node
//!   (an update outside the mark, a second update inside it, a create, a
//!   delete, a write reported by the store's notifier, a change to a schema of
//!   the database) ends the record outright.
//! - A live record counts only while the stored node has exactly that version
//!   and creation time; a node that differs in either ends it. A pending record
//!   never counts.
//! - The record is taken back when the approval's write does not land, however
//!   it ends (a failure, a conflict, a caller that gave up), by the guard
//!   [`begin`] returns.
//!
//! A write that never reaches the node service (to the database file directly)
//! is not seen; it can only put back the state that is live now, since any
//! reported write since the approval has ended the record.
//!
//! Records are kept per database (by the database file's path) and node: a
//! copy of an approved node in another database is not approved. They are lost
//! when the process stops, like the key the tags are made under. At most
//! [`MAX_RECORDS`] are kept per database; the oldest is dropped first, which
//! only ends that approval.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

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
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Ledger) -> T) -> T {
    let mut guard = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Ledger::default))
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
pub async fn as_own_write<F: Future>(scope: &str, node_id: &str, write: F) -> F::Output {
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
pub struct PendingApproval {
    scope: String,
    node_id: String,
    tag: String,
    landed: bool,
}

impl PendingApproval {
    /// The approval's write landed: its record stays (live if the write was
    /// reported as the approval's own, else pending, which never counts).
    pub fn landed(mut self) {
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
pub fn begin(
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
                ledger.entries.remove(&oldest);
            }
        }
        let order = ledger.next;
        ledger.next += 1;
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
pub fn reported_update(scope: &str, node: &Node) {
    let own = is_own_write(scope, &node.id);
    with(|ledger| {
        let k = key(scope, &node.id);
        match ledger.entries.get_mut(&k) {
            Some(entry) if own && entry.state == State::Pending => {
                entry.state = State::Live {
                    version: node.version,
                    created_at: node.created_at,
                };
            }
            Some(_) => {
                ledger.entries.remove(&k);
            }
            None => {}
        }
    });
}

/// Ends the record of `node_id`, whatever it holds: a create, a delete, or a
/// write reported where it cannot be told apart.
pub fn forget(scope: &str, node_id: &str) {
    with(|ledger| {
        ledger.entries.remove(&key(scope, node_id));
    });
}

/// Ends every record of the database `scope`: a change that can reach any
/// tool of it (a schema's fields or what it extends).
pub fn forget_scope(scope: &str) {
    with(|ledger| ledger.entries.retain(|(s, _), _| s != scope));
}

/// Ends the record of `node_id` only if it is the approval `tag` (an
/// approval whose write did not land takes back its own record and no newer
/// one).
pub fn forget_tag(scope: &str, node_id: &str, tag: &str) {
    with(|ledger| {
        let k = key(scope, node_id);
        if ledger.entries.get(&k).is_some_and(|e| e.tag == tag) {
            ledger.entries.remove(&k);
        }
    });
}

/// The approval `tag` of `node` as it is stored now, if it is in effect: a
/// live record for this database and node, the same tag, and exactly the
/// version and creation time the approval's write left. A live record the node
/// no longer matches is ended; a pending one is left and does not count.
pub fn live(scope: &str, node: &Node, tag: &str) -> Option<Issued> {
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
        if version != node.version || created_at != node.created_at {
            ledger.entries.remove(&k);
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
pub fn recorded(scope: &str, node_id: &str) -> bool {
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
        // The node differs from what the approval left: over, for good, the
        // version lower as much as higher.
        for version in [3, 1] {
            approve(&scope, &n, "t1").await;
            assert!(live(&scope, &node_with(&n, version), "t1").is_none());
            assert!(!recorded(&scope, &n.id), "{version}");
            assert!(live(&scope, &n, "t1").is_none());
        }
        approve(&scope, &n, "t1").await;
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
        forget_scope(&scope);
        assert!(!recorded(&scope, &a.id) && !recorded(&scope, &b.id));
        assert!(recorded(&other, &a.id));
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
}
