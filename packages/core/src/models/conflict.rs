//! Conflict journal types (ADR-068): a durable, resolvable record of a
//! convergence conflict, stored in the local-only `conflict` +
//! `conflict_participant` tables (see `db::migrations::v005_conflict_journal`).
//!
//! Replaces the `_possible_duplicate` boolean property, which could never be
//! reached on a local-only install, could not name the counterparty, and
//! could never be cleared once set (ADR-068).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The kind of convergence conflict a record represents. A closed set —
/// unlike `TaskStatus`/`TaskPriority`, there is no user-defined extension
/// point here, so an unrecognized stored value is a genuine data error, not a
/// legitimate "user-defined kind" the way `TaskStatus::User(_)` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// Two active nodes of the same type hold the same value in a
    /// schema-declared `unique`/`unique_case_insensitive` field.
    UniqueFieldCollision,
    /// Two active collections share a (case-folded) name.
    CollectionNameCollision,
    /// A local edit was overwritten by last-writer-wins during sync apply.
    /// Reserved for the Recovered Items fold-in (S2); no writer yet.
    SupersededEdit,
    /// N devices independently created a node for the same playbook trigger.
    /// Reserved for the ADR-060 playbook work; no detection yet.
    DuplicateReactiveCreate,
}

impl ConflictKind {
    /// Wire/storage form — the exact string persisted in `conflict.kind` and
    /// used as part of the deterministic id's seed (see
    /// `services::node_service::conflicts::deterministic_conflict_id`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UniqueFieldCollision => "unique_field_collision",
            Self::CollectionNameCollision => "collection_name_collision",
            Self::SupersededEdit => "superseded_edit",
            Self::DuplicateReactiveCreate => "duplicate_reactive_create",
        }
    }
}

impl fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ConflictKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unique_field_collision" => Ok(Self::UniqueFieldCollision),
            "collection_name_collision" => Ok(Self::CollectionNameCollision),
            "superseded_edit" => Ok(Self::SupersededEdit),
            "duplicate_reactive_create" => Ok(Self::DuplicateReactiveCreate),
            other => Err(format!("unknown ConflictKind: '{other}'")),
        }
    }
}

/// A conflict record's resolution lifecycle. `Open` is the only state that
/// re-detection (`record_conflict`) may write into — `Resolved`/`Dismissed`
/// are terminal until a human acts again via `resolve_conflict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictStatus {
    Open,
    Resolved,
    Dismissed,
}

impl ConflictStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }
}

impl fmt::Display for ConflictStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ConflictStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            "dismissed" => Ok(Self::Dismissed),
            other => Err(format!("unknown ConflictStatus: '{other}'")),
        }
    }
}

/// A row of the `conflict` table joined with its `conflict_participant` rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    pub id: String,
    pub kind: ConflictKind,
    /// Participants, lexicographically sorted (ADR-068 §3 — sorting is what
    /// makes the derived id symmetric regardless of arrival order).
    pub node_ids: Vec<String>,
    /// Kind-specific evidence captured AT DETECTION TIME, so the resolution
    /// surface renders from the record and never re-derives (the exact
    /// failure mode the old badge's `findDuplicateFor` re-run had).
    pub detail: serde_json::Value,
    pub status: ConflictStatus,
    pub detected_at: String,
    pub detected_by: Option<String>,
    pub occurrences: i64,
    pub last_seen_at: String,
    pub resolved_at: Option<String>,
    pub resolution: Option<serde_json::Value>,
}

/// A resolution action, per conflict-journal-and-resolution.md §2.5/§5.1.
/// Serializes to the exact JSON shape stored in `conflict.resolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Resolution {
    Dismiss,
    AdoptExisting {
        adopted: String,
    },
    Rename {
        renamed: String,
        from: String,
        to: String,
    },
    Restore {
        restored_to: String,
    },
    Merge {
        survivor: String,
        loser: String,
        superseded: serde_json::Value,
        edges_repointed: u32,
        edges_dropped: u32,
    },
    /// Closed by the reconciliation sweep (conflict-journal-and-resolution.md
    /// §5.4), not a human: a participant was hard-deleted, or the kind's
    /// predicate no longer finds a collision (renamed, merged elsewhere on
    /// another device — §5.3's cross-device self-resolution). `reason` is a
    /// short machine string (`"participant_deleted"` |
    /// `"no_longer_conflicting"`) so the Conflicts view can label a
    /// self-resolved record distinctly from one a user actually decided.
    SelfResolved {
        reason: String,
    },
}

impl Resolution {
    /// The terminal `ConflictStatus` this resolution transitions a record
    /// into. Only `Dismiss` yields `Dismissed`; every other resolution is a
    /// genuine decision that a conflict is settled, not merely acknowledged.
    pub fn terminal_status(&self) -> ConflictStatus {
        match self {
            Self::Dismiss => ConflictStatus::Dismissed,
            Self::AdoptExisting { .. }
            | Self::Rename { .. }
            | Self::Restore { .. }
            | Self::Merge { .. }
            | Self::SelfResolved { .. } => ConflictStatus::Resolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_kind_round_trips_through_as_str() {
        for kind in [
            ConflictKind::UniqueFieldCollision,
            ConflictKind::CollectionNameCollision,
            ConflictKind::SupersededEdit,
            ConflictKind::DuplicateReactiveCreate,
        ] {
            assert_eq!(ConflictKind::from_str(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn conflict_status_round_trips_through_as_str() {
        for status in [
            ConflictStatus::Open,
            ConflictStatus::Resolved,
            ConflictStatus::Dismissed,
        ] {
            assert_eq!(ConflictStatus::from_str(status.as_str()).unwrap(), status);
        }
    }

    #[test]
    fn resolution_terminal_status() {
        assert_eq!(
            Resolution::Dismiss.terminal_status(),
            ConflictStatus::Dismissed
        );
        assert_eq!(
            Resolution::AdoptExisting {
                adopted: "n1".into()
            }
            .terminal_status(),
            ConflictStatus::Resolved
        );
    }

    #[test]
    fn resolution_merge_serializes_with_action_tag() {
        let resolution = Resolution::Merge {
            survivor: "s1".into(),
            loser: "l1".into(),
            superseded: serde_json::json!({"email": "old@example.com"}),
            edges_repointed: 3,
            edges_dropped: 1,
        };
        let json = serde_json::to_value(&resolution).unwrap();
        assert_eq!(json["action"], "merge");
        assert_eq!(json["survivor"], "s1");
        assert_eq!(json["edges_dropped"], 1);
    }
}
