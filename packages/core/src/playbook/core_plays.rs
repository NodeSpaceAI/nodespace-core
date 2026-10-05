//! Plays that ship with the product (ADR-060 §8, ADR-079).
//!
//! A core Play is seeded as an ordinary, user-visible play node carrying
//! `properties._seed.default_rules` — the same DB-seeded, user-modifiable
//! pattern prompts and skills use. It can be inspected, edited, disabled or
//! reset to its shipped default like any other play; see [`super::seeded`].
//!
//! Core Plays are one of the seed tables (ADR-086 §10): each is a
//! [`NodeTemplate`] reconciled by its fixed id on every open, like a seeded
//! skill. A Play added in a later release reaches an existing database, a
//! shipped change replaces a Play nobody edited, and a Play the user edited
//! is kept, with the shipped change recorded for them to decide on (ADR-072,
//! ADR-094 §8). A Play has no body, so its one aspect is its config.

use crate::markdown::{prepare_nodes_from_template, NodeTemplate, SeedTier};
use crate::services::error::NodeServiceError;
use crate::services::NodeService;
use serde_json::json;

/// Node id of the parent-task completion rollup Play (ADR-079).
///
/// A fixed literal UUID rather than one minted per install: ADR-060 §5 keys
/// cross-Play rule ordering on the Play node id, so a random id would make two
/// devices order the same rules differently. A seeded node's identity is its
/// id (ADR-086 §10).
pub const PARENT_TASK_COMPLETION_PLAY_ID: &str = "5dc9b580-8840-4d02-b89d-9aa16ac552fd";

/// The id of every Play that ships with the product: the core play table, in
/// the order the plays are seeded.
pub const CORE_PLAY_IDS: &[&str] = &[PARENT_TASK_COMPLETION_PLAY_ID];

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
        "description": "Mark a task done once every one of its sub-tasks is done or cancelled",
        "class": "reactive",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "select": { "target_type": "task" },
            "property_key": "task.status"
        },
        "conditions": [{
            "expr": "node.child_of.has_child.all(c, c.status == 'done' || c.status == 'cancelled')",
            "description": "Every sub-task of the task's parent is done or cancelled"
        }],
        "actions": [{
            "action_type": "update_node",
            "description": "Mark the parent task done",
            "params": {
                "node_id": "{trigger.node.child_of.id}",
                "properties": { "status": "done" }
            }
        }]
    }])
}

/// The play as shipped, carrying its own default for reset (ADR-060 §8).
fn parent_task_completion_play() -> NodeTemplate {
    let rules = parent_task_completion_rules();
    NodeTemplate {
        id: PARENT_TASK_COMPLETION_PLAY_ID.to_string(),
        title: "Complete a parent task when all its children are done".to_string(),
        markdown_content: String::new(),
        root_node_type: "play".to_string(),
        root_properties: json!({
            "rules": rules,
            "description": "When every sub-task of a task is done or cancelled, \
                            mark the parent done. Reactive rules currently fire \
                            only for changes made on this device.",
            "_seed": { "default_rules": rules },
        }),
        child_node_type: None,
        tier: SeedTier::System,
    }
}

/// Every Play that ships with the product: the core play seed table.
pub fn core_play_templates() -> Vec<NodeTemplate> {
    vec![parent_task_completion_play()]
}

/// Reconcile the core Plays with their seed table.
///
/// Runs on every open, through the reconciliation every seeded kind uses
/// ([`NodeService::seed_nodes_from_templates`]): a missing Play is created
/// under its fixed id, a shipped change replaces a Play nobody edited, and a
/// Play the user edited or switched off is never overwritten.
pub async fn seed_core_plays(service: &NodeService) -> Result<(), NodeServiceError> {
    let mut groups = Vec::new();
    for template in core_play_templates() {
        let nodes = prepare_nodes_from_template(&template).map_err(|e| {
            NodeServiceError::invalid_update(format!(
                "core play '{}' does not expand: {e}",
                template.title
            ))
        })?;
        groups.push(nodes);
    }
    service.seed_nodes_from_templates(groups).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::parse_rules_from_properties;

    #[test]
    fn the_shipped_rules_parse() {
        let props = json!({ "play": { "rules": parent_task_completion_rules() } });
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
            crate::models::PlayFields::seeded_in(&play.root_properties),
            "a core Play must be marked as seeded"
        );
        assert_eq!(
            play.root_properties["_seed"]["default_rules"], play.root_properties["rules"],
            "the stored default must match the shipped rules"
        );
    }

    /// Every core play ships a description on each rule, condition and action
    /// (ADR-090 §1), in its live rules and in its reset target alike.
    #[test]
    fn every_core_play_describes_its_rules_conditions_and_actions() {
        for play in core_play_templates() {
            for rules in [
                &play.root_properties["rules"],
                &play.root_properties["_seed"]["default_rules"],
            ] {
                let rules = parse_rules_from_properties(&json!({ "rules": rules }))
                    .unwrap_or_else(|e| panic!("{}: {e}", play.id));
                assert!(!rules.is_empty(), "{} has no rules", play.id);
                if let Err(errors) = crate::playbook::descriptions::check_descriptions(&rules, None)
                {
                    panic!("{}: {errors:?}", play.id);
                }
            }
        }
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

    /// The id table names exactly the plays that are seeded, in order, and
    /// each is a UUID: no play id is a slug.
    #[test]
    fn the_id_table_matches_the_seeded_plays() {
        let seeded: Vec<String> = core_play_templates()
            .into_iter()
            .map(|play| play.id)
            .collect();
        assert_eq!(seeded, CORE_PLAY_IDS);
        for id in CORE_PLAY_IDS {
            assert!(uuid::Uuid::parse_str(id).is_ok(), "{id} is not a UUID");
        }
    }

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::NodeUpdate;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        /// Seeding never overwrites an existing Play node, so a user's edits to
        /// a core Play survive the next open.
        #[tokio::test]
        async fn re_seeding_keeps_an_existing_plays_edits() {
            let (service, _temp) = create_test_service().await;

            let existing = service
                .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
                .await
                .unwrap()
                .expect("the core Play is seeded");
            service
                .update_node(
                    PARENT_TASK_COMPLETION_PLAY_ID,
                    existing.version,
                    NodeUpdate::default().with_properties(json!({
                        "description": "A description the user wrote."
                    })),
                )
                .await
                .unwrap();

            seed_core_plays(&service).await.unwrap();

            let stored = service
                .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
                .await
                .unwrap()
                .unwrap();
            // A persisted Play keeps its properties under the `play` namespace.
            assert_eq!(
                stored.properties["play"]["description"],
                "A description the user wrote."
            );
        }
    }
}
