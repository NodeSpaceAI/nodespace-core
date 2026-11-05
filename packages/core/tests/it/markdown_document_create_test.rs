//! `node_ops::create_node` takes a markdown document: the first line is the
//! node's title, front matter is its properties and the rest is parsed into
//! child nodes by the importer's parser.

use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_markdown;
use nodespace_core::ops::node_ops::{create_node, CreateNodeInput};
use nodespace_core::ops::OpsError;
use nodespace_core::services::{InsertPositionOwned, NodeService};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let mut store = Arc::new(
        SqliteStore::new(temp_dir.path().join("test.db"))
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

fn input(node_type: &str, content: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: None,
        node_type: node_type.to_string(),
        content: content.to_string(),
        parent_id: None,
        position: InsertPositionOwned::End,
        properties: json!({}),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
    }
}

/// The (type, content) of a node's children, in order.
async fn children(svc: &NodeService, id: &str) -> Vec<(String, String)> {
    svc.get_children(id)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.node_type, n.content))
        .collect()
}

/// A stored property: where the node keeps it, under the bucket of `bucket`.
async fn property(svc: &NodeService, id: &str, bucket: &str, key: &str) -> Value {
    let node = svc.get_node(id).await.unwrap().unwrap();
    node.properties
        .get(bucket)
        .and_then(|b| b.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

const BODY: &str = "Some text.\n\n## Steps\n\n1. one\n2. two\n\n```rust\nfn main() {}\n```";

#[tokio::test]
async fn a_decision_takes_a_title_and_a_parsed_body() {
    let (svc, _tmp) = test_service().await;
    let out = create_node(
        &svc,
        input("decision", &format!("# Pick a store\n\n{BODY}")),
    )
    .await
    .unwrap();

    let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
    assert_eq!(root.node_type, "decision");
    assert_eq!(root.content, "Pick a store");

    // The same text imported as a file gives the same children, in order.
    let expected: Vec<(String, String)> = prepare_nodes_from_markdown(BODY, Some("r".to_string()))
        .unwrap()
        .into_iter()
        .map(|n| (n.node_type, n.content))
        .collect();
    let types: Vec<&str> = expected.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        types.contains(&"header")
            && types.contains(&"ordered-list")
            && types.contains(&"code-block")
    );

    let mut stored = Vec::new();
    for child in svc.get_descendants(&out.node_id).await.unwrap() {
        stored.push((child.node_type, child.content));
    }
    assert_eq!(stored.len(), expected.len());
    for pair in &expected {
        assert!(stored.contains(pair), "missing {pair:?}");
    }
    let top: Vec<String> = children(&svc, &out.node_id)
        .await
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(top[0], "text");
}

#[tokio::test]
async fn the_titles_heading_marker_is_stripped_in_every_shape() {
    let (svc, _tmp) = test_service().await;
    for first_line in ["# Title", "## Title", "###### Title", "Title"] {
        let out = create_node(&svc, input("spec", &format!("{first_line}\n\nbody")))
            .await
            .unwrap();
        let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
        assert_eq!(root.content, "Title", "first line {first_line:?}");
        assert_eq!(
            children(&svc, &out.node_id).await,
            vec![("text".to_string(), "body".to_string())]
        );
    }
    // A setext heading is split as the importer splits it: its first line is
    // the title.
    let out = create_node(&svc, input("spec", "Title\n=====\n\nbody"))
        .await
        .unwrap();
    let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
    assert_eq!(root.content, "Title");
}

#[tokio::test]
async fn a_single_block_is_stored_as_given() {
    let (svc, _tmp) = test_service().await;
    for content in [
        "Just a title",
        "# A heading only",
        "two lines\nof one paragraph",
    ] {
        let out = create_node(&svc, input("decision", content)).await.unwrap();
        let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
        assert_eq!(root.content, content);
        assert!(svc.get_children(&out.node_id).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn the_markup_types_keep_multi_line_content_as_one_node() {
    let (svc, _tmp) = test_service().await;
    let cases = [
        ("code-block", "```rust\nfn a() {}\n```\n\n```\nsecond\n```"),
        ("quote-block", "> one\n\ntwo"),
        ("table", "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter"),
    ];
    for (node_type, content) in cases {
        let out = create_node(&svc, input(node_type, content)).await.unwrap();
        let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
        assert_eq!(root.content, content, "{node_type}");
        assert!(
            svc.get_children(&out.node_id).await.unwrap().is_empty(),
            "{node_type}"
        );
    }
    // A document type on the other side of the list splits the same text.
    let out = create_node(&svc, input("text", "> one\n\ntwo"))
        .await
        .unwrap();
    assert_eq!(
        svc.get_node(&out.node_id).await.unwrap().unwrap().content,
        "> one"
    );
}

#[tokio::test]
async fn a_type_named_by_a_title_template_still_rejects_content() {
    let (svc, _tmp) = test_service().await;
    let result = create_node(&svc, input("person", "Ann\n\nbody")).await;
    assert!(result.is_err(), "a person takes no content");
}

#[tokio::test]
async fn front_matter_sets_a_declared_field_and_an_invalid_value_creates_nothing() {
    let (svc, _tmp) = test_service().await;
    let out = create_node(
        &svc,
        input(
            "decision",
            "---\ndecision_status: accepted\n---\n# Pick\n\nbody",
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        property(&svc, &out.node_id, "decision", "decision_status").await,
        json!("accepted")
    );
    assert_eq!(
        svc.get_node(&out.node_id).await.unwrap().unwrap().content,
        "Pick"
    );
    assert_eq!(
        children(&svc, &out.node_id).await,
        vec![("text".to_string(), "body".to_string())]
    );
    let before = svc
        .query_nodes_by_type("decision", true)
        .await
        .unwrap()
        .len();

    let err = create_node(
        &svc,
        input(
            "decision",
            "---\ndecision_status: maybe\n---\n# Pick\n\nbody",
        ),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("decision_status"),
        "the field's own message: {err}"
    );
    assert_eq!(
        svc.query_nodes_by_type("decision", true)
            .await
            .unwrap()
            .len(),
        before,
        "nothing was created"
    );
    // Nor a stray body: the invalid root is rejected before any child lands.
    assert_eq!(
        svc.query_nodes_by_type("text", true).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn an_undeclared_key_is_stored_under_custom() {
    let (svc, _tmp) = test_service().await;
    let doc = "---\nauthor: ann\ntags: [a, b]\nowner:\n  team: core\n  size: 2\ncount: 3\ndraft: true\n---\n# T\n\nbody";
    let out = create_node(&svc, input("decision", doc)).await.unwrap();
    let id = &out.node_id;
    assert_eq!(
        property(&svc, id, "decision", "custom:author").await,
        json!("ann")
    );
    assert_eq!(
        property(&svc, id, "decision", "custom:tags").await,
        json!(["a", "b"])
    );
    assert_eq!(
        property(&svc, id, "decision", "custom:owner").await,
        json!({"team": "core", "size": 2})
    );
    assert_eq!(
        property(&svc, id, "decision", "custom:count").await,
        json!(3)
    );
    assert_eq!(
        property(&svc, id, "decision", "custom:draft").await,
        json!(true)
    );
    // No node was made for the block.
    assert_eq!(
        children(&svc, id).await,
        vec![("text".to_string(), "body".to_string())]
    );
}

#[tokio::test]
async fn namespaced_and_reserved_keys_follow_the_stated_rule() {
    let (svc, _tmp) = test_service().await;
    let doc = "---\ncustom:x: 1\norg:y: 2\n_secret: 3\nplain: 4\n---\n# T";
    let out = create_node(&svc, input("decision", doc)).await.unwrap();
    let id = &out.node_id;
    // Already namespaced: stored as given.
    assert_eq!(property(&svc, id, "decision", "custom:x").await, json!(1));
    assert_eq!(property(&svc, id, "decision", "org:y").await, json!(2));
    // Reserved: left out, not renamed.
    assert_eq!(property(&svc, id, "decision", "_secret").await, Value::Null);
    assert_eq!(
        property(&svc, id, "decision", "custom:_secret").await,
        Value::Null
    );
    assert_eq!(
        property(&svc, id, "decision", "custom:plain").await,
        json!(4)
    );
}

#[tokio::test]
async fn front_matter_without_a_title_is_refused() {
    let (svc, _tmp) = test_service().await;
    let err = create_node(&svc, input("decision", "---\na: 1\n---\n"))
        .await
        .unwrap_err();
    assert!(matches!(err, OpsError::InvalidParams(_)), "{err}");
}

#[tokio::test]
async fn a_block_that_is_not_front_matter_is_a_horizontal_line() {
    let (svc, _tmp) = test_service().await;
    // Not closed around a mapping: the first line is the title and the rest
    // parses as it always did.
    let out = create_node(&svc, input("decision", "---\n\nbody\n\n---\n\nmore"))
        .await
        .unwrap();
    let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
    assert_eq!(root.content, "---");
    let kinds: Vec<String> = children(&svc, &out.node_id)
        .await
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert!(kinds.contains(&"horizontal-line".to_string()), "{kinds:?}");
}
