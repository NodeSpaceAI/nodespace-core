//! `search_nodes` never returns a conversation.
//!
//! A chat is titled from the user's own words, so a keyword search for the
//! question just asked matches the chat it was asked in — and the model then
//! reports its own conversation back as what it found. Observed as a turn
//! whose first search result was the ai-chat node the turn was running in.
//!
//! Drives the real production `GraphToolExecutor::execute("search_nodes", ...)`
//! surface against a real `SqliteStore`, on both of the search core's paths:
//! the title listing (`node_ops::query_nodes`) and the typed-property one
//! (`QueryService`), which a `sorting` argument selects.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// One note and `chats` conversations, every title containing "shared data
/// layer". Returns the note's id.
async fn seed_note_among_chats(ns: &Arc<NodeService>, chats: usize) -> String {
    for i in 0..chats {
        ns.create_node(Node::new(
            "ai-chat-native".to_string(),
            format!("what does our shared data layer do ({i})"),
            json!({ "agent": "nodespace" }),
        ))
        .await
        .unwrap();
    }
    ns.create_node(Node::new(
        "text".to_string(),
        "Shared data layer design".to_string(),
        json!({}),
    ))
    .await
    .unwrap()
}

fn types_of(result: &Value) -> Vec<String> {
    result["nodes"]
        .as_array()
        .expect("nodes is an array")
        .iter()
        .map(|n| n["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The seed must actually put a conversation where an unfiltered query finds
/// it, or the exclusion tests below pass for the boring reason that there was
/// nothing to exclude.
#[tokio::test]
async fn the_store_does_hold_a_conversation_matching_the_keyword() {
    let (_executor, ns, _tmp) = make_executor().await;
    seed_note_among_chats(&ns, 1).await;

    let found = nodespace_core::ops::node_ops::query_nodes(
        &ns,
        nodespace_core::ops::node_ops::QueryNodesInput {
            node_type: None,
            limit: Some(50),
            offset: None,
            collection_id: None,
            collection: None,
            filters: Some(vec![nodespace_core::ops::node_ops::QueryFilterItem {
                field: "title".to_string(),
                operator: "contains".to_string(),
                value: json!("shared data layer"),
            }]),
        },
    )
    .await
    .unwrap();
    let types: Vec<&str> = found
        .nodes
        .iter()
        .filter_map(|n| n.get("nodeType").and_then(|t| t.as_str()))
        .collect();
    assert!(
        types.contains(&"ai-chat-native"),
        "an unfiltered title query must match the seeded conversation: {types:?}"
    );
}

#[tokio::test]
async fn a_keyword_search_returns_the_note_and_not_the_chat_titled_after_it() {
    let (executor, ns, _tmp) = make_executor().await;
    let note_id = seed_note_among_chats(&ns, 1).await;

    let result = executor
        .execute("search_nodes", json!({ "query": "shared data layer" }))
        .await
        .expect("search_nodes must succeed");

    assert!(!result.is_error);
    assert_eq!(types_of(&result.result), vec!["text"]);
    assert_eq!(result.result["count"], 1);
    assert_eq!(
        result.result["nodes"][0]["id"],
        format!("nodespace://{note_id}")
    );
}

/// `sorting` routes the search through `QueryService` rather than the title
/// listing. The exclusion sits above both, so it holds there too.
#[tokio::test]
async fn a_sorted_keyword_search_excludes_conversations_too() {
    let (executor, ns, _tmp) = make_executor().await;
    seed_note_among_chats(&ns, 1).await;

    let result = executor
        .execute(
            "search_nodes",
            json!({
                "query": "shared data layer",
                "sorting": [{ "field": "created_at", "direction": "desc" }]
            }),
        )
        .await
        .expect("search_nodes must succeed");

    assert!(!result.is_error);
    assert_eq!(types_of(&result.result), vec!["text"]);
}

/// Dropping conversations after the fetch must not cost the caller the rows
/// it asked for: with more matching chats than `limit`, the one note still
/// comes back. A filter applied to an already-limited page would return
/// nothing here.
#[tokio::test]
async fn matching_conversations_do_not_crowd_the_note_out_of_a_small_limit() {
    let (executor, ns, _tmp) = make_executor().await;
    let note_id = seed_note_among_chats(&ns, 5).await;

    let result = executor
        .execute(
            "search_nodes",
            json!({ "query": "shared data layer", "limit": 1 }),
        )
        .await
        .expect("search_nodes must succeed");

    assert_eq!(result.result["count"], 1);
    assert_eq!(
        result.result["nodes"][0]["id"],
        format!("nodespace://{note_id}")
    );
}

/// The wildcard spelling of "every type" is unscoped too.
#[tokio::test]
async fn a_wildcard_type_search_excludes_conversations() {
    let (executor, ns, _tmp) = make_executor().await;
    seed_note_among_chats(&ns, 1).await;

    let result = executor
        .execute(
            "search_nodes",
            json!({ "query": "shared data layer", "node_type": "*" }),
        )
        .await
        .expect("search_nodes must succeed");

    assert_eq!(types_of(&result.result), vec!["text"]);
}

/// Asking for conversations by type is refused rather than answered with an
/// empty list, which would read as "you have no conversations".
#[tokio::test]
async fn a_search_scoped_to_conversations_is_refused() {
    let (executor, ns, _tmp) = make_executor().await;
    seed_note_among_chats(&ns, 1).await;

    let err = executor
        .execute(
            "search_nodes",
            json!({ "query": "shared data layer", "node_type": "ai-chat" }),
        )
        .await
        .expect_err("a conversation-scoped search must be refused");

    assert!(
        err.to_string().contains("Conversations cannot be searched"),
        "the refusal must say why: {err}"
    );
}

/// Every chat type is a conversation: the rule reaches each subtype of the
/// chat base through the type registry, not by matching one type id.
#[tokio::test]
async fn every_chat_subtype_is_excluded() {
    let (executor, ns, _tmp) = make_executor().await;
    for chat_type in ["ai-chat-native", "ai-chat-pty"] {
        ns.create_node(Node::new(
            chat_type.to_string(),
            format!("shared data layer thread ({chat_type})"),
            json!({ "agent": "nodespace" }),
        ))
        .await
        .unwrap();
    }
    let note_id = seed_note_among_chats(&ns, 0).await;

    let result = executor
        .execute("search_nodes", json!({ "query": "shared data layer" }))
        .await
        .expect("search_nodes must succeed");

    assert_eq!(types_of(&result.result), vec!["text"]);
    assert_eq!(
        result.result["nodes"][0]["id"],
        format!("nodespace://{note_id}")
    );

    // A search scoped to the base, or to either subtype, is refused.
    for scoped in ["ai-chat", "ai-chat-native", "ai-chat-pty"] {
        let err = executor
            .execute(
                "search_nodes",
                json!({ "query": "shared data layer", "node_type": scoped }),
            )
            .await
            .expect_err("a search scoped to a chat type must be refused");
        assert!(
            err.to_string().contains("Conversations cannot be searched"),
            "{scoped}: {err}"
        );
    }
}

/// A listing with no keyword, newest first, matches every chat there is. With
/// more of them ahead of the notes than the first fetch allows for, the notes
/// must still come back: a page that came up short because chats filled it is
/// fetched again, larger. "Nothing matches" would be a wrong answer.
#[tokio::test]
async fn more_recent_chats_than_the_first_fetch_holds_do_not_hide_the_notes() {
    let (executor, ns, _tmp) = make_executor().await;
    // Created first, so every chat below is newer and sorts ahead of them.
    let first_note = seed_note_among_chats(&ns, 0).await;
    let second_note = seed_note_among_chats(&ns, 0).await;
    for i in 0..40 {
        ns.create_node(Node::new(
            "ai-chat-native".to_string(),
            format!("chat {i}"),
            json!({ "agent": "nodespace" }),
        ))
        .await
        .unwrap();
    }

    let result = executor
        .execute(
            "search_nodes",
            json!({
                "query": "",
                "node_type": "*",
                "sorting": [{ "field": "created_at", "direction": "desc" }],
                "limit": 2
            }),
        )
        .await
        .expect("search_nodes must succeed");

    let ids: Vec<String> = result.result["nodes"]
        .as_array()
        .expect("nodes is an array")
        .iter()
        .filter_map(|n| n["id"].as_str().map(str::to_owned))
        .collect();
    assert!(
        !types_of(&result.result)
            .iter()
            .any(|t| t.starts_with("ai-chat")),
        "no conversation in the result: {:?}",
        types_of(&result.result)
    );
    assert_eq!(ids.len(), 2, "limit honoured behind 40 chats: {ids:?}");
    for note in [first_note, second_note] {
        assert!(
            ids.contains(&format!("nodespace://{note}")),
            "the note behind the chats must come back: {ids:?}"
        );
    }
}
