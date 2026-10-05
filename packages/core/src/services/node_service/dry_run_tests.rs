//! A dry run of an update (ADR-094 §9): the invariant rules a write would
//! run, evaluated against the proposed change, with nothing written.
//!
//! Every test asks the same change twice, as a dry run and as the write, and
//! holds the two to one answer: the dry run is the write's own rule loop in
//! its conditions-only mode, so a rule cannot say one thing to a prediction
//! and another to the write it predicts.

use super::*;
use crate::db::SqliteStore;
use crate::playbook::PlaybookEngine;
use serde_json::json;
use tempfile::TempDir;

const DOC: &str = "dr_doc";
const PLAY_ID: &str = "d7000000-0000-4000-8000-000000000001";

/// A database with a `dr_doc` type and one active play holding `rules`. The
/// engine is returned so the lifecycle the service reads rules from stays
/// alive for the test.
async fn service_with_rules(
    rules: serde_json::Value,
) -> (Arc<NodeService>, PlaybookEngine, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let mut store = Arc::new(
        SqliteStore::new(temp_dir.path().join("test.db"))
            .await
            .unwrap(),
    );
    let service = Arc::new(NodeService::new(&mut store).await.unwrap());
    crate::schema::handle_create_schema(
        &service,
        json!({
            "name": "dr_reviewer",
            "fields": [{ "name": "name", "type": "text" }]
        }),
    )
    .await
    .expect("reviewer schema creation failed");
    crate::schema::handle_create_schema(
        &service,
        json!({
            "name": DOC,
            "fields": [
                { "name": "stage", "type": "text" },
                { "name": "verified", "type": "boolean" }
            ],
            "relationships": [{
                "name": "reviewer",
                "targetType": "dr_reviewer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "reviews",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect("doc schema creation failed");

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play = Node::new_with_id(
        PLAY_ID.to_string(),
        "play".to_string(),
        "dry-run-test-play".to_string(),
        json!({ "play": { "rules": rules } }),
    );
    engine
        .lifecycle()
        .write()
        .unwrap()
        .activate_play(&play)
        .expect("play must parse and activate");
    (service, engine, temp_dir)
}

/// An invariant rule on a change of `dr_doc.stage`.
fn on_stage_change(
    name: &str,
    conditions: serde_json::Value,
    actions: serde_json::Value,
) -> serde_json::Value {
    json!({
        "name": name,
        "class": "invariant",
        "description": "Test rule",
        "trigger": {
            "type": "graph_event", "on": "property_changed",
            "select": { "target_type": DOC },
            "property_key": format!("{DOC}.stage")
        },
        "conditions": conditions,
        "actions": actions
    })
}

fn reject(message: &str) -> serde_json::Value {
    json!([{
        "description": "Refuse the change",
        "action_type": "reject",
        "params": { "message": message }
    }])
}

fn expr(condition: &str) -> serde_json::Value {
    json!([{ "expr": condition, "description": "Test condition" }])
}

async fn doc(service: &NodeService, stage: &str) -> Node {
    let id = service
        .create_node(Node::new(
            DOC.to_string(),
            "A doc".to_string(),
            json!({ "stage": stage }),
        ))
        .await
        .unwrap();
    service.get_node(&id).await.unwrap().unwrap()
}

fn stage(value: &str) -> NodeUpdate {
    NodeUpdate::default().with_properties(json!({ "stage": value }))
}

/// A rejection is reported with the rule's message, play and rule name, the
/// write is refused by that same rule with that same message, and the dry
/// run leaves the node as it was: same fields, same version.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_names_the_rule_that_would_reject_and_writes_nothing() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "no-skipping-review",
        // The event a real write carries: the stored value as old, the
        // proposed value as new.
        expr("trigger.property.old_value == 'draft' && trigger.property.new_value == 'final'"),
        reject("A {trigger.node.stage} doc was a draft: review it first."),
    )]))
    .await;
    let draft = doc(&service, "draft").await;

    let verdict = service
        .dry_run_update(&draft, stage("final"))
        .await
        .unwrap();
    assert_eq!(
        verdict,
        DryRunVerdict::Rejected {
            play_id: PLAY_ID.to_string(),
            rule_name: "no-skipping-review".to_string(),
            // The message's binding reads the proposed node, as on a write.
            message: "A final doc was a draft: review it first.".to_string(),
        }
    );
    assert_eq!(
        service.get_node(&draft.id).await.unwrap().unwrap(),
        draft,
        "a dry run must leave the node exactly as it was"
    );

    let written = service
        .update_node(&draft.id, draft.version, stage("final"))
        .await;
    match written {
        Err(NodeServiceError::PlayRuleRejected {
            play_id: by_play,
            rule_name,
            message,
            ..
        }) => {
            assert_eq!(by_play, PLAY_ID);
            assert_eq!(rule_name, "no-skipping-review");
            assert_eq!(message, "A final doc was a draft: review it first.");
        }
        other => panic!("the write must be refused as the dry run said, got {other:?}"),
    }

    // The change beside it, which the rule's condition does not hold for.
    assert_eq!(
        service
            .dry_run_update(&draft, stage("review"))
            .await
            .unwrap(),
        DryRunVerdict::Allowed
    );
    assert_eq!(
        service.get_node(&draft.id).await.unwrap().unwrap().version,
        draft.version,
        "an allowed dry run changes no version either"
    );
    service
        .update_node(&draft.id, draft.version, stage("review"))
        .await
        .expect("the write the dry run allowed");
}

/// Of a rule's actions, a dry run carries out `reject` alone: an action that
/// would write is not run, so its effect is neither made nor predicted.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_executes_no_action_other_than_reject() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "stamp-verified",
        json!([]),
        json!([{
            "description": "Stamp the doc",
            "action_type": "update_node",
            "params": { "node_id": "{trigger.node.id}", "properties": { "verified": true } }
        }]),
    )]))
    .await;
    let draft = doc(&service, "draft").await;

    assert_eq!(
        service
            .dry_run_update(&draft, stage("final"))
            .await
            .unwrap(),
        DryRunVerdict::Allowed
    );
    assert_eq!(
        service.get_node(&draft.id).await.unwrap().unwrap(),
        draft,
        "the rule's update_node action must not have run"
    );

    // The write runs it.
    let written = service
        .update_node(&draft.id, draft.version, stage("final"))
        .await
        .unwrap();
    assert_eq!(written.properties[DOC]["verified"], json!(true));
}

/// A change that touches nothing a rule is triggered by matches no rule, in
/// a dry run as on a write.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_matches_rules_by_what_the_change_touches() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "always-reject",
        json!([]),
        reject("stage is frozen"),
    )]))
    .await;
    let draft = doc(&service, "draft").await;

    let other_field = NodeUpdate::default().with_properties(json!({ "verified": true }));
    assert_eq!(
        service.dry_run_update(&draft, other_field).await.unwrap(),
        DryRunVerdict::Allowed
    );
    // Setting the field to the value it already has is no change.
    assert_eq!(
        service
            .dry_run_update(&draft, stage("draft"))
            .await
            .unwrap(),
        DryRunVerdict::Allowed
    );
    assert!(matches!(
        service.dry_run_update(&draft, stage("final")).await.unwrap(),
        DryRunVerdict::Rejected { message, .. } if message == "stage is frozen"
    ));
}

/// What fails the write before any rule runs fails the dry run the same way:
/// it is not reported as allowed.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_fails_as_the_write_does_before_a_rule_runs() {
    let (service, _engine, _tmp) = service_with_rules(json!([])).await;
    let draft = doc(&service, "draft").await;

    let empty = service.dry_run_update(&draft, NodeUpdate::default()).await;
    assert!(
        matches!(empty, Err(NodeServiceError::InvalidUpdate(_))),
        "{empty:?}"
    );

    let wrong_type = NodeUpdate::default().with_properties(json!({ "verified": "yes" }));
    let dry = service.dry_run_update(&draft, wrong_type.clone()).await;
    let written = service
        .update_node(&draft.id, draft.version, wrong_type)
        .await;
    assert!(dry.is_err(), "{dry:?}");
    assert_eq!(
        dry.unwrap_err().to_string(),
        written.unwrap_err().to_string()
    );
}

/// A rule that cannot be evaluated is reported as unresolved, naming the
/// rule, and is not an allowance: the write fails closed on it too. Here the
/// rejection's own message binds a value that does not resolve.
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_that_cannot_be_evaluated_is_reported_as_unresolved_and_is_not_allowed() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "needs-a-reviewer",
        expr("!has(node.reviewer)"),
        reject("{trigger.node.reviewer.name}"),
    )]))
    .await;
    let draft = doc(&service, "draft").await;

    let verdict = service
        .dry_run_update(&draft, stage("final"))
        .await
        .unwrap();
    match &verdict {
        DryRunVerdict::Unresolved {
            play_id,
            rule_name,
            reason,
        } => {
            assert_eq!(play_id, PLAY_ID);
            assert_eq!(rule_name, "needs-a-reviewer");
            assert!(!reason.is_empty());
        }
        other => panic!("a rule that cannot be evaluated must be unresolved, got {other:?}"),
    }
    assert!(!verdict.is_allowed());

    let written = service
        .update_node(&draft.id, draft.version, stage("final"))
        .await;
    assert!(
        matches!(
            &written,
            Err(NodeServiceError::InvariantRuleFailed { rule_name, .. })
                if rule_name == "needs-a-reviewer"
        ),
        "the write fails closed on the same rule, got {written:?}"
    );
}

/// On a database a condition cannot be read from, a dry run never answers
/// "allowed": a failed read is unresolved or an error, as it is on the write.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_on_an_unreadable_graph_is_never_an_allowance() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "needs-a-reviewer",
        expr("!has(node.reviewer)"),
        reject("a doc needs a reviewer before it moves on"),
    )]))
    .await;
    let draft = doc(&service, "draft").await;

    // Healthy: the doc has no reviewer, so the rule rejects.
    assert!(matches!(
        service
            .dry_run_update(&draft, stage("final"))
            .await
            .unwrap(),
        DryRunVerdict::Rejected { .. }
    ));

    // The walk to `reviewer` reads the relationship table.
    service
        .store()
        .write()
        .await
        .execute("DROP TABLE relationship", ())
        .await
        .expect("dropping the relationship table should succeed");

    let dry = service.dry_run_update(&draft, stage("final")).await;
    assert!(
        !matches!(dry, Ok(DryRunVerdict::Allowed)),
        "an unreadable graph must not read as allowed, got {dry:?}"
    );
    assert!(service
        .update_node(&draft.id, draft.version, stage("final"))
        .await
        .is_err());
}

/// No rule fires on an archived node, so a dry run of a change to one is
/// allowed, as the write is.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_of_a_change_to_an_archived_node_is_allowed() {
    let (service, _engine, _tmp) = service_with_rules(json!([on_stage_change(
        "always-reject",
        json!([]),
        reject("stage is frozen"),
    )]))
    .await;
    let draft = doc(&service, "draft").await;
    let archived = service
        .update_node(
            &draft.id,
            draft.version,
            NodeUpdate {
                lifecycle_status: Some("archived".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        service
            .dry_run_update(&archived, stage("final"))
            .await
            .unwrap(),
        DryRunVerdict::Allowed
    );
    service
        .update_node(&archived.id, archived.version, stage("final"))
        .await
        .expect("no rule vetoes a write to an archived node");
}
