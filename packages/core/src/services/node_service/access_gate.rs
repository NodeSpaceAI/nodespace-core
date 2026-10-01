//! Pre-delete subtree access gate (ADR-041 "CASCADE requires read access across the whole subtree").
//!
//! `delete_subtree_atomic` hard-deletes a `has_child` subtree unconditionally — that's correct
//! for OCC (a concurrent edit to a descendant must not abort the delete) but says nothing about
//! whether the actor may *read* every descendant being destroyed. Core has no actor or access
//! model of its own, so its default gate allows everything. A host that enforces access
//! elsewhere injects its own gate through `NodeService::set_subtree_access_gate`, an extension
//! point (ADR-082).
//!
//! The gate is advisory: the host's own enforcement stays the authority, and the gate only lets
//! the host refuse a local delete before it commits instead of after.

use async_trait::async_trait;

/// Outcome of a pre-delete subtree readability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtreeAccessDecision {
    /// The actor can read every node in the checked set.
    Allowed,
    /// At least one node in the checked set is unreadable by the actor.
    ///
    /// `inaccessible_count` is the minimum disclosure needed to explain the refusal — no
    /// identifying information (ids, names, types) about the inaccessible nodes is carried here.
    Denied { inaccessible_count: u64 },
}

/// Checks whether the current actor may read every node in a `has_child` subtree, before a
/// cascade delete commits.
///
/// Implementors receive the exact id set `delete_subtree_atomic` is about to delete (target +
/// all descendants), computed once by the caller so the walk isn't duplicated.
#[async_trait]
pub trait SubtreeAccessGate: Send + Sync {
    /// `node_ids` is the target node followed by every descendant that would be deleted.
    async fn check_subtree_access(&self, node_ids: &[String]) -> SubtreeAccessDecision;
}

/// Core's default gate: always allows.
///
/// Core has no actor whose read access differs from the local user's, so there is nothing to
/// check access against. This preserves the unconditional-cascade behavior exactly.
pub struct AlwaysAllowGate;

#[async_trait]
impl SubtreeAccessGate for AlwaysAllowGate {
    async fn check_subtree_access(&self, _node_ids: &[String]) -> SubtreeAccessDecision {
        SubtreeAccessDecision::Allowed
    }
}

impl super::NodeService {
    /// Inject the real subtree access gate. A host calls this once its gate is ready.
    /// Silently ignored if called more than once (mirrors `set_embedding_waker`). Works on
    /// `Arc<NodeService>`/any clone since the `OnceLock` is shared via `Arc`.
    pub fn set_subtree_access_gate(&self, gate: std::sync::Arc<dyn SubtreeAccessGate>) {
        let _ = self.subtree_access_gate.set(gate);
    }

    /// The active gate: the injected gate if one has been set, otherwise [`AlwaysAllowGate`]
    /// (core's default).
    pub(crate) fn subtree_access_gate(&self) -> &dyn SubtreeAccessGate {
        // Safe to hand out a `&'static` reference to a stack-local `static` only because
        // AlwaysAllowGate is a zero-sized unit struct with no fields to ever go stale — if it
        // grows state, this would need to move to an `Arc`/`OnceLock` like the injected case.
        static DEFAULT: AlwaysAllowGate = AlwaysAllowGate;
        self.subtree_access_gate
            .get()
            .map(|g| g.as_ref())
            .unwrap_or(&DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn always_allow_gate_allows_any_set() {
        let gate = AlwaysAllowGate;
        let decision = gate
            .check_subtree_access(&["a".to_string(), "b".to_string()])
            .await;
        assert_eq!(decision, SubtreeAccessDecision::Allowed);
    }

    #[tokio::test]
    async fn always_allow_gate_allows_empty_set() {
        let gate = AlwaysAllowGate;
        let decision = gate.check_subtree_access(&[]).await;
        assert_eq!(decision, SubtreeAccessDecision::Allowed);
    }
}
