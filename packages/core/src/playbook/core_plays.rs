//! Plays that ship with the product (ADR-060 §8, ADR-079).
//!
//! A core Play is seeded as an ordinary, user-visible play node carrying
//! `properties._seed.default_rules` — the same DB-seeded, user-modifiable
//! pattern prompts and skills use. It can be inspected, edited, disabled or
//! reset to its shipped default like any other play; see [`super::seeded`].
//!
//! Seeding reconciles per play id rather than gating on "has anything been
//! seeded", so a Play added after a database's first run still reaches that
//! database — the same per-item reconciliation `seed_core_schemas_if_needed`
//! uses. An existing play node is left untouched, so a user's edits survive
//! startup (ADR-072).

use crate::models::Node;
use crate::services::error::NodeServiceError;
use crate::services::NodeService;
use serde_json::json;

/// Node id of the parent-task completion rollup Play (ADR-079).
///
/// Stable and hardcoded rather than minted per install: ADR-060 §5 keys
/// cross-Play rule ordering on the Play node id, so a random id would make two
/// devices order the same rules differently.
pub const PARENT_TASK_COMPLETION_PLAY_ID: &str = "play-core-parent-task-completion";

/// The rollup rule, as shipped (ADR-079).
///
/// Trigger, condition and action in one rule:
///
/// - **Trigger** — a `task`'s `status` changes. Registered against `task`, so
///   per ADR-078 it also fires for any type extending `task`, and the
///   condition is evaluated at `task`'s scope: an extending type's own status
///   vocabulary resolves through `maps_to` before comparison, and its own
///   fields are not visible. A Play written here keeps working when an `issue`
///   type appears, without knowing `issue` exists.
/// - **Condition** — walk `child_of` to the parent, then back down its
///   `has_child` children, and require every one to be finished. `cancelled`
///   counts as finished alongside `done`: the question is whether any work
///   remains under the parent, not whether everything succeeded. A parent with
///   no children never completes, because an empty collection evaluates to
///   `false` here — including under `.all()` — so no explicit count guard is
///   needed.
///
/// **Single-parent assumption.** `child_of` is the outline's parent edge, and
/// the whole hierarchy is single-parent by construction: `SqliteStore::get_parent`
/// and `get_parent_id` both resolve it with `LIMIT 1`, so no read path has ever
/// contemplated a second one. A node holding two `has_child` parents is already
/// malformed with respect to that model — nothing in the product creates one —
/// and this Play no-ops there rather than picking a parent arbitrarily: the
/// resolver yields a `Collection` for a multi-row walk, `.has_child` cannot
/// continue from it, and the condition is simply false. That is the desired
/// failure: declining to act on a malformed hierarchy, not silently completing
/// whichever parent happened to sort first.
/// - **Action** — set the parent's `status` to `done`. Never `cancelled`: a
///   parent whose children were all cancelled has completed as a unit of work,
///   and propagating `cancelled` upward would assert an intent this Play has no
///   basis to claim.
///
/// The rule is `reactive`, not `invariant`. ADR-060 §2 restricts invariants to
/// "non-chaining, depth 1", and this rule chains by construction: its own write
/// to the parent is itself a `task` status change, which re-fires the rule with
/// the parent now in the child position. That is how the rollup reaches a
/// grandparent, and it is bounded by the engine's chain-depth cap.
pub fn parent_task_completion_rules() -> serde_json::Value {
    json!([{
        "name": "complete-parent-when-all-children-done",
        "class": "reactive",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "node_type": "task",
            "property_key": "task.status"
        },
        "conditions": [
            "node.child_of.has_child.all(c, c.status == 'done' || c.status == 'cancelled')"
        ],
        "actions": [{
            "action_type": "update_node",
            "params": {
                "node_id": "{trigger.node.child_of.id}",
                "properties": { "status": "done" }
            }
        }]
    }])
}

/// The play node as shipped, carrying its own default for reset (ADR-060 §8).
fn parent_task_completion_play() -> Node {
    let rules = parent_task_completion_rules();
    Node::new_with_id(
        PARENT_TASK_COMPLETION_PLAY_ID.to_string(),
        "play".to_string(),
        "Complete a parent task when all its children are done".to_string(),
        json!({
            "rules": rules,
            "description": "When every sub-task of a task is done or cancelled, \
                            mark the parent done. Reactive rules currently fire \
                            only for changes made on this device.",
            "_seed": { "default_rules": rules },
        }),
    )
}

/// Every Play that ships with the product.
fn core_plays() -> Vec<Node> {
    vec![parent_task_completion_play()]
}

/// Seed the core Plays, skipping any that already exist.
///
/// Idempotent, and reconciled per play id: a Play added in a later release
/// reaches an existing database on its next open. An existing play node is
/// never overwritten — a user may have edited or disabled it, and ADR-060 §8's
/// reset path (not a silent re-seed) is how the shipped default is restored.
pub async fn seed_core_plays_if_needed(service: &NodeService) -> Result<(), NodeServiceError> {
    for play in core_plays() {
        if service.get_node(&play.id).await?.is_some() {
            continue;
        }
        let id = service.create_node(play).await?;
        tracing::info!(node_id = %id, "🌱 Seeded core Play (ADR-079)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::parse_rules_from_properties;

    #[test]
    fn the_shipped_rules_parse() {
        let props = json!({ "rules": parent_task_completion_rules() });
        let rules = parse_rules_from_properties(&props).expect("shipped rules must parse");
        assert_eq!(rules.len(), 1);
    }

    /// ADR-060 §2: an invariant must be non-chaining, and this rule chains by
    /// construction. Declaring it invariant would be rejected at save time.
    #[test]
    fn the_rollup_rule_is_reactive() {
        let rules = parent_task_completion_rules();
        assert_eq!(
            rules[0]["class"], "reactive",
            "the rollup chains, so it cannot be an invariant"
        );
    }

    /// The seeded node must carry its own shipped default, or ADR-060 §8's
    /// reset has nothing to restore from.
    #[test]
    fn the_seeded_play_carries_its_default_rules() {
        let play = parent_task_completion_play();
        assert!(
            crate::playbook::seeded::is_seeded_play(&play),
            "a core Play must be marked as seeded"
        );
        assert_eq!(
            play.properties["_seed"]["default_rules"], play.properties["rules"],
            "the stored default must match the shipped rules"
        );
    }

    /// The id is load-bearing for cross-device rule ordering (ADR-060 §5), so
    /// it must not drift.
    #[test]
    fn the_play_id_is_stable() {
        assert_eq!(
            parent_task_completion_play().id,
            PARENT_TASK_COMPLETION_PLAY_ID
        );
    }
}
