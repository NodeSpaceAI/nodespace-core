//! A `system` field's value is written only by the code that maintains it
//! (Schema Management, Protection; ADR-112 section 12). Every node write path
//! refuses a write that sets or changes one and names the field and the type;
//! the maintaining code writes through `maintaining_system_fields`, which no
//! user-facing surface calls.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate, PlaySuspensionReason};
use nodespace_core::ops::node_ops;
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Arc::new(
        SqliteStore::new(tmp.path().join("test.db"))
            .await
            .expect("store"),
    );
    (
        Arc::new(NodeService::new(&mut store).await.expect("service")),
        tmp,
    )
}

fn pty_chat(properties: Value) -> Node {
    Node::new("ai-chat-pty".into(), "Session".into(), properties)
}

async fn a_pty_chat(svc: &NodeService) -> Node {
    let id = svc
        .create_node(pty_chat(json!({ "agent": "claude-code" })))
        .await
        .expect("a chat without system fields is created");
    svc.get_node(&id).await.unwrap().unwrap()
}

async fn a_play(svc: &NodeService) -> Node {
    let id = svc
        .create_node(Node::new(
            "play".into(),
            "A play".into(),
            json!({ "rules": [] }),
        ))
        .await
        .expect("a play");
    svc.get_node(&id).await.unwrap().unwrap()
}

/// The refusal names the field and the type that declares it.
fn assert_refused<T: std::fmt::Debug>(
    what: &str,
    result: Result<T, impl std::fmt::Display>,
    field: &str,
    declared_by: &str,
) {
    let message = result
        .expect_err(&format!("{what}: a system field write must be refused"))
        .to_string();
    assert!(message.contains(&format!("'{field}'")), "{what}: {message}");
    assert!(
        message.contains(&format!("'{declared_by}'")),
        "{what}: {message}"
    );
}

#[tokio::test]
async fn every_create_path_refuses_a_system_field() {
    let (svc, _tmp) = test_service().await;
    let props = || json!({ "agent": "claude-code", "summary": "forged" });

    assert_refused(
        "create_node",
        svc.create_node(pty_chat(props())).await,
        "summary",
        "ai-chat",
    );
    // Named in its declaring bucket, not flat.
    assert_refused(
        "create_node, bucketed",
        svc.create_node(pty_chat(json!({
            "ai-chat": { "agent": "claude-code", "summary": "forged" }
        })))
        .await,
        "summary",
        "ai-chat",
    );
    assert_refused(
        "a subtype's own field",
        svc.create_node(pty_chat(json!({ "agent": "claude-code", "exit_code": 0 })))
            .await,
        "exit_code",
        "ai-chat-pty",
    );
    assert_refused(
        "create_node_with_parent",
        svc.create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "ai-chat-pty".into(),
            content: "Session".into(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: props(),
            lifecycle_status: None,
        })
        .await,
        "summary",
        "ai-chat",
    );
    assert_refused(
        "bulk_create",
        svc.bulk_create(vec![pty_chat(
            json!({ "ai-chat": { "agent": "claude-code", "summary": "forged" } }),
        )])
        .await,
        "summary",
        "ai-chat",
    );
    for (name, trusted) in [("bulk_create_hierarchy", false), ("trusted import", true)] {
        let row = || {
            vec![(
                "00000000-0000-4000-8000-0000000000a1".to_string(),
                "ai-chat-pty".to_string(),
                "Session".to_string(),
                None,
                1.0,
                props(),
            )]
        };
        let result = if trusted {
            svc.bulk_create_hierarchy_trusted(row()).await
        } else {
            svc.bulk_create_hierarchy(row()).await
        };
        assert_refused(name, result, "summary", "ai-chat");
    }
    assert_refused(
        "node_ops::create_node",
        node_ops::create_node(
            &svc,
            node_ops::CreateNodeInput {
                id: None,
                node_type: "ai-chat-pty".into(),
                content: "Session".into(),
                parent_id: None,
                position: InsertPositionOwned::End,
                properties: props(),
                collections: vec![],
                collection_ids: vec![],
                lifecycle_status: None,
            },
        )
        .await,
        "summary",
        "ai-chat",
    );
    let refusal = svc
        .create_node(Node::new(
            "play".into(),
            "A play".into(),
            json!({ "rules": [], "suspended_at": "2026-10-02T10:00:00Z" }),
        ))
        .await
        .expect_err("a play is not created suspended")
        .to_string();
    assert!(refusal.contains("'suspended_at'"), "{refusal}");
}

#[tokio::test]
async fn every_update_path_refuses_a_system_field() {
    let (svc, _tmp) = test_service().await;
    let chat = a_pty_chat(&svc).await;
    let patch = || NodeUpdate::new().with_properties(json!({ "summary": "forged" }));

    assert_refused(
        "update_node",
        svc.update_node(&chat.id, chat.version, patch()).await,
        "summary",
        "ai-chat",
    );
    assert_refused(
        "update_node_unchecked",
        svc.update_node_unchecked(&chat.id, patch()).await,
        "summary",
        "ai-chat",
    );
    assert_refused(
        "bulk_update",
        svc.bulk_update(vec![(chat.id.clone(), patch())]).await,
        "summary",
        "ai-chat",
    );
    assert_refused(
        "a bucketed patch",
        svc.update_node(
            &chat.id,
            chat.version,
            NodeUpdate::new().with_properties(json!({ "ai-chat-pty": { "session_id": "x" } })),
        )
        .await,
        "session_id",
        "ai-chat-pty",
    );
    assert_refused(
        "node_ops::update_node",
        node_ops::update_node(
            &svc,
            node_ops::UpdateNodeInput {
                node_id: chat.id.clone(),
                version: None,
                node_type: None,
                content: None,
                properties: Some(json!({ "transcript": "forged" })),
                add_to_collections: vec![],
                add_to_collection_ids: vec![],
                remove_from_collection_ids: vec![],
                lifecycle_status: None,
            },
        )
        .await,
        "transcript",
        "ai-chat-pty",
    );
    let stored = svc.get_node(&chat.id).await.unwrap().unwrap();
    assert_eq!(
        stored.properties, chat.properties,
        "a refusal changes nothing"
    );
}

#[tokio::test]
async fn a_user_update_cannot_set_a_play_suspension_but_the_engine_does() {
    let (svc, _tmp) = test_service().await;
    let play = a_play(&svc).await;

    let refusal = node_ops::update_node(
        &svc,
        node_ops::UpdateNodeInput {
            node_id: play.id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(json!({ "suspended_at": "2026-10-02T10:00:00Z" })),
            add_to_collections: vec![],
            add_to_collection_ids: vec![],
            remove_from_collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await
    .expect_err("a user-facing update cannot set a suspension")
    .to_string();
    assert!(refusal.contains("'suspended_at'"), "{refusal}");
    let stored = svc.get_node(&play.id).await.unwrap().unwrap();
    assert!(!nodespace_core::models::PlayFields::suspended_in(
        &stored.properties
    ));

    svc.record_play_suspension(&play.id, PlaySuspensionReason::ActionFailed, "boom")
        .await
        .expect("the engine records a suspension");
    let stored = svc.get_node(&play.id).await.unwrap().unwrap();
    assert!(nodespace_core::models::PlayFields::suspended_in(
        &stored.properties
    ));
}

#[tokio::test]
async fn a_write_that_leaves_a_system_field_as_stored_is_not_refused() {
    let (svc, _tmp) = test_service().await;
    let chat = a_pty_chat(&svc).await;
    svc.maintaining_system_fields(svc.update_node(
        &chat.id,
        chat.version,
        NodeUpdate::new().with_properties(json!({ "summary": "Fixed the build" })),
    ))
    .await
    .expect("the maintainer writes the field");
    let chat = svc.get_node(&chat.id).await.unwrap().unwrap();

    // A client that sends the node's fields back whole, with its own edit.
    svc.update_node(
        &chat.id,
        chat.version,
        NodeUpdate::new()
            .with_content("Renamed".into())
            .with_properties(json!({ "summary": "Fixed the build", "agent": "claude-code" })),
    )
    .await
    .expect("an unchanged system field is not a change");
    let chat = svc.get_node(&chat.id).await.unwrap().unwrap();
    assert_eq!(chat.properties["ai-chat"]["summary"], "Fixed the build");

    // Clearing it is a change.
    assert_refused(
        "clearing",
        svc.update_node(
            &chat.id,
            chat.version,
            NodeUpdate::new().with_properties(json!({ "summary": null })),
        )
        .await,
        "summary",
        "ai-chat",
    );
}

#[tokio::test]
async fn the_maintainer_path_writes_a_system_field_and_only_inside_its_scope() {
    let (svc, _tmp) = test_service().await;
    let id = svc
        .maintaining_system_fields(
            svc.create_node(pty_chat(json!({ "agent": "claude-code", "exit_code": 3 }))),
        )
        .await
        .expect("the maintainer creates with a system field");
    let chat = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(chat.properties["ai-chat-pty"]["exit_code"], 3);

    // The scope ends with the write: the next one is an ordinary write.
    assert_refused(
        "after the scope",
        svc.update_node(
            &id,
            chat.version,
            NodeUpdate::new().with_properties(json!({ "exit_code": 4 })),
        )
        .await,
        "exit_code",
        "ai-chat-pty",
    );
    // A field that is not `system` is written as always.
    svc.update_node(
        &id,
        chat.version,
        NodeUpdate::new().with_properties(json!({ "session_status": "ended" })),
    )
    .await
    .expect("a non-system field is not held back");
}

#[tokio::test]
async fn a_user_rule_triggered_by_a_maintainers_write_cannot_set_a_system_field() {
    let (svc, _tmp) = test_service().await;
    let engine = nodespace_core::PlaybookEngine::new(Arc::clone(&svc));
    svc.set_playbook_lifecycle(engine.lifecycle().clone());
    let victim = a_pty_chat(&svc).await;
    let rule_play = Node::new(
        "play".into(),
        "forge".into(),
        json!({ "play": { "rules": [{
            "name": "forge-summary",
            "class": "invariant",
            "description": "A user's rule",
            "trigger": { "type": "graph_event", "on": "node_created",
                         "select": { "target_type": "ai-chat-message" } },
            "conditions": [],
            "actions": [{
                "description": "Forge the summary",
                "action_type": "update_node",
                "params": { "node_id": victim.id, "properties": { "summary": "forged" } }
            }]
        }] } }),
    );
    engine
        .lifecycle()
        .write()
        .unwrap()
        .activate_play(&rule_play)
        .expect("the play activates");

    let chat = svc
        .create_node(Node::new(
            "ai-chat-native".into(),
            "Chat".into(),
            json!({ "agent": "nodespace" }),
        ))
        .await
        .unwrap();
    let message = |parent: String| CreateNodeParams {
        id: None,
        node_type: "ai-chat-message".into(),
        content: "hello".into(),
        parent_id: Some(parent),
        position: InsertPositionOwned::End,
        properties: json!({ "role": "assistant", "timestamp": "2026-10-02T10:00:00Z",
                            "outcome": "replied" }),
        lifecycle_status: None,
    };
    let appended = svc
        .maintaining_system_fields(svc.create_node_with_parent(message(chat.clone())))
        .await;
    let stored = svc.get_node(&victim.id).await.unwrap().unwrap();
    assert!(
        stored.properties["ai-chat"]
            .get("summary")
            .is_none_or(Value::is_null),
        "the rule forged a system field inside the maintainer's write: {:?} / {:?}",
        stored.properties,
        appended
    );
    assert!(
        appended.is_err(),
        "the invariant rule's refused action fails the write it ran in"
    );

    // With the rule gone the maintainer's own write still lands.
    engine
        .lifecycle()
        .write()
        .unwrap()
        .deactivate_play(&rule_play.id);
    svc.maintaining_system_fields(svc.create_node_with_parent(message(chat)))
        .await
        .expect("the maintainer's write lands");
}

/// A single-chain system field (no `extends`) through the trusted import and
/// the batch update, and a number's value rather than its spelling.
#[tokio::test]
async fn a_single_chain_system_field_is_held_on_the_trusted_import_and_bulk_update() {
    let (svc, _tmp) = test_service().await;
    assert_refused(
        "trusted import of a query",
        svc.bulk_create_hierarchy_trusted(vec![(
            "00000000-0000-4000-8000-0000000000b1".to_string(),
            "query".to_string(),
            "A query".to_string(),
            None,
            1.0,
            json!({ "generated_by": "user", "execution_count": 5 }),
        )])
        .await,
        "execution_count",
        "query",
    );

    let id = svc
        .maintaining_system_fields(svc.create_node(Node::new(
            "query".into(),
            "A query".into(),
            json!({ "generated_by": "user", "execution_count": 3 }),
        )))
        .await
        .expect("the maintainer creates a query with a count");
    let patch =
        |count: Value| NodeUpdate::new().with_properties(json!({ "execution_count": count }));
    // The stored value carried back is not a change, however it is spelled.
    svc.bulk_update(vec![(id.clone(), patch(json!(3)))])
        .await
        .expect("an unchanged system field on bulk_update");
    assert_refused(
        "bulk_update",
        svc.bulk_update(vec![(id.clone(), patch(json!(4)))]).await,
        "execution_count",
        "query",
    );
}

/// A number is compared by its value, not its spelling: a client that sends a
/// whole node back with `0` as `0.0` is not changing `exit_code`.
#[tokio::test]
async fn a_number_resent_as_a_float_is_not_a_change() {
    let (svc, _tmp) = test_service().await;
    let id = svc
        .maintaining_system_fields(
            svc.create_node(pty_chat(json!({ "agent": "claude-code", "exit_code": 0 }))),
        )
        .await
        .expect("the maintainer records an exit code");
    let node = svc.get_node(&id).await.unwrap().unwrap();
    svc.update_node(
        &id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "exit_code": 0.0 })),
    )
    .await
    .expect("0.0 is the stored 0");
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_refused(
        "a different number",
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "exit_code": 1.0 })),
        )
        .await,
        "exit_code",
        "ai-chat-pty",
    );
}

/// Integers beyond 2^53 compare exactly, not through a float.
#[tokio::test]
async fn a_big_integer_changed_by_one_is_a_change() {
    let (svc, _tmp) = test_service().await;
    let id = svc
        .maintaining_system_fields(svc.create_node(pty_chat(
            json!({ "agent": "claude-code", "exit_code": 9007199254740992i64 }),
        )))
        .await
        .expect("the maintainer records an exit code");
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_refused(
        "2^53 + 1",
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "exit_code": 9007199254740993i64 })),
        )
        .await,
        "exit_code",
        "ai-chat-pty",
    );
}

/// A tripwire, not the protection: the scope in `maintaining_system_fields`
/// is what keeps a user surface out (a surface that called it would only lift
/// the check for its own write, and nothing a user can author runs inside it).
/// This scans the product source of every surface for the name, so a new call
/// there is a review conversation rather than an accident.
#[test]
fn no_user_facing_surface_names_the_maintainer_path() {
    let packages = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    // Whole crates or directories, except the files named in `allowed`, which
    // are the maintaining code itself.
    let surfaces = [
        "cli/src",
        "desktop-app/app-lib/src",
        "desktop-app/src-tauri/src",
        "agent/src",
        "core/src/ops",
        "core/src/playbook",
        "daemon/src/router.rs",
        "daemon/src/lib.rs",
        "daemon/src/main.rs",
    ];
    let daemon_services = [
        "node_service.rs",
        "import_service.rs",
        "local_agent_service.rs",
        "agent_session_service.rs",
        "database_service.rs",
        "extensions.rs",
    ];
    let mut roots: Vec<std::path::PathBuf> = surfaces.iter().map(|s| packages.join(s)).collect();
    roots.extend(
        daemon_services
            .iter()
            .map(|s| packages.join("daemon/src/services").join(s)),
    );
    let mut scanned = 0;
    for root in roots {
        let mut files = vec![root];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else if path.extension().is_some_and(|e| e == "rs") {
                scanned += 1;
                let source = std::fs::read_to_string(&path).unwrap();
                // Test modules emulate the maintainer. A module is cut at its
                // `#[cfg(test)]` line to its closing brace at column zero, so
                // product code after one is still read.
                let mut product = String::new();
                let mut in_tests = false;
                for line in source.lines() {
                    if line.trim_start().starts_with("#[cfg(test)]") && !line.starts_with(' ') {
                        in_tests = true;
                        continue;
                    }
                    if in_tests {
                        if line == "}" {
                            in_tests = false;
                        }
                        continue;
                    }
                    product.push_str(line);
                    product.push('\n');
                }
                // The declaration path for an extension's `system` custom fields
                // (ADR-112 section 12) is held to the same line.
                for name in ["maintaining_system_fields", "declare_system_custom_fields"] {
                    assert!(!product.contains(name), "{} calls {name}", path.display());
                }
            }
        }
    }
    assert!(
        scanned > 40,
        "the scan found little surface source ({scanned})"
    );
}
