//! The remote-tool approvals this process has given, in memory only.
//!
//! An approval tag (`approved_binding`) says what was approved; this record
//! says that it is still in effect. The tag alone is not enough: a node can be
//! turned off, edited, archived, retyped or deleted, and then put back exactly
//! as it was approved (the old tag included) by anything that can write it.
//! So each approval is recorded here with the node's version and creation
//! time as the approval's own write leaves them, and it counts only while the
//! stored node still has both: every later write to the node moves its
//! version, and a node made again has a new creation time. A delete the node
//! service reports removes the record outright, so a row put back afterwards
//! with its old version (by writing the database file directly) does not count
//! either.
//!
//! Records are kept per database (by the database file's path) and node: a
//! copy of an approved node in another database is not approved. They are lost
//! when the process stops, like the key the tags are made under. At most
//! [`MAX_RECORDS`] are kept per database; the oldest is dropped first, which
//! only ends that approval.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::models::Node;

/// Most approvals recorded at once in one database.
pub const MAX_RECORDS: usize = 4096;

/// An approval as it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    /// The tag written to `approved_binding`.
    pub tag: String,
    /// The node's version once the approval's own write is stored.
    pub version: i64,
    pub created_at: DateTime<Utc>,
    /// SHA-256 of the program file the approval pins (a `tool-cli` program, a
    /// stdio `tool-mcp` command), as lowercase hex; `None` for a tool that
    /// starts no program.
    pub program_sha256: Option<String>,
}

struct Entry {
    issued: Issued,
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

/// Records an approval of `node_id` in the database `scope`, replacing an
/// earlier one of the same node.
pub fn record(scope: &str, node_id: &str, issued: Issued) {
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
        ledger
            .entries
            .insert(key(scope, node_id), Entry { issued, order });
    });
}

/// Removes the record of `node_id`, whatever it holds.
pub fn forget(scope: &str, node_id: &str) {
    with(|ledger| {
        ledger.entries.remove(&key(scope, node_id));
    });
}

/// Removes the record of `node_id` only if it is the approval `tag` (an
/// approval whose write failed takes back its own record and no newer one).
pub fn forget_tag(scope: &str, node_id: &str, tag: &str) {
    with(|ledger| {
        let k = key(scope, node_id);
        if ledger.entries.get(&k).is_some_and(|e| e.issued.tag == tag) {
            ledger.entries.remove(&k);
        }
    });
}

/// The approval `tag` of `node` as it is stored now, if it is still in
/// effect: recorded for this database and node, the same tag, and the node's
/// version and creation time unchanged since the approval's write. A record
/// that no longer matches is removed (a version only grows).
pub fn live(scope: &str, node: &Node, tag: &str) -> Option<Issued> {
    with(|ledger| {
        let k = key(scope, &node.id);
        let entry = ledger.entries.get(&k)?;
        let issued = &entry.issued;
        if issued.tag != tag {
            return None;
        }
        if issued.version != node.version || issued.created_at != node.created_at {
            ledger.entries.remove(&k);
            return None;
        }
        Some(issued.clone())
    })
}

/// Whether `node_id` has a record in the database `scope`.
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

    fn issued(node: &Node, tag: &str) -> Issued {
        Issued {
            tag: tag.to_string(),
            version: node.version,
            created_at: node.created_at,
            program_sha256: None,
        }
    }

    #[test]
    fn an_approval_is_live_only_for_its_tag_version_creation_and_database() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        record(&scope, &n.id, issued(&n, "t1"));
        assert!(live(&scope, &n, "t1").is_some());
        assert!(live(&scope, &n, "t2").is_none());
        assert!(live("another-database", &n, "t1").is_none());
        // A later write moved the version: over, for good.
        assert!(live(&scope, &node_with(&n, 3), "t1").is_none());
        assert!(!recorded(&scope, &n.id));
        assert!(live(&scope, &n, "t1").is_none());
        // A node made again under the same id, at the same version.
        record(&scope, &n.id, issued(&n, "t1"));
        let mut made_again = n.clone();
        made_again.created_at = n.created_at + chrono::Duration::microseconds(1);
        assert!(live(&scope, &made_again, "t1").is_none());
        assert!(!recorded(&scope, &n.id));
    }

    fn node_with(n: &Node, version: i64) -> Node {
        let mut n = n.clone();
        n.version = version;
        n
    }

    #[test]
    fn a_failed_approval_takes_back_only_its_own_record() {
        let scope = uuid::Uuid::new_v4().to_string();
        let n = node(2);
        record(&scope, &n.id, issued(&n, "new"));
        forget_tag(&scope, &n.id, "old");
        assert!(recorded(&scope, &n.id));
        forget_tag(&scope, &n.id, "new");
        assert!(!recorded(&scope, &n.id));
        record(&scope, &n.id, issued(&n, "t"));
        forget(&scope, &n.id);
        assert!(!recorded(&scope, &n.id));
    }

    #[test]
    fn the_records_are_bounded_and_the_oldest_goes_first() {
        let scope = uuid::Uuid::new_v4().to_string();
        let first = node(1);
        record(&scope, &first.id, issued(&first, "t"));
        for _ in 0..MAX_RECORDS {
            let n = node(1);
            record(&scope, &n.id, issued(&n, "t"));
        }
        assert!(!recorded(&scope, &first.id));
        let in_scope = with(|ledger| ledger.entries.keys().filter(|(s, _)| *s == scope).count());
        assert_eq!(in_scope, MAX_RECORDS);
    }
}
