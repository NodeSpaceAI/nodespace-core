//! The AI-chat type family, end to end against a real store (ADR-088 §1–§2):
//! `ai-chat` is an abstract base that is never instantiated, `ai-chat-native`
//! and `ai-chat-pty` are core subtypes with their own fields and typed wire
//! structs, the base's rules reach both, and each type's bucket is closed.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{
    node_to_typed_value, AiChatNativeNode, AiChatProvider, AiChatPtyNode, AiChatSessionStatus,
    AiChatTurnStatus, CoreNodeType, Node, NodeUpdate,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NodeService, NodeServiceError,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const NATIVE: &str = "ai-chat-native";
const PTY: &str = "ai-chat-pty";

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(
        SqliteStore::new(db_path)
            .await
            .expect("SqliteStore init failed"),
    );
    let node_service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("NodeService init failed"),
    );
    (node_service, temp_dir)
}

async fn create(
    svc: &Arc<NodeService>,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> Result<String, NodeServiceError> {
    svc.create_node(Node::new(
        node_type.to_string(),
        content.to_string(),
        properties,
    ))
    .await
}

async fn native_chat(svc: &Arc<NodeService>) -> String {
    create(svc, NATIVE, "Untitled", json!({ "agent": "nodespace" }))
        .await
        .expect("a native chat is created")
}

async fn pty_chat(svc: &Arc<NodeService>) -> String {
    create(svc, PTY, "Untitled", json!({ "agent": "claude-code" }))
        .await
        .expect("a terminal chat is created")
}

async fn get(svc: &Arc<NodeService>, id: &str) -> Node {
    svc.get_node(id).await.unwrap().expect("the node exists")
}

fn message(result: Result<impl std::fmt::Debug, NodeServiceError>) -> String {
    result.expect_err("the write must be refused").to_string()
}

// ---------------------------------------------------------------------------
// The abstract base
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ai_chat_is_refused_on_every_create_path() {
    let (svc, _tmp) = test_service().await;
    let abstract_error = |e: String| assert!(e.contains("abstract type"), "{e}");
    let properties = json!({ "agent": "nodespace" });

    abstract_error(message(
        create(&svc, "ai-chat", "Untitled", properties.clone()).await,
    ));

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    abstract_error(message(
        svc.create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "ai-chat".to_string(),
            content: "Untitled".to_string(),
            parent_id: Some(page),
            position: InsertPositionOwned::End,
            properties: properties.clone(),
            lifecycle_status: None,
        })
        .await,
    ));

    abstract_error(message(
        svc.bulk_create(vec![Node::new(
            "ai-chat".to_string(),
            "Untitled".to_string(),
            properties.clone(),
        )])
        .await,
    ));

    abstract_error(message(
        svc.bulk_create_hierarchy(vec![(
            uuid::Uuid::new_v4().to_string(),
            "ai-chat".to_string(),
            "Untitled".to_string(),
            None,
            0.0,
            properties,
        )])
        .await,
    ));
}

#[tokio::test]
async fn a_chat_cannot_be_retyped_into_the_abstract_base() {
    let (svc, _tmp) = test_service().await;
    let id = native_chat(&svc).await;
    let version = get(&svc, &id).await.version;

    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_node_type("ai-chat".to_string()),
        )
        .await,
    );
    assert!(error.contains("abstract type"), "{error}");
    assert_eq!(get(&svc, &id).await.node_type, NATIVE);
}

#[tokio::test]
async fn the_registry_and_the_store_agree_on_the_family() {
    let (svc, _tmp) = test_service().await;

    assert!(svc.store().is_abstract_type("ai-chat").await.unwrap());
    for subtype in [NATIVE, PTY] {
        assert!(!svc.store().is_abstract_type(subtype).await.unwrap());
        assert_eq!(
            svc.store().type_chain(subtype).await.unwrap(),
            vec![subtype, "ai-chat"]
        );
        assert!(svc.type_is_a(subtype, CoreNodeType::AiChat).await.unwrap());
    }
    // The seeded `extends` edges are what the registry says they are, so the
    // ancestry table SQL rules read agrees with it.
    let parents = svc.store().get_extends_parent_map().await.unwrap();
    assert_eq!(parents.get(NATIVE).map(String::as_str), Some("ai-chat"));
    assert_eq!(parents.get(PTY).map(String::as_str), Some("ai-chat"));
}

// ---------------------------------------------------------------------------
// The subtypes: fields, buckets, defaults and typed structs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_native_chat_stores_each_field_in_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = create(
        &svc,
        NATIVE,
        "Untitled",
        json!({ "agent": "nodespace", "model": "gemma-4-e4b", "provider": "openai-compat" }),
    )
    .await
    .unwrap();

    let node = get(&svc, &id).await;
    // Base fields live in the base's bucket; the subtype's in its own, with
    // the schema defaults filled in.
    assert_eq!(
        node.properties["ai-chat"],
        json!({ "agent": "nodespace", "model": "gemma-4-e4b" })
    );
    assert_eq!(
        node.properties[NATIVE],
        json!({
            "provider": "openai-compat",
            "turn_status": "idle",
            "context_tokens": 0,
            "messages": []
        })
    );

    let chat = AiChatNativeNode::from_node(node).unwrap();
    assert_eq!(chat.base.agent, "nodespace");
    assert_eq!(chat.base.model.as_deref(), Some("gemma-4-e4b"));
    assert_eq!(chat.provider, AiChatProvider::OpenaiCompat);
    assert_eq!(chat.turn_status, AiChatTurnStatus::Idle);
    assert!(chat.messages.is_empty());
}

#[tokio::test]
async fn a_terminal_chat_stores_each_field_in_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = pty_chat(&svc).await;

    let node = get(&svc, &id).await;
    assert_eq!(
        node.properties["ai-chat"],
        json!({ "agent": "claude-code" })
    );
    assert_eq!(node.properties[PTY], json!({ "session_status": "active" }));

    let chat = AiChatPtyNode::from_node(node).unwrap();
    assert_eq!(chat.base.agent, "claude-code");
    assert_eq!(chat.base.model, None, "a harness name is not a model");
    assert_eq!(chat.session_status, AiChatSessionStatus::Active);
    assert_eq!(chat.session_id, None);
    assert_eq!(chat.exit_code, None);
}

/// Each subtype travels as its own typed struct: its chain's fields at the
/// top level, and `properties` holding extension fields only.
#[tokio::test]
async fn each_subtype_travels_as_its_own_typed_struct() {
    let (svc, _tmp) = test_service().await;
    let native = native_chat(&svc).await;
    let pty = pty_chat(&svc).await;

    let wire = |node: Node| node_to_typed_value(node).unwrap();

    let native = wire(get(&svc, &native).await);
    assert_eq!(native["nodeType"], NATIVE);
    assert_eq!(native["agent"], "nodespace");
    assert_eq!(native["provider"], "native");
    assert_eq!(native["turnStatus"], "idle");
    assert_eq!(native["messages"], json!([]));
    assert!(native.get("sessionStatus").is_none());
    assert_eq!(native["properties"], json!({}));

    let pty = wire(get(&svc, &pty).await);
    assert_eq!(pty["nodeType"], PTY);
    assert_eq!(pty["agent"], "claude-code");
    assert_eq!(pty["sessionStatus"], "active");
    assert!(pty.get("messages").is_none());
    assert!(pty.get("turnStatus").is_none());
    assert_eq!(pty["properties"], json!({}));
}

/// `agent` says who runs the conversation, and every chat states it.
#[tokio::test]
async fn a_chat_without_an_agent_is_refused() {
    let (svc, _tmp) = test_service().await;
    for subtype in [NATIVE, PTY] {
        let error = message(create(&svc, subtype, "Untitled", json!({})).await);
        assert!(error.contains("agent"), "{subtype}: {error}");
    }
}

/// A subtype's bucket is closed, like the base's: it takes its own declared
/// fields, not the other subtype's, not the retired names, and not an
/// arbitrary key.
#[tokio::test]
async fn a_chat_takes_only_its_own_declared_fields() {
    let (svc, _tmp) = test_service().await;

    for (subtype, agent, foreign) in [
        // A native chat has no session state; a terminal chat no messages.
        (NATIVE, "nodespace", json!({ "session_status": "active" })),
        (NATIVE, "nodespace", json!({ "transcript": "$ ls" })),
        (PTY, "codex", json!({ "messages": [] })),
        (PTY, "codex", json!({ "provider": "native" })),
        (PTY, "codex", json!({ "turn_status": "idle" })),
        // The retired names.
        (PTY, "codex", json!({ "capture:session_id": "s-1" })),
        (
            PTY,
            "codex",
            json!({ "started_at": "2026-01-01T00:00:00Z" }),
        ),
        (PTY, "codex", json!({ "ended_at": "2026-01-01T00:00:00Z" })),
        (PTY, "codex", json!({ "agent_type": "codex" })),
        (NATIVE, "nodespace", json!({ "created_nodes": [] })),
        (NATIVE, "nodespace", json!({ "anything": 1 })),
    ] {
        let mut properties = foreign.clone();
        properties["agent"] = json!(agent);
        let error = message(create(&svc, subtype, "Untitled", properties).await);
        let key = foreign.as_object().unwrap().keys().next().unwrap().clone();
        assert!(error.contains(&key), "{subtype} {foreign}: {error}");
    }

    // A namespaced extension field is still welcome.
    create(
        &svc,
        NATIVE,
        "Untitled",
        json!({ "agent": "nodespace", "custom:pinned": true }),
    )
    .await
    .expect("an extension field is accepted");
}

/// The closed vocabularies: there is no `pty` provider (a terminal chat is a
/// type), and a finished session is `ended`, never governance's `archived`.
#[tokio::test]
async fn the_closed_enums_are_enforced_on_write() {
    let (svc, _tmp) = test_service().await;

    let error = message(
        create(
            &svc,
            NATIVE,
            "Untitled",
            json!({ "agent": "nodespace", "provider": "pty" }),
        )
        .await,
    );
    assert!(error.contains("pty"), "{error}");

    let id = pty_chat(&svc).await;
    let version = get(&svc, &id).await.version;
    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_properties(json!({ "session_status": "archived" })),
        )
        .await,
    );
    assert!(error.contains("archived"), "{error}");

    svc.update_node(
        &id,
        version,
        NodeUpdate::new().with_properties(json!({ "session_status": "ended" })),
    )
    .await
    .expect("ended is the session's finished state");
    assert_eq!(
        get(&svc, &id).await.properties[PTY]["session_status"],
        "ended"
    );
}

/// A flat patch names base and subtype fields alike; each lands in the
/// bucket of the schema that declares it.
#[tokio::test]
async fn a_flat_update_lands_each_field_in_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = pty_chat(&svc).await;
    let version = get(&svc, &id).await.version;

    svc.update_node(
        &id,
        version,
        NodeUpdate::new().with_properties(json!({
            "summary": "Fixed the build",
            "session_id": "s-1",
            "exit_code": 0,
            "session_status": "ended"
        })),
    )
    .await
    .unwrap();

    let node = get(&svc, &id).await;
    assert_eq!(
        node.properties["ai-chat"],
        json!({ "agent": "claude-code", "summary": "Fixed the build" })
    );
    assert_eq!(
        node.properties[PTY],
        json!({ "session_status": "ended", "session_id": "s-1", "exit_code": 0 })
    );
}

// ---------------------------------------------------------------------------
// The base's rules reach both subtypes
// ---------------------------------------------------------------------------

/// A query for the abstract base returns both subtypes' nodes, each under its
/// own type.
#[tokio::test]
async fn a_query_for_ai_chat_returns_both_subtypes() {
    let (svc, _tmp) = test_service().await;
    let native = native_chat(&svc).await;
    let pty = pty_chat(&svc).await;
    create(&svc, "text", "Not a chat", json!({})).await.unwrap();

    let found = svc.query_nodes_by_type("ai-chat", false).await.unwrap();
    let mut types: Vec<(&str, &str)> = found
        .iter()
        .map(|n| (n.id.as_str(), n.node_type.as_str()))
        .collect();
    types.sort_unstable();
    let mut expected = vec![(native.as_str(), NATIVE), (pty.as_str(), PTY)];
    expected.sort_unstable();
    assert_eq!(types, expected);

    // A query for one subtype returns only that subtype.
    let only_pty = svc.query_nodes_by_type(PTY, false).await.unwrap();
    assert_eq!(
        only_pty.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        vec![pty.as_str()]
    );
}

/// No node may reference a chat (ADR-061 §8), whichever subtype it is.
#[tokio::test]
async fn neither_subtype_can_be_referenced() {
    let (svc, _tmp) = test_service().await;
    let page = create(&svc, "text", "A page", json!({})).await.unwrap();

    for chat in [native_chat(&svc).await, pty_chat(&svc).await] {
        let error = message(svc.create_mention(&page, &chat).await);
        assert!(error.contains("ai-chat"), "{error}");
    }
}

/// A chat comes into being only by being created as one. Between two chat
/// types a retype is allowed: the node gains no inbound references.
#[tokio::test]
async fn a_chat_can_be_retyped_only_from_another_chat_type() {
    let (svc, _tmp) = test_service().await;

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let version = get(&svc, &page).await.version;
    for subtype in [NATIVE, PTY] {
        let error = message(
            svc.update_node(
                &page,
                version,
                NodeUpdate::new()
                    .with_node_type(subtype.to_string())
                    .with_properties(json!({ "agent": "nodespace" })),
            )
            .await,
        );
        assert!(
            error.contains("cannot be converted to an ai-chat node"),
            "{subtype}: {error}"
        );
    }

    // The user picks a terminal harness for a chat that began as a native
    // one: the base bucket stays live, the harness becomes the agent, and
    // the model is cleared.
    let id = create(
        &svc,
        NATIVE,
        "Untitled",
        json!({ "agent": "nodespace", "model": "gemma-4-e4b" }),
    )
    .await
    .unwrap();
    let version = get(&svc, &id).await.version;
    svc.update_node(
        &id,
        version,
        NodeUpdate::new()
            .with_node_type(PTY.to_string())
            .with_properties(json!({ "agent": "codex", "model": null })),
    )
    .await
    .expect("a native chat becomes a terminal chat");

    let chat = AiChatPtyNode::from_node(get(&svc, &id).await).unwrap();
    assert_eq!(chat.base.agent, "codex");
    assert_eq!(chat.base.model, None);
    assert_eq!(chat.session_status, AiChatSessionStatus::Active);
}

/// A chat may hold children (ADR-088 §1), and both subtypes inherit that.
#[tokio::test]
async fn a_chat_may_hold_children() {
    let (svc, _tmp) = test_service().await;
    for chat in [native_chat(&svc).await, pty_chat(&svc).await] {
        let child = svc
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "A note under the chat".to_string(),
                parent_id: Some(chat.clone()),
                position: InsertPositionOwned::End,
                properties: json!({}),
                lifecycle_status: None,
            })
            .await
            .expect("a chat accepts a child");
        let parent = svc.get_parent(&child).await.unwrap();
        assert_eq!(parent.map(|p| p.id), Some(chat));
    }
}

/// The title rule is the base's, and holds for both subtypes on create and
/// on update.
#[tokio::test]
async fn both_subtypes_require_a_title() {
    let (svc, _tmp) = test_service().await;
    for (subtype, agent) in [(NATIVE, "nodespace"), (PTY, "codex")] {
        let error = message(create(&svc, subtype, "  ", json!({ "agent": agent })).await);
        assert!(error.contains("content"), "{subtype}: {error}");

        let id = create(&svc, subtype, "A title", json!({ "agent": agent }))
            .await
            .unwrap();
        let version = get(&svc, &id).await.version;
        let error = message(
            svc.update_node(&id, version, NodeUpdate::new().with_content(String::new()))
                .await,
        );
        assert!(error.contains("content"), "{subtype}: {error}");
    }
}
