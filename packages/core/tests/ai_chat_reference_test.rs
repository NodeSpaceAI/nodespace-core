//! An ai-chat node can never be the target of a reference.
//!
//! Chats are private by default, so a reference *to* one would surface a
//! private conversation's existence and title to readers who cannot open it.
//! Rather than render a dangling or "inaccessible" link, every path that
//! creates a reference refuses an ai-chat target at write time: content
//! mentions (`@mention` / `[[wikilink]]`), the direct mention API, the generic
//! relationship API, bulk import, mention autocomplete, and schema
//! relationship declarations. A chat remains free to be an edge's *source*.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::{Node, NodeUpdate},
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

async fn create_typed_node(service: &NodeService, node_type: &str, content: &str) -> Result<Node> {
    let node = Node::new(node_type.to_string(), content.to_string(), json!({}));
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

    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;
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

    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;
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

    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;
    let source = create_typed_node(&service, "text", "note").await?;

    let err = service
        .create_relationship(&source.id, "mentions", &chat.id, json!({}))
        .await
        .expect_err("a relationship targeting an ai-chat must be rejected");
    assert!(err.to_string().contains("ai-chat"), "{err}");
    assert!(service.get_mentions(&source.id).await?.is_empty());
    Ok(())
}

/// An ai-chat stays a legal edge *source*: it may still mention other nodes.
#[tokio::test]
async fn ai_chat_can_still_mention_other_nodes() -> Result<()> {
    let (service, _store, _t) = create_test_service().await?;

    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;
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
    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;

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

    let chat = create_typed_node(&service, "ai-chat", "Private chat").await?;
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

    let chat = create_typed_node(&service, "ai-chat", "Quarterly planning").await?;
    let page = create_typed_node(&service, "text", "Quarterly planning notes").await?;

    let results = service.mention_autocomplete("quarterly", None).await?;
    let ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&page.id.as_str()), "{ids:?}");
    assert!(!ids.contains(&chat.id.as_str()), "{ids:?}");
    Ok(())
}
