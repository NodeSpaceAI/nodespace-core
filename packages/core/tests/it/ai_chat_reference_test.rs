//! An ai-chat node can never be the target of a reference (ADR-061 §8).
//!
//! Rather than render a dangling or "inaccessible" link, every path that
//! creates a reference refuses an ai-chat target at write time: content
//! mentions (`@mention` / `[[wikilink]]`), the direct mention API, the generic
//! relationship API, bulk import, mention autocomplete, and schema
//! relationship declarations. An existing node cannot be retyped into a chat,
//! which would otherwise carry its inbound references along. A chat remains
//! free to be an edge's *source*.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::{Node, NodeUpdate},
    schema::handle_create_schema,
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const TEST_CLIENT_ID: &str = "test-client";

async fn create_test_service() -> Result<(NodeService, Arc<SqliteStore>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = NodeService::new(&mut store).await?;
    Ok((service, store, temp_dir))
}

/// The chats these tests reference. Any chat type would do: the rules are
/// the abstract `ai-chat` base's, and every subtype inherits them.
const CHAT: &str = "ai-chat-native";

async fn create_typed_node(service: &NodeService, node_type: &str, content: &str) -> Result<Node> {
    // A chat names who runs it; nothing else these tests create has a
    // required field.
    let properties = if node_type == CHAT {
        json!({ "agent": "nodespace" })
    } else {
        json!({})
    };
    let node = Node::new(node_type.to_string(), content.to_string(), properties);
    service
        .with_client(TEST_CLIENT_ID)
        .create_node(node.clone())
        .await?;
    Ok(service
        .get_node(&node.id)
        .await?
        .expect("node should exist after create"))
}

/// A wikilink and a markdown node link to an ai-chat in saved content produce
/// no `mentions` edge, while a link to an ordinary node in the same content
/// still does — the refusal is per-target, not a failed save.
#[tokio::test]
async fn content_mention_of_ai_chat_creates_no_edge() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let page = create_typed_node(&service, "text", "Some page").await?;
    let source = create_typed_node(&service, "text", "draft").await?;

    let content = format!(
        "See [[{}]], [the chat](nodespace://{}) and [[{}]]",
        chat.id, chat.id, page.id
    );
    service
        .with_client(TEST_CLIENT_ID)
        .update_node(
            &source.id,
            source.version,
            NodeUpdate::new().with_content(content),
        )
        .await?;

    let mentions = service.get_mentions(&source.id).await?;
    assert!(
        !mentions.contains(&chat.id),
        "an ai-chat must never become a mention target: {mentions:?}"
    );
    assert!(
        mentions.contains(&page.id),
        "the ordinary link in the same content must still be recorded: {mentions:?}"
    );
    assert!(service.get_mentioned_by(&chat.id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn create_mention_rejects_ai_chat_target() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let source = create_typed_node(&service, "text", "note").await?;

    let err = service
        .create_mention(&source.id, &chat.id)
        .await
        .expect_err("mentioning an ai-chat must be rejected");
    assert!(err.to_string().contains("ai-chat"), "{err}");
    assert!(service.get_mentions(&source.id).await?.is_empty());
    Ok(())
}

/// The generic relationship API is where a provenance link (a node pointing
/// back at the chat that produced it) would be written; the built-in
/// `mentions` name is refused there too.
#[tokio::test]
async fn create_relationship_rejects_ai_chat_target() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let source = create_typed_node(&service, "text", "note").await?;

    let err = service
        .create_relationship(&source.id, "mentions", &chat.id, json!({}))
        .await
        .expect_err("a relationship targeting an ai-chat must be rejected");
    assert!(err.to_string().contains("ai-chat"), "{err}");
    assert!(service.get_mentions(&source.id).await?.is_empty());
    Ok(())
}

/// A schema-declared relationship with no `targetType` accepts any target
/// type, so the declaration check cannot catch it: the refusal must come from
/// the transactional create path a declared relationship runs through.
#[tokio::test]
async fn untyped_declared_relationship_rejects_ai_chat_target() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;
    let service = Arc::new(service);

    handle_create_schema(
        &service,
        json!({
            "name": "finding",
            "fields": [],
            "relationships": [
                { "name": "cites", "direction": "out", "cardinality": "many", "reverseName": "cited_by", "reverseCardinality": "many" }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("schema: {e}"))?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let page = create_typed_node(&service, "text", "page").await?;
    let finding = create_typed_node(&service, "finding", "A finding").await?;

    let err = service
        .create_relationship(&finding.id, "cites", &chat.id, json!({}))
        .await
        .expect_err("a declared relationship targeting an ai-chat must be rejected");
    assert!(err.to_string().contains("ai-chat"), "{err}");

    // The same declaration still accepts an ordinary target.
    service
        .create_relationship(&finding.id, "cites", &page.id, json!({}))
        .await?;
    Ok(())
}

/// Retyping an existing node into a chat would carry its inbound references
/// into the chat, and no node may reference an ai-chat node (ADR-061 §8).
#[tokio::test]
async fn existing_node_cannot_be_retyped_to_ai_chat() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let target = create_typed_node(&service, "text", "Soon a chat?").await?;
    let source = create_typed_node(&service, "text", "note").await?;
    service.create_mention(&source.id, &target.id).await?;

    let err = service
        .with_client(TEST_CLIENT_ID)
        .update_node(
            &target.id,
            target.version,
            NodeUpdate {
                node_type: Some(CHAT.to_string()),
                ..NodeUpdate::new()
            },
        )
        .await
        .expect_err("retyping a node into an ai-chat must be rejected");
    assert!(
        err.to_string()
            .contains("cannot be converted to an ai-chat node"),
        "{err}"
    );
    assert_eq!(
        service.get_node(&target.id).await?.map(|n| n.node_type),
        Some("text".to_string())
    );
    Ok(())
}

/// An ai-chat stays a legal edge *source*: it may still mention other nodes.
#[tokio::test]
async fn ai_chat_can_still_mention_other_nodes() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let target = create_typed_node(&service, "text", "page").await?;

    service.create_mention(&chat.id, &target.id).await?;
    assert!(service.get_mentions(&chat.id).await?.contains(&target.id));
    Ok(())
}

/// Nesting a chat under a page is outline placement, not a reference: the
/// `has_child` edge onto an ai-chat is still allowed.
#[tokio::test]
async fn has_child_onto_ai_chat_is_still_allowed() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let page = create_typed_node(&service, "text", "Topic page").await?;
    let chat = create_typed_node(&service, CHAT, "Private chat").await?;

    service
        .create_relationship(&page.id, "has_child", &chat.id, json!({}))
        .await?;
    assert_eq!(
        service.get_parent(&chat.id).await?.map(|p| p.id),
        Some(page.id.clone())
    );
    Ok(())
}

/// Import writes mentions in bulk below the service; the ai-chat target is
/// dropped there like a dangling link, without failing the rest of the batch.
#[tokio::test]
async fn bulk_create_mentions_skips_ai_chat_target() -> Result<()> {
    let (service, store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Private chat").await?;
    let page = create_typed_node(&service, "text", "page").await?;
    let source = create_typed_node(&service, "text", "note").await?;

    let created = store
        .bulk_create_mentions(&[
            (source.id.clone(), chat.id.clone()),
            (source.id.clone(), page.id.clone()),
        ])
        .await?;

    assert_eq!(created, 1);
    let mentions = service.get_mentions(&source.id).await?;
    assert_eq!(mentions, vec![page.id.clone()]);
    Ok(())
}

#[tokio::test]
async fn mention_autocomplete_does_not_offer_ai_chats() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, CHAT, "Quarterly planning").await?;
    let page = create_typed_node(&service, "text", "Quarterly planning notes").await?;

    let results = service.mention_autocomplete("quarterly", None).await?;
    let ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&page.id.as_str()), "{ids:?}");
    assert!(!ids.contains(&chat.id.as_str()), "{ids:?}");
    Ok(())
}
