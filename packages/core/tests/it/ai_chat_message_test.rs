//! Chat messages as nodes, end to end against a real store (ADR-088 §3): an
//! `ai-chat-message` is a child of its native chat with its text as content,
//! what it wrote, looked up or asked to delete is on edges from it, its
//! mentions come from the standard pipeline, and it takes part in no surface
//! of its own.

use nodespace_core::db::{SqliteStore, TreeInvariantRule};
use nodespace_core::models::{
    node_to_typed_value, AiChatMessageNode, AiChatMessageRole, AiChatPendingDeleteEdge,
    AiChatResolvedEdge, AiChatTurnOutcome, AiChatWrite, AiChatWroteEdge, Node, NodeFilter,
    NodeQuery, NodeUpdate, AI_CHAT_PENDING_DELETE, AI_CHAT_RESOLVED, AI_CHAT_WROTE,
};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService, NodeServiceError,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const MESSAGE: &str = "ai-chat-message";

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
) -> String {
    svc.create_node(Node::new(
        node_type.to_string(),
        content.to_string(),
        properties,
    ))
    .await
    .unwrap_or_else(|e| panic!("creating {node_type} '{content}' failed: {e}"))
}

async fn native_chat(svc: &Arc<NodeService>) -> String {
    create(
        svc,
        "ai-chat-native",
        "Untitled",
        json!({ "agent": "nodespace" }),
    )
    .await
}

async fn try_create_under(
    svc: &Arc<NodeService>,
    parent_id: Option<&str>,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> Result<String, NodeServiceError> {
    svc.create_node_with_parent(CreateNodeParams {
        id: None,
        node_type: node_type.to_string(),
        content: content.to_string(),
        parent_id: parent_id.map(str::to_string),
        position: InsertPositionOwned::End,
        properties,
        lifecycle_status: None,
    })
    .await
}

async fn message(
    svc: &Arc<NodeService>,
    chat_id: &str,
    content: &str,
    properties: serde_json::Value,
) -> String {
    try_create_under(svc, Some(chat_id), MESSAGE, content, properties)
        .await
        .unwrap_or_else(|e| panic!("creating message '{content}' failed: {e}"))
}

async fn get(svc: &Arc<NodeService>, id: &str) -> Node {
    svc.get_node(id).await.unwrap().expect("the node exists")
}

/// The rule a refused write names.
fn refused<T: std::fmt::Debug>(result: Result<T, NodeServiceError>) -> TreeInvariantRule {
    match result.expect_err("the write must be refused") {
        NodeServiceError::TreeInvariantViolation(v) => v.rule,
        other => panic!("expected a tree invariant violation, got {other:?}"),
    }
}

fn has(nodes: &[Node], id: &str) -> bool {
    nodes.iter().any(|n| n.id == id)
}

// ---------------------------------------------------------------------------
// The type and its fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_message_keeps_its_text_as_content_and_its_fields_in_its_bucket() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(
        &svc,
        &chat,
        "Which one did you mean?",
        json!({
            "role": "assistant",
            "timestamp": "2026-10-02T10:00:00Z",
            "reasoning": "Two tasks match.",
            "outcome": "clarified",
            "options": ["Login bug", "Logout bug"]
        }),
    )
    .await;

    let node = get(&svc, &id).await;
    assert_eq!(node.content, "Which one did you mean?");
    assert_eq!(
        node.properties[MESSAGE],
        json!({
            "role": "assistant",
            "timestamp": "2026-10-02T10:00:00Z",
            "reasoning": "Two tasks match.",
            "outcome": "clarified",
            "options": ["Login bug", "Logout bug"]
        })
    );

    let typed = AiChatMessageNode::from_node(node.clone()).unwrap();
    assert_eq!(typed.role, AiChatMessageRole::Assistant);
    assert_eq!(typed.outcome, Some(AiChatTurnOutcome::Clarified));
    assert_eq!(typed.options, ["Login bug", "Logout bug"]);

    let wire = node_to_typed_value(node).unwrap();
    assert_eq!(wire["nodeType"], MESSAGE);
    assert_eq!(wire["role"], "assistant");
    assert_eq!(wire["content"], "Which one did you mean?");
    assert_eq!(wire["properties"], json!({}));
}

/// The schema's default is the only default: a message written without a
/// role is the user's.
#[tokio::test]
async fn a_message_written_without_a_role_is_a_user_message() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(&svc, &chat, "hello", json!({})).await;

    let node = get(&svc, &id).await;
    assert_eq!(node.properties[MESSAGE], json!({ "role": "user" }));
}

/// The bucket is closed and the vocabularies are the wire enums': the keys
/// of the old nested shape are refused, and so is a role outside the three.
#[tokio::test]
async fn a_message_takes_only_its_declared_fields_and_values() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;

    for (properties, named) in [
        (json!({ "question": "Which?" }), "question"),
        (json!({ "completedWrites": [] }), "completedWrites"),
        (json!({ "completed_writes": [] }), "completed_writes"),
        (json!({ "pending_deletions": [] }), "pending_deletions"),
        (json!({ "role": "tool_call" }), "tool_call"),
        (json!({ "role": "assistant", "outcome": "done" }), "done"),
        (json!({ "role": "user", "outcome": "replied" }), "assistant"),
    ] {
        let error = try_create_under(&svc, Some(&chat), MESSAGE, "hi", properties.clone())
            .await
            .expect_err("the write must be refused")
            .to_string();
        assert!(error.contains(named), "{properties}: {error}");
    }
}

/// A native chat has no list of messages and no list of what it created:
/// both are the graph's to answer.
#[tokio::test]
async fn a_native_chat_stores_neither_messages_nor_created_nodes() {
    let (svc, _tmp) = test_service().await;
    for gone in ["messages", "created_nodes"] {
        let error = svc
            .create_node(Node::new(
                "ai-chat-native".to_string(),
                "Untitled".to_string(),
                json!({ "agent": "nodespace", gone: [] }),
            ))
            .await
            .expect_err("the field is gone")
            .to_string();
        assert!(error.contains(gone), "{gone}: {error}");
    }
}

// ---------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_message_must_sit_under_a_native_chat() {
    let (svc, _tmp) = test_service().await;
    let page = create(&svc, "text", "A page", json!({})).await;
    let terminal = create(
        &svc,
        "ai-chat-pty",
        "Untitled",
        json!({ "agent": "claude-code" }),
    )
    .await;

    assert_eq!(
        refused(try_create_under(&svc, None, MESSAGE, "orphan", json!({})).await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(
        refused(
            svc.create_node(Node::new(
                MESSAGE.to_string(),
                "orphan".to_string(),
                json!({})
            ))
            .await
        ),
        TreeInvariantRule::ParentRequired
    );
    for parent in [&page, &terminal] {
        assert_eq!(
            refused(try_create_under(&svc, Some(parent), MESSAGE, "misplaced", json!({})).await),
            TreeInvariantRule::ParentRequired,
            "a message under {parent}"
        );
    }
}

#[tokio::test]
async fn a_message_cannot_leave_its_chat_or_take_children() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let other_chat = native_chat(&svc).await;
    let page = create(&svc, "text", "A page", json!({})).await;
    let id = message(&svc, &chat, "hello", json!({})).await;

    let version = get(&svc, &id).await.version;
    assert_eq!(
        refused(svc.move_node(&id, version, None, InsertPosition::End).await),
        TreeInvariantRule::ParentRequired,
        "to the root"
    );
    assert_eq!(
        refused(
            svc.move_node(&id, version, Some(&page), InsertPosition::End)
                .await
        ),
        TreeInvariantRule::ParentRequired,
        "under a page"
    );
    assert_eq!(svc.get_parent(&id).await.unwrap().unwrap().id, chat);

    assert_eq!(
        refused(try_create_under(&svc, Some(&id), "text", "a reply", json!({})).await),
        TreeInvariantRule::ChildrenNone
    );

    // The rule names a type, not one chat: another native chat qualifies.
    svc.move_node(&id, version, Some(&other_chat), InsertPosition::End)
        .await
        .expect("a native chat is a legal parent");
}

/// Messages are their chat's children in conversation order, beside
/// whatever else the chat holds.
#[tokio::test]
async fn a_chats_messages_are_its_children_in_order() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let first = message(&svc, &chat, "one", json!({})).await;
    let note = try_create_under(&svc, Some(&chat), "text", "a note", json!({}))
        .await
        .unwrap();
    let second = message(&svc, &chat, "two", json!({ "role": "assistant" })).await;

    let children: Vec<String> = svc
        .get_children(&chat)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(children, [first, note, second]);
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

fn write(seq: u32, tool: &str, args: &str) -> AiChatWrite {
    AiChatWrite {
        seq,
        tool: tool.to_string(),
        summary: Some("Buy milk".to_string()),
        canonical_args: args.to_string(),
        replaced: Vec::new(),
    }
}

/// What a message wrote, looked up or asked to delete is three declared
/// relationships with their edge fields, read back for the whole chat in one
/// call. "What did this chat create?" is that read over `wrote`.
#[tokio::test]
async fn a_messages_turn_records_are_edges_read_back_for_the_whole_chat() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let task = create(&svc, "task", "Buy milk", json!({})).await;
    let person = create(&svc, "person", "", json!({ "first_name": "Ada" })).await;
    let asked = message(&svc, &chat, "add a task", json!({})).await;
    let reply = message(&svc, &chat, "Added.", json!({ "role": "assistant" })).await;

    // One edge per node holds every write the turn made to it, in order.
    let wrote = AiChatWroteEdge {
        writes: vec![
            write(0, "create_node", r#"{"content":"Buy milk"}"#),
            write(1, "create_relationship", r#"{"from":"a","to":"b"}"#),
        ],
    };
    svc.create_relationship(
        &reply,
        AI_CHAT_WROTE,
        &task,
        serde_json::to_value(&wrote).unwrap(),
    )
    .await
    .expect("a wrote edge is created");
    svc.create_relationship(
        &reply,
        AI_CHAT_RESOLVED,
        &person,
        json!(AiChatResolvedEdge {
            tool: "search_nodes".to_string()
        }),
    )
    .await
    .expect("a resolved edge is created");
    svc.create_relationship(
        &reply,
        AI_CHAT_PENDING_DELETE,
        &task,
        json!(AiChatPendingDeleteEdge {
            version: 1,
            descendant_count: 0,
        }),
    )
    .await
    .expect("a pending_delete edge is created");

    let edges = svc
        .get_child_edges(
            &chat,
            &[AI_CHAT_WROTE, AI_CHAT_RESOLVED, AI_CHAT_PENDING_DELETE],
        )
        .await
        .unwrap();
    let summary: Vec<(&str, &str, &str)> = edges
        .iter()
        .map(|(source, name, target, _)| (source.as_str(), name.as_str(), target.id.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (reply.as_str(), "wrote", task.as_str()),
            (reply.as_str(), "resolved", person.as_str()),
            (reply.as_str(), "pending_delete", task.as_str()),
        ]
    );
    assert_eq!(
        serde_json::from_value::<AiChatWroteEdge>(edges[0].3.clone()).unwrap(),
        wrote
    );
    assert_eq!(
        serde_json::from_value::<AiChatPendingDeleteEdge>(edges[2].3.clone()).unwrap(),
        AiChatPendingDeleteEdge {
            version: 1,
            descendant_count: 0,
        }
    );

    // What the chat created: its messages' `wrote` edges. The user's message
    // wrote nothing.
    let created: Vec<String> = svc
        .get_child_edges(&chat, &[AI_CHAT_WROTE])
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, target, _)| target.id)
        .collect();
    assert_eq!(created, std::slice::from_ref(&task));
    assert!(!edges.iter().any(|(source, ..)| source == &asked));

    // From the node's end: which message wrote it.
    let writers = svc
        .get_related_nodes(&task, AI_CHAT_WROTE, "in")
        .await
        .unwrap();
    assert!(has(&writers, &reply));

    // Answering a held delete removes its edge.
    svc.delete_relationship(&reply, AI_CHAT_PENDING_DELETE, &task)
        .await
        .expect("the edge is removed");
    assert!(svc
        .get_child_edges(&chat, &[AI_CHAT_PENDING_DELETE])
        .await
        .unwrap()
        .is_empty());

    // An edge to a deleted node goes with the node.
    let version = get(&svc, &task).await.version;
    svc.delete_node(&task, version).await.unwrap();
    assert!(svc
        .get_child_edges(&chat, &[AI_CHAT_WROTE])
        .await
        .unwrap()
        .is_empty());
}

/// A turn that creates a type records it like any other write: the edge
/// points at the schema node.
#[tokio::test]
async fn a_message_records_a_schema_it_created() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    handle_create_schema(&svc, json!({ "name": "Invoice", "fields": [] }))
        .await
        .expect("create the schema");
    let reply = message(&svc, &chat, "Created.", json!({ "role": "assistant" })).await;

    svc.create_relationship(
        &reply,
        AI_CHAT_WROTE,
        "invoice",
        json!(AiChatWroteEdge {
            writes: vec![write(0, "create_schema", r#"{"name":"Invoice"}"#)],
        }),
    )
    .await
    .expect("a wrote edge to a schema node is created");

    let created: Vec<String> = svc
        .get_child_edges(&chat, &[AI_CHAT_WROTE])
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, target, _)| target.id)
        .collect();
    assert_eq!(created, ["invoice"]);
    // The edge is the message's, not a declaration of the schema's.
    assert!(svc
        .store()
        .get_schema_declarations("invoice")
        .await
        .unwrap()
        .is_empty());
}

/// A message's own edges may point at a chat or at another message: they
/// are the chat's record of what a turn looked up or asked to delete, not a
/// reference to the chat from outside. A mention still creates no edge.
#[tokio::test]
async fn a_messages_own_edges_may_point_at_a_chat_or_a_message() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let other_chat = native_chat(&svc).await;
    let asked = message(&svc, &chat, "delete that chat", json!({})).await;
    let reply = message(
        &svc,
        &chat,
        &format!("Delete [it](nodespace://{other_chat})?"),
        json!({ "role": "assistant" }),
    )
    .await;

    svc.create_relationship(
        &reply,
        AI_CHAT_PENDING_DELETE,
        &other_chat,
        json!(AiChatPendingDeleteEdge {
            version: 1,
            descendant_count: 0,
        }),
    )
    .await
    .expect("a held delete of a chat is recorded");
    svc.create_relationship(
        &reply,
        AI_CHAT_RESOLVED,
        &asked,
        json!(AiChatResolvedEdge {
            tool: "get_node".to_string()
        }),
    )
    .await
    .expect("a looked-up message is recorded");

    let targets: Vec<String> = svc
        .get_child_edges(&chat, &[AI_CHAT_PENDING_DELETE, AI_CHAT_RESOLVED])
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, target, _)| target.id)
        .collect();
    assert_eq!(targets, [other_chat.clone(), asked]);

    // The link in the reply's text is a mention, and a chat takes none,
    // whichever path asks for it.
    assert!(svc.get_mentions(&reply).await.unwrap().is_empty());
    let error = svc.create_mention(&reply, &other_chat).await.unwrap_err();
    assert!(error.to_string().contains("ai-chat"), "{error}");
}

/// A message and its edges are created together or not at all, and an edge
/// to a node that is already gone is left out without costing the rest.
#[tokio::test]
async fn a_message_is_created_with_its_edges_in_one_write() {
    use nodespace_core::services::NewRelationship;

    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let task = create(&svc, "task", "Buy milk", json!({})).await;
    let params = |content: &str| CreateNodeParams {
        id: None,
        node_type: MESSAGE.to_string(),
        content: content.to_string(),
        parent_id: Some(chat.clone()),
        position: InsertPositionOwned::End,
        properties: json!({ "role": "assistant" }),
        lifecycle_status: None,
    };
    let edge = |name: &str, target: &str, data: serde_json::Value| NewRelationship {
        name: name.to_string(),
        target_id: target.to_string(),
        edge_data: data,
    };
    let wrote = || {
        json!(AiChatWroteEdge {
            writes: vec![write(0, "create_node", "{}")]
        })
    };

    let (id, skipped) = svc
        .create_node_with_relationships(
            params("Added."),
            vec![
                edge(AI_CHAT_WROTE, "no-such-node", wrote()),
                edge(AI_CHAT_WROTE, &task, wrote()),
            ],
        )
        .await
        .expect("the message and its edge are created");
    assert_eq!(skipped, [0], "the edge to the missing node is left out");
    let edges = svc.get_child_edges(&chat, &[AI_CHAT_WROTE]).await.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].0.as_str(), edges[0].2.id.as_str()),
        (id.as_str(), task.as_str())
    );

    // An edge that cannot be written fails the whole create: no message is
    // left behind without it.
    let before = svc.get_children(&chat).await.unwrap().len();
    svc.create_node_with_relationships(
        params("Never stored."),
        vec![
            edge(AI_CHAT_WROTE, &task, wrote()),
            edge("not_a_declared_relationship", &task, json!({})),
        ],
    )
    .await
    .expect_err("an undeclared relationship fails the create");
    assert_eq!(svc.get_children(&chat).await.unwrap().len(), before);
    assert_eq!(
        svc.get_child_edges(&chat, &[AI_CHAT_WROTE])
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Only a message declares these relationships: another type cannot be the
/// source of one.
#[tokio::test]
async fn only_a_message_is_the_source_of_a_turn_record() {
    let (svc, _tmp) = test_service().await;
    let page = create(&svc, "text", "A page", json!({})).await;
    let task = create(&svc, "task", "Buy milk", json!({})).await;

    assert!(svc
        .create_relationship(&page, AI_CHAT_WROTE, &task, json!({ "writes": [] }))
        .await
        .is_err());
}

// ---------------------------------------------------------------------------
// Mentions and references
// ---------------------------------------------------------------------------

/// A link in a message is a `mentions` edge from the message, made when the
/// message is written, and the node's backlinks name that message.
#[tokio::test]
async fn a_link_in_a_message_is_a_mention_from_the_message() {
    let (svc, _tmp) = test_service().await;
    let page = create(&svc, "text", "Harbour plan", json!({})).await;
    let root = create(&svc, "text", "Projects", json!({})).await;
    let chat = try_create_under(
        &svc,
        Some(&root),
        "ai-chat-native",
        "Untitled",
        json!({ "agent": "nodespace" }),
    )
    .await
    .unwrap();
    let id = message(
        &svc,
        &chat,
        &format!("I updated [Harbour plan](nodespace://{page})."),
        json!({ "role": "assistant" }),
    )
    .await;

    assert_eq!(
        svc.get_mentions(&id).await.unwrap(),
        std::slice::from_ref(&page)
    );
    assert_eq!(
        svc.get_mentioned_by(&page).await.unwrap(),
        std::slice::from_ref(&id)
    );

    // The backlink is the message itself, not the page its chat sits under:
    // a reader opens the chat at that message.
    let containers = svc.get_mentioning_containers(&page).await.unwrap();
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [id.as_str()]);
    assert_eq!(containers[0].node_type, MESSAGE);
}

/// No node may reference a message: a link to one creates no edge, no
/// relationship may target one, and no other node becomes one.
#[tokio::test]
async fn nothing_references_a_message() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(&svc, &chat, "hello", json!({})).await;
    let page = create(&svc, "text", "A page", json!({})).await;
    let task = create(&svc, "task", "Buy milk", json!({})).await;

    // A content link.
    let version = get(&svc, &page).await.version;
    svc.update_node(
        &page,
        version,
        NodeUpdate::new().with_content(format!("See [[{id}]] and [[{task}]]")),
    )
    .await
    .unwrap();
    assert_eq!(
        svc.get_mentions(&page).await.unwrap(),
        std::slice::from_ref(&task)
    );

    // The mention and relationship APIs.
    let error = svc.create_mention(&page, &id).await.unwrap_err();
    assert!(error.to_string().contains(MESSAGE), "{error}");
    let error = svc
        .create_relationship(&task, "relates_to", &id, json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains(MESSAGE), "{error}");

    // A declared relationship.
    let error = handle_create_schema(
        &svc,
        json!({
            "name": "Citation",
            "fields": [],
            "relationships": [{
                "name": "cites",
                "targetType": MESSAGE,
                "direction": "out",
                "cardinality": "many",
                "reverseName": "cited_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("a relationship may not target a message");
    assert!(format!("{error:?}").contains(MESSAGE), "{error:?}");

    // A retype. A node under a chat satisfies the message's parent rule, so
    // only the reference rule stands in the way.
    let note = try_create_under(&svc, Some(&chat), "text", "a note", json!({}))
        .await
        .unwrap();
    let version = get(&svc, &note).await.version;
    let error = svc
        .update_node(
            &note,
            version,
            NodeUpdate::new().with_node_type(MESSAGE.to_string()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot be converted"), "{error}");
}

// ---------------------------------------------------------------------------
// Participation
// ---------------------------------------------------------------------------

/// A message is in no default query, count, search or picker result, and is
/// still read by id and as its chat's child.
#[tokio::test]
async fn a_message_takes_part_in_no_query_count_search_or_picker() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let page = create(&svc, "text", "Saltmarsh survey notes", json!({})).await;
    let id = message(&svc, &chat, "Saltmarsh survey question", json!({})).await;

    // By type, with and without the archived opt-in: the rule is the type's.
    for include_archived in [false, true] {
        let by_type = svc
            .query_nodes(
                NodeFilter::new()
                    .with_node_type(MESSAGE.to_string())
                    .with_include_archived(include_archived),
            )
            .await
            .unwrap();
        assert!(!has(&by_type, &id), "include_archived={include_archived}");
    }
    assert!(!has(
        &svc.query_nodes_by_type(MESSAGE, false).await.unwrap(),
        &id
    ));

    // Counts agree with the queries.
    let count = svc
        .count_nodes(NodeQuery {
            node_type: Some(MESSAGE.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(count, 0);

    // Keyword search, by title and by content.
    for by_title in [true, false] {
        let mut query = NodeQuery::default();
        if by_title {
            query.title_contains = Some("Saltmarsh survey".to_string());
        } else {
            query.content_contains = Some("Saltmarsh survey".to_string());
        }
        let found = svc.query_nodes_simple(query).await.unwrap();
        assert!(has(&found, &page), "by_title={by_title}");
        assert!(!has(&found, &id), "by_title={by_title}");
    }

    // The `@` picker.
    let offered = svc.mention_autocomplete("Saltmarsh", None).await.unwrap();
    assert!(has(&offered, &page));
    assert!(!has(&offered, &id));

    // Roots: a message is never one, and its chat still is.
    let roots = svc.get_roots(None, None, false).await.unwrap();
    assert!(has(&roots, &chat));
    assert!(!has(&roots, &id));

    // It is still read by id, and as its chat's child.
    assert_eq!(get(&svc, &id).await.content, "Saltmarsh survey question");
    assert!(has(&svc.get_children(&chat).await.unwrap(), &id));
}

/// A message is not an embedding root and adds nothing to another root's
/// text: conversation fragments stay out of knowledge search.
#[tokio::test]
async fn a_message_is_not_embedded() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(&svc, &chat, "How do I rotate the harbour keys?", json!({})).await;

    let registry = nodespace_core::behaviors::NodeBehaviorRegistry::new();
    let node = get(&svc, &id).await;
    let behavior = registry.get(MESSAGE).expect("registered");
    assert!(behavior.get_embeddable_content(&node).is_none());
    assert!(behavior.get_parent_contribution(&node).is_none());
    assert!(
        !nodespace_core::models::CoreNodeType::AiChatMessage
            .participation()
            .embedded
    );
}

/// A reader that holds only a message's id finds its chat: the message's
/// parent is one related-nodes read away, which is how a client opens the
/// chat a backlink or a message link leads to.
#[tokio::test]
async fn a_messages_chat_is_found_from_the_message() {
    use nodespace_core::ops::rel_ops::{get_related_nodes, GetRelatedInput};

    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(&svc, &chat, "hello", json!({})).await;

    let parent = get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: id,
            relationship_name: "has_child".to_string(),
            direction: "in".to_string(),
        },
    )
    .await
    .expect("the parent read succeeds");
    let parents = serde_json::to_value(&parent.related_nodes).unwrap();
    assert_eq!(parents.as_array().unwrap().len(), 1, "{parents}");
    assert_eq!(parents[0]["id"], json!(chat));
    assert_eq!(parents[0]["nodeType"], "ai-chat-native");

    // A root has none.
    let none = get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: chat,
            relationship_name: "has_child".to_string(),
            direction: "in".to_string(),
        },
    )
    .await
    .expect("the parent read succeeds");
    assert_eq!(none.count, 0);
}

/// Deleting a chat removes its messages with it.
#[tokio::test]
async fn deleting_a_chat_deletes_its_messages() {
    let (svc, _tmp) = test_service().await;
    let chat = native_chat(&svc).await;
    let id = message(&svc, &chat, "hello", json!({})).await;

    let version = get(&svc, &chat).await.version;
    svc.delete_node(&chat, version).await.unwrap();
    assert!(svc.get_node(&id).await.unwrap().is_none());
}
