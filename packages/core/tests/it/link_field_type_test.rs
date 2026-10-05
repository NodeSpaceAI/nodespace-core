//! A `link` field holds a title and an absolute URL, and an `array` field may
//! hold a list of them (ADR-092 §4). Every write is checked against that
//! shape, and a schema cannot mark a link field unique.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    ops::node_ops,
    schema::{handle_create_schema, handle_update_schema},
    services::{InsertPositionOwned, NodeService},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

async fn seed_repo_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Repo",
            "fields": [
                { "name": "repository", "type": "link" },
                { "name": "commits", "type": "array", "itemType": "link" }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("repo schema: {e}"))?;
    Ok(())
}

async fn create_repo(svc: &Arc<NodeService>, properties: Value) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "repo".to_string(),
            content: "Core".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(output.node_id)
}

async fn update_repo(svc: &Arc<NodeService>, id: &str, properties: Value) -> Result<()> {
    node_ops::update_node(
        svc,
        node_ops::UpdateNodeInput {
            node_id: id.to_string(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(properties),
            add_to_collections: vec![],
            add_to_collection_ids: vec![],
            remove_from_collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(())
}

async fn repo_props(svc: &Arc<NodeService>, id: &str) -> Result<Value> {
    let node = node_ops::get_node(
        svc,
        node_ops::GetNodeInput {
            node_id: id.to_string(),
        },
    )
    .await?;
    Ok(node["properties"].clone())
}

/// Assert creation with `properties` is refused, naming `field` and `problem`.
async fn assert_refused(svc: &Arc<NodeService>, properties: Value, field: &str, problem: &str) {
    let msg = create_repo(svc, properties.clone())
        .await
        .expect_err(&format!("{properties} must be refused"))
        .to_string();
    assert!(
        msg.contains(&format!("'{field}'")),
        "error must name the field, got: {msg}"
    );
    assert!(msg.contains(problem), "expected '{problem}', got: {msg}");
}

#[tokio::test]
async fn a_link_is_stored_as_its_title_and_url() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_repo_schema(&svc).await?;

    let link = json!({ "title": "Core", "url": "https://github.com/NodeSpaceAI/core" });
    let commit =
        json!({ "title": "a1b2c3", "url": "https://github.com/NodeSpaceAI/core/commit/a1b2c3" });
    let id = create_repo(
        &svc,
        json!({ "repository": link, "commits": [commit.clone()] }),
    )
    .await?;
    let props = repo_props(&svc, &id).await?;
    assert_eq!(props["repository"], link);
    assert_eq!(props["commits"], json!([commit]));

    // The scheme is not restricted where a link is stored.
    let ssh = json!({ "title": "Core", "url": "ssh://git@github.com/NodeSpaceAI/core.git" });
    update_repo(&svc, &id, json!({ "repository": ssh })).await?;
    assert_eq!(repo_props(&svc, &id).await?["repository"], ssh);

    // Null clears the field, and an empty list is a list.
    update_repo(&svc, &id, json!({ "repository": null, "commits": [] })).await?;
    let props = repo_props(&svc, &id).await?;
    assert!(props.get("repository").is_none_or(Value::is_null));
    assert_eq!(props["commits"], json!([]));
    Ok(())
}

#[tokio::test]
async fn a_malformed_link_is_refused_naming_the_field_and_the_problem() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_repo_schema(&svc).await?;
    let url = "https://example.com/a";

    for (value, problem) in [
        (json!(url), "received the string"),
        (
            json!(7),
            "an object with exactly the keys 'title' and 'url'",
        ),
        (json!({ "url": url }), "missing 'title'"),
        (json!({ "title": "A" }), "missing 'url'"),
        (
            json!({ "title": "A", "url": url, "label": "x" }),
            "unknown key 'label'",
        ),
        (
            json!({ "title": 1, "url": url }),
            "'title' that is not a string",
        ),
        (
            json!({ "title": "A", "url": ["x"] }),
            "'url' that is not a string",
        ),
        (
            json!({ "title": "A", "url": "example.com/a" }),
            "not an absolute URL",
        ),
        (json!({ "title": "A", "url": "/a" }), "not an absolute URL"),
    ] {
        assert_refused(&svc, json!({ "repository": value }), "repository", problem).await;
    }

    // A list of links checks each one, and says which item is wrong.
    assert_refused(
        &svc,
        json!({ "commits": { "title": "A", "url": url } }),
        "commits",
        "declared as type 'array' but received an object",
    )
    .await;
    assert_refused(
        &svc,
        json!({ "commits": [{ "title": "A", "url": url }, { "title": "B" }] }),
        "commits",
        "item 1 is missing 'url'",
    )
    .await;

    // An update is checked like a create.
    let id = create_repo(&svc, json!({})).await?;
    let msg = update_repo(&svc, &id, json!({ "repository": url }))
        .await
        .expect_err("a bare string is not a link")
        .to_string();
    assert!(msg.contains("Link field 'repository'"), "{msg}");
    Ok(())
}

#[tokio::test]
async fn a_link_field_cannot_be_marked_unique() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;

    for field in [
        json!({ "name": "home", "type": "link", "unique": true }),
        json!({ "name": "home", "type": "link", "uniqueCaseInsensitive": true }),
        json!({ "name": "home", "type": "array", "itemType": "link", "unique": true }),
    ] {
        let msg = handle_create_schema(&svc, json!({ "name": "Site", "fields": [field] }))
            .await
            .expect_err("a unique link field must be refused")
            .to_string();
        assert!(
            msg.contains("Link field 'home' cannot be marked unique"),
            "{msg}"
        );
    }
    assert!(svc.get_schema_node("site").await?.is_none());

    // `update_schema` adds a link field, and refuses a unique one.
    handle_create_schema(&svc, json!({ "name": "Site", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "site",
            "add_fields": [
                { "name": "home", "type": "link" },
                { "name": "mirrors", "type": "array", "itemType": "link" }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let msg = handle_update_schema(
        &svc,
        json!({
            "schema_id": "site",
            "add_fields": [{ "name": "docs", "type": "link", "unique": true }]
        }),
    )
    .await
    .expect_err("a unique link field must be refused")
    .to_string();
    assert!(
        msg.contains("Link field 'docs' cannot be marked unique"),
        "{msg}"
    );

    let schema = svc.get_schema_node("site").await?.expect("site schema");
    let types: Vec<String> = schema
        .fields
        .iter()
        .map(|f| format!("{}:{}", f.name, f.field_type))
        .collect();
    assert_eq!(types, ["home:link", "mirrors:array"]);
    assert_eq!(
        schema.fields[1].item_type,
        Some(nodespace_core::models::SchemaFieldType::Link)
    );
    Ok(())
}

/// A title is plain text: a title template that names a link field takes the
/// link's title.
#[tokio::test]
async fn a_link_in_a_title_template_renders_as_its_title() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Bookmark",
            "fields": [{ "name": "target", "type": "link" }],
            "title_template": "{target}"
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let output = node_ops::create_node(
        &svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "bookmark".to_string(),
            content: String::new(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({
                "target": { "title": "Docs", "url": "https://example.com/docs" }
            }),
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    let node = svc.get_node(&output.node_id).await?.expect("bookmark");
    assert_eq!(node.title.as_deref(), Some("Docs"));
    Ok(())
}
