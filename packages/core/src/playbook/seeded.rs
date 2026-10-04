//! Seeded-play protection (ADR-060 §8).
//!
//! Plays shipped as part of the product are seeded as ordinary, user-visible
//! play nodes — the same DB-seeded, user-modifiable pattern ADR-030 uses for
//! prompts and skills. A seeded play that carries an invariant rule gets two
//! extra behaviors on top of that: it can be reset to its shipped default,
//! and editing its rules or switching it off surfaces a concrete warning
//! rather than silently changing what future nodes get.
//!
//! Convention: a seeded play's node carries `properties._seed.default_rules`
//! — the play's `rules` JSON array exactly as shipped. This mirrors the
//! `_seed` marker prompt/skill/tool nodes already carry
//! (`prepare_nodes_from_template`), reusing the same top-level, `_`-prefixed
//! internal-bookkeeping property rather than inventing a second convention.
//! Storing the default directly on the node (rather than in a compiled-in
//! template registry) is deliberate: "reset" then needs nothing at runtime
//! beyond the node itself, and survives the registry that produced it ever
//! changing shape.

use crate::models::{Node, NodeUpdate, PlayFields};
use crate::playbook::types::{ParsedPlay, ParsedRule, RuleClass};
use crate::services::{NodeService, NodeServiceError};
use std::sync::Arc;

/// Whether `node` (a play node) was seeded — carries a `_seed` marker.
pub fn is_seeded_play(node: &Node) -> bool {
    PlayFields::seeded_in(&node.properties)
}

/// Whether any rule in `rules` is `RuleClass::Invariant`.
pub fn carries_invariant(rules: &[Arc<ParsedRule>]) -> bool {
    rules.iter().any(|r| r.class == RuleClass::Invariant)
}

/// The names of every invariant rule in `rules`, for naming the concrete
/// consequence in a warning message.
pub fn invariant_rule_names(rules: &[Arc<ParsedRule>]) -> Vec<String> {
    rules
        .iter()
        .filter(|r| r.class == RuleClass::Invariant)
        .map(|r| r.name.clone())
        .collect()
}

/// The change to a seeded play that ADR-060 §8 warns about, as the verb the
/// warning uses: `"disabled"` when `enabled` went from `true` to `false`,
/// `"edited"` when its rules changed (ADR-087 §5). `None` for any other
/// update, for a play that is not seeded, and for one that carried no
/// invariant rule.
///
/// `previous` is the play as the engine last read it, which is the play
/// BEFORE the update that produced `node`: the warning is about the invariant
/// rule(s) it carried then.
pub fn warned_change(previous: &ParsedPlay, node: &Node) -> Option<&'static str> {
    if !is_seeded_play(node) || !carries_invariant(&previous.rules) {
        return None;
    }
    let rules = PlayFields::stored_field(&node.properties, nodespace_types::PLAY_RULES_FIELD)
        .cloned()
        .unwrap_or_default();
    if previous.enabled && !PlayFields::enabled_in(&node.properties) {
        Some("disabled")
    } else if rules != previous.stored_rules {
        Some("edited")
    } else {
        None
    }
}

/// Errors specific to resetting a seeded play.
#[derive(Debug, thiserror::Error)]
pub enum ResetSeededPlayError {
    #[error("play '{0}' is not seeded (no properties._seed) — nothing to reset to")]
    NotSeeded(String),
    #[error("play '{0}' has a _seed marker but no default_rules recorded — cannot reset")]
    NoDefaultRulesRecorded(String),
    #[error(transparent)]
    Service(#[from] NodeServiceError),
}

/// Reset a seeded play's rules to their shipped default.
///
/// Reads `properties._seed.default_rules` off the play node itself (see the
/// module doc for why the default lives on the node rather than a separate
/// registry) and writes it back as the play's `rules`, replacing whatever
/// edits the user made. Does not touch any other property (name, status,
/// `_seed` metadata itself) — a reset restores the RULES, not the whole node.
pub async fn reset_seeded_play_to_default(
    node_service: &NodeService,
    play_id: &str,
) -> Result<Node, ResetSeededPlayError> {
    let node = node_service
        .get_node(play_id)
        .await?
        .ok_or_else(|| NodeServiceError::node_not_found(play_id))?;

    let seed = node
        .properties
        .get("_seed")
        .ok_or_else(|| ResetSeededPlayError::NotSeeded(play_id.to_string()))?;
    let default_rules = seed
        .get("default_rules")
        .cloned()
        .ok_or_else(|| ResetSeededPlayError::NoDefaultRulesRecorded(play_id.to_string()))?;

    let update =
        NodeUpdate::default().with_properties(serde_json::json!({ "rules": default_rules }));
    let updated = node_service
        .update_node(play_id, node.version, update)
        .await?;
    Ok(updated)
}

/// Build the warning message surfaced when a seeded play carrying one or
/// more invariant rules is edited or disabled — names the rule(s) and the
/// concrete consequence rather than a generic "this play changed" notice.
///
/// `action` is a short present-tense verb phrase ("edited", "disabled").
pub fn edit_or_disable_warning(play_id: &str, action: &str, invariant_rules: &[String]) -> String {
    let rule_list = invariant_rules.join("', '");
    format!(
        "Seeded play '{play_id}' was {action} while carrying the invariant rule(s) '{rule_list}'. \
         This changes the default for every node created from now on — nodes already created \
         before this change keep the effect they got at creation time and are unaffected. \
         Reset this play to its shipped default to restore the original behavior."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::{GraphEventType, ParsedTrigger};
    use serde_json::json;

    fn rule(name: &str, class: RuleClass) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: name.to_string(),
            class,
            trigger: ParsedTrigger::GraphEvent {
                on: GraphEventType::NodeCreated,
                node_type: "task".to_string(),
                property_key: None,
            },
            conditions: vec![],
            actions: vec![],
        })
    }

    #[test]
    fn is_seeded_play_true_only_with_seed_marker() {
        let seeded = Node::new_with_id(
            "b3a71e70-3e65-5eed-a513-7b6bf83e00da".to_string(),
            "play".to_string(),
            "Seeded Play".to_string(),
            json!({ "rules": [], "_seed": { "default_rules": [] } }),
        );
        let user_authored = Node::new_with_id(
            "458637b5-29b0-5c91-b6ef-f283a95efa88".to_string(),
            "play".to_string(),
            "User Play".to_string(),
            json!({ "rules": [] }),
        );
        assert!(is_seeded_play(&seeded));
        assert!(!is_seeded_play(&user_authored));
    }

    #[test]
    fn carries_invariant_detects_any_invariant_rule() {
        let all_reactive = vec![
            rule("r1", RuleClass::Reactive),
            rule("r2", RuleClass::Reactive),
        ];
        let mixed = vec![
            rule("r1", RuleClass::Reactive),
            rule("r2", RuleClass::Invariant),
        ];
        assert!(!carries_invariant(&all_reactive));
        assert!(carries_invariant(&mixed));
    }

    #[test]
    fn invariant_rule_names_returns_only_invariant_names() {
        let rules = vec![
            rule("reactive-one", RuleClass::Reactive),
            rule("invariant-one", RuleClass::Invariant),
            rule("invariant-two", RuleClass::Invariant),
        ];
        let names = invariant_rule_names(&rules);
        assert_eq!(
            names,
            vec!["invariant-one".to_string(), "invariant-two".to_string()]
        );
    }

    /// A seeded play as the engine last read it, carrying `class` rules.
    fn previous_play(class: RuleClass, stored_rules: serde_json::Value) -> ParsedPlay {
        ParsedPlay {
            id: "pb".to_string(),
            created_at: chrono::Utc::now(),
            rules: vec![rule("r1", class)],
            status: crate::playbook::types::PlayStatus::Runnable,
            enabled: true,
            stored_rules,
        }
    }

    fn seeded_node(play: serde_json::Value) -> Node {
        Node::new_with_id(
            "pb".to_string(),
            "play".to_string(),
            "Seeded Play".to_string(),
            json!({ "play": play, "_seed": { "default_rules": [] } }),
        )
    }

    #[test]
    fn switching_a_seeded_invariant_play_off_is_warned_about() {
        let rules = json!([{ "name": "r1" }]);
        let previous = previous_play(RuleClass::Invariant, rules.clone());

        let off = seeded_node(json!({ "rules": rules, "enabled": false }));
        assert_eq!(warned_change(&previous, &off), Some("disabled"));

        // Already off: nothing changed about the switch.
        let mut was_off = previous.clone();
        was_off.enabled = false;
        assert_eq!(warned_change(&was_off, &off), None);
    }

    #[test]
    fn editing_a_seeded_invariant_plays_rules_is_warned_about() {
        let previous = previous_play(RuleClass::Invariant, json!([{ "name": "r1" }]));
        let edited = seeded_node(json!({ "rules": [{ "name": "r2" }] }));
        assert_eq!(warned_change(&previous, &edited), Some("edited"));
    }

    /// Neither archiving, nor an edit of another field, nor the engine's own
    /// suspension write is the user switching the play off or editing rules.
    #[test]
    fn any_other_update_is_not_warned_about() {
        let rules = json!([{ "name": "r1" }]);
        let previous = previous_play(RuleClass::Invariant, rules.clone());

        let described = seeded_node(json!({ "rules": rules, "description": "new" }));
        assert_eq!(warned_change(&previous, &described), None);

        let suspended = seeded_node(json!({
            "rules": rules,
            "suspended_reason": "action_failed",
            "suspended_at": "2026-10-02T10:00:00Z"
        }));
        assert_eq!(warned_change(&previous, &suspended), None);

        let mut archived = seeded_node(json!({ "rules": rules }));
        archived.lifecycle_status = "archived".to_string();
        assert_eq!(warned_change(&previous, &archived), None);
    }

    #[test]
    fn a_play_that_is_not_seeded_or_carries_no_invariant_is_not_warned_about() {
        let rules = json!([{ "name": "r1" }]);
        let off = seeded_node(json!({ "rules": rules, "enabled": false }));
        assert_eq!(
            warned_change(&previous_play(RuleClass::Reactive, rules.clone()), &off),
            None
        );

        let user_authored = Node::new_with_id(
            "pb".to_string(),
            "play".to_string(),
            "User Play".to_string(),
            json!({ "play": { "rules": rules, "enabled": false } }),
        );
        assert_eq!(
            warned_change(&previous_play(RuleClass::Invariant, rules), &user_authored),
            None
        );
    }

    #[test]
    fn warning_names_the_play_action_and_rules() {
        let msg =
            edit_or_disable_warning("pb-privacy", "disabled", &["default-private".to_string()]);
        assert!(msg.contains("pb-privacy"));
        assert!(msg.contains("disabled"));
        assert!(msg.contains("default-private"));
    }

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::services::NodeService;
        use std::sync::Arc as StdArc;
        use tempfile::TempDir;

        async fn create_test_service() -> (StdArc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: StdArc<SqliteStore> =
                StdArc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = StdArc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        fn seeded_play_node(id: &str, default_rules: serde_json::Value) -> Node {
            Node::new_with_id(
                id.to_string(),
                "play".to_string(),
                "Seeded Play".to_string(),
                json!({
                    "rules": default_rules,
                    "_seed": { "default_rules": default_rules },
                }),
            )
        }

        #[tokio::test]
        async fn reset_restores_edited_rules_to_shipped_default() {
            let (svc, _tmp) = create_test_service().await;

            let default_rules = json!([{
                "name": "default-private",
                "class": "invariant",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                "conditions": [],
                "actions": []
            }]);
            let play = seeded_play_node(
                "f7eb8adf-4f35-5528-8094-91c27c51b810",
                default_rules.clone(),
            );
            svc.create_node(play).await.unwrap();

            // Simulate a user edit: replace rules with something else entirely.
            let edited = crate::models::NodeUpdate::default().with_properties(json!({
                "rules": [{
                    "name": "user-edited",
                    "description": "Test rule",
                    "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                    "conditions": [],
                    "actions": []
                }]
            }));
            let after_edit = svc
                .get_node("f7eb8adf-4f35-5528-8094-91c27c51b810")
                .await
                .unwrap()
                .unwrap();
            svc.update_node(
                "f7eb8adf-4f35-5528-8094-91c27c51b810",
                after_edit.version,
                edited,
            )
            .await
            .unwrap();

            let restored =
                reset_seeded_play_to_default(&svc, "f7eb8adf-4f35-5528-8094-91c27c51b810")
                    .await
                    .expect("reset must succeed for a seeded play with a recorded default");

            let rules = restored
                .properties
                .get("play")
                .and_then(|p| p.get("rules"))
                .cloned()
                .expect("rules must be present after reset");
            assert_eq!(
                rules, default_rules,
                "reset must restore the exact shipped default rules"
            );
        }

        /// A reset changes rule content back while the user may have kept the
        /// shipped description, which an ordinary write would reject as
        /// stale. Restoring the shipped rules is exempt.
        #[tokio::test]
        async fn reset_is_not_rejected_for_a_description_the_user_kept() {
            let (svc, _tmp) = create_test_service().await;
            let id = "0b0cbd6c-5d0c-4f5e-9a55-1f2c8b9c3a11";
            let rule = |expr: &str, description: &str| {
                json!([{
                    "name": "flag-open",
                    "description": "Flag open tasks",
                    "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                    "conditions": [{ "expr": expr, "description": description }],
                    "actions": []
                }])
            };
            let shipped = rule("node.status == 'open'", "The task is open");
            svc.create_node(seeded_play_node(id, shipped.clone()))
                .await
                .unwrap();

            // Change the condition and its description, then put the shipped
            // description back over the changed condition.
            for rules in [
                rule("node.status == 'done'", "The task is done"),
                rule("node.status == 'done'", "The task is open"),
            ] {
                let current = svc.get_node(id).await.unwrap().unwrap();
                svc.update_node(
                    id,
                    current.version,
                    NodeUpdate::default().with_properties(json!({ "rules": rules })),
                )
                .await
                .unwrap();
            }

            let restored = reset_seeded_play_to_default(&svc, id)
                .await
                .expect("a reset is not a stale write");
            assert_eq!(restored.properties["play"]["rules"], shipped);
        }

        #[tokio::test]
        async fn reset_rejects_a_non_seeded_play() {
            let (svc, _tmp) = create_test_service().await;
            let play = Node::new_with_id(
                "19f637b3-bb42-5edc-988d-2d63aef2020f".to_string(),
                "play".to_string(),
                "User Play".to_string(),
                json!({ "rules": [] }),
            );
            svc.create_node(play).await.unwrap();

            let result =
                reset_seeded_play_to_default(&svc, "19f637b3-bb42-5edc-988d-2d63aef2020f").await;
            assert!(
                matches!(result, Err(ResetSeededPlayError::NotSeeded(id)) if id == "19f637b3-bb42-5edc-988d-2d63aef2020f")
            );
        }

        #[tokio::test]
        async fn reset_rejects_a_seed_marker_with_no_recorded_default() {
            let (svc, _tmp) = create_test_service().await;
            let play = Node::new_with_id(
                "6710251f-045c-506e-be5e-ffa8ce5f875a".to_string(),
                "play".to_string(),
                "Broken Seed".to_string(),
                json!({ "rules": [], "_seed": { "tier": "starter" } }),
            );
            svc.create_node(play).await.unwrap();

            let result =
                reset_seeded_play_to_default(&svc, "6710251f-045c-506e-be5e-ffa8ce5f875a").await;
            assert!(matches!(
                result,
                Err(ResetSeededPlayError::NoDefaultRulesRecorded(id)) if id == "6710251f-045c-506e-be5e-ffa8ce5f875a"
            ));
        }
    }
}
