//! `node_ops::create_document_node` takes a markdown document: the first line is the
//! node's title, front matter is its properties and the rest is parsed into
//! child nodes by the importer's parser.

use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_markdown;
use nodespace_core::ops::node_ops::{create_document_node, create_node, CreateNodeInput};
use nodespace_core::ops::OpsError;
use nodespace_core::schema::handle_create_schema;
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
    let out = create_document_node(
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
        let out = create_document_node(&svc, input("spec", &format!("{first_line}\n\nbody")))
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
    let out = create_document_node(&svc, input("spec", "Title\n=====\n\nbody"))
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
        let out = create_document_node(&svc, input("decision", content))
            .await
            .unwrap();
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
        let out = create_document_node(&svc, input(node_type, content))
            .await
            .unwrap();
        let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
        assert_eq!(root.content, content, "{node_type}");
        assert!(
            svc.get_children(&out.node_id).await.unwrap().is_empty(),
            "{node_type}"
        );
    }
    // A document type on the other side of the list splits the same text.
    let out = create_document_node(&svc, input("text", "one\n\ntwo"))
        .await
        .unwrap();
    assert_eq!(
        svc.get_node(&out.node_id).await.unwrap().unwrap().content,
        "one"
    );
}

#[tokio::test]
async fn a_type_named_by_a_title_template_still_rejects_content() {
    let (svc, _tmp) = test_service().await;
    let result = create_document_node(&svc, input("person", "Ann\n\nbody")).await;
    assert!(result.is_err(), "a person takes no content");
}

#[tokio::test]
async fn front_matter_sets_a_declared_field_and_an_invalid_value_creates_nothing() {
    let (svc, _tmp) = test_service().await;
    let out = create_document_node(
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

    let err = create_document_node(
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
    let out = create_document_node(&svc, input("decision", doc))
        .await
        .unwrap();
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
    let out = create_document_node(&svc, input("decision", doc))
        .await
        .unwrap();
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
    let err = create_document_node(&svc, input("decision", "---\na: 1\n---\n"))
        .await
        .unwrap_err();
    assert!(matches!(err, OpsError::InvalidParams(_)), "{err}");
}

async fn refused(svc: &Arc<NodeService>, node_type: &str, content: &str) -> OpsError {
    create_document_node(svc, input(node_type, content))
        .await
        .expect_err(&format!("{content:?} is refused"))
}

async fn node_count(svc: &NodeService) -> usize {
    let mut n = 0;
    for t in ["decision", "text", "task", "issue"] {
        n += svc.query_nodes_by_type(t, true).await.unwrap().len();
    }
    n
}

/// Only the document entry point splits: the generic create stores the content
/// as given, which is what the editor and the agent tools rely on.
#[tokio::test]
async fn the_generic_create_does_not_split_a_multi_block_node() {
    let (svc, _tmp) = test_service().await;
    for node_type in ["text", "task", "decision"] {
        let content = "# Title\n\n- a\n- b\n\n---\nk: v\n---";
        let out = create_node(&svc, input(node_type, content)).await.unwrap();
        let root = svc.get_node(&out.node_id).await.unwrap().unwrap();
        assert_eq!(root.content, content, "{node_type}");
        assert!(svc.get_children(&out.node_id).await.unwrap().is_empty());
    }
    let with_fm = "---\nauthor: ann\n---\n# T\n\nbody";
    let out = create_node(&svc, input("text", with_fm)).await.unwrap();
    assert_eq!(
        svc.get_node(&out.node_id).await.unwrap().unwrap().content,
        with_fm
    );
}

#[tokio::test]
async fn a_first_line_that_starts_another_block_is_refused_and_nothing_is_created() {
    let (svc, _tmp) = test_service().await;
    for doc in [
        "```rust\nx\n```\n\nbody",
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nbody",
        "1. x\n\nbody",
        "> q\n\nbody",
        "- a\n\nbody",
        "- [ ] t\n\nbody",
        "---\n\nbody",
    ] {
        let err = refused(&svc, "decision", doc).await;
        assert!(matches!(err, OpsError::InvalidParams(_)), "{doc:?}: {err}");
        assert!(err.to_string().contains("title"), "{doc:?}: {err}");
    }
    assert_eq!(node_count(&svc).await, 0);
}

#[tokio::test]
async fn front_matter_that_does_not_parse_is_refused_and_nothing_is_created() {
    let (svc, _tmp) = test_service().await;
    for doc in [
        "---\n1: a\n---\n# T\n\nbody",
        "---\na: 1\na: 2\n---\n# T\n\nbody",
        "---\na: [1\n---\n# T\n\nbody",
        "---\n- a\n- b\n---\n# T\n\nbody",
        "---\n\nbody\n\n---\n\nmore",
    ] {
        let err = refused(&svc, "decision", doc).await;
        assert!(err.to_string().contains("front matter"), "{doc:?}: {err}");
    }
    assert_eq!(node_count(&svc).await, 0);
}

#[tokio::test]
async fn an_explicit_property_wins_over_a_front_matter_key() {
    let (svc, _tmp) = test_service().await;
    let mut request = input(
        "decision",
        "---\ndecision_status: accepted\nauthor: ann\n---\n# T",
    );
    request.properties = json!({"decision_status": "proposed"});
    let out = create_document_node(&svc, request).await.unwrap();
    assert_eq!(
        property(&svc, &out.node_id, "decision", "decision_status").await,
        json!("proposed")
    );
    assert_eq!(
        property(&svc, &out.node_id, "decision", "custom:author").await,
        json!("ann")
    );
}

#[tokio::test]
async fn properties_that_are_not_an_object_are_refused_with_front_matter() {
    let (svc, _tmp) = test_service().await;
    let mut request = input("decision", "---\na: 1\n---\n# T");
    request.properties = json!("nope");
    let err = create_document_node(&svc, request).await.unwrap_err();
    assert!(matches!(err, OpsError::InvalidParams(_)), "{err}");
    assert_eq!(node_count(&svc).await, 0);
}

#[tokio::test]
async fn a_subtype_is_gated_by_the_type_it_extends() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({"name": "Adr", "extends": "decision", "fields": []}),
    )
    .await
    .unwrap();
    handle_create_schema(&svc, json!({"name": "Ticket", "fields": []}))
        .await
        .unwrap();
    let doc = "# T\n\nbody";

    // `issue` extends `task`, `adr` extends `decision`: both split.
    for node_type in ["issue", "adr"] {
        let out = create_document_node(&svc, input(node_type, doc))
            .await
            .unwrap();
        assert_eq!(
            svc.get_node(&out.node_id).await.unwrap().unwrap().content,
            "T",
            "{node_type}"
        );
        assert_eq!(svc.get_children(&out.node_id).await.unwrap().len(), 1);
    }
    // A user type that extends no document type keeps its content.
    let out = create_document_node(&svc, input("ticket", doc))
        .await
        .unwrap();
    assert_eq!(
        svc.get_node(&out.node_id).await.unwrap().unwrap().content,
        doc
    );
}

#[tokio::test]
async fn a_document_over_the_node_cap_or_the_size_cap_is_refused() {
    let (svc, _tmp) = test_service().await;
    let many = format!("# T\n\n{}", "p\n\n".repeat(1000));
    let err = refused(&svc, "decision", &many).await;
    assert!(
        err.to_string().contains("exceeding the maximum of 1000"),
        "{err}"
    );

    let huge = format!("# T\n\n{}", "x".repeat(1_000_001));
    let err = refused(&svc, "decision", &huge).await;
    assert!(err.to_string().contains("maximum size"), "{err}");
    assert_eq!(node_count(&svc).await, 0);
}

#[tokio::test]
async fn a_document_keeps_its_parent_position_and_collection() {
    let (svc, _tmp) = test_service().await;
    let parent = create_node(&svc, input("text", "Parent"))
        .await
        .unwrap()
        .node_id;
    let first = create_node(
        &svc,
        CreateNodeInput {
            parent_id: Some(parent.clone()),
            ..input("text", "First")
        },
    )
    .await
    .unwrap()
    .node_id;
    let doc = create_document_node(
        &svc,
        CreateNodeInput {
            parent_id: Some(parent.clone()),
            position: InsertPositionOwned::Beginning,
            ..input("decision", "# Doc\n\nbody")
        },
    )
    .await
    .unwrap();
    let kids: Vec<String> = svc
        .get_children(&parent)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(kids, vec![doc.node_id.clone(), first]);
    assert_eq!(svc.get_children(&doc.node_id).await.unwrap().len(), 1);

    // A root document can be filed into a collection; its body stays under it.
    let filed = create_document_node(
        &svc,
        CreateNodeInput {
            collections: vec!["docs".to_string()],
            ..input("decision", "# Filed\n\nbody")
        },
    )
    .await
    .unwrap();
    assert_eq!(filed.collection_ids.len(), 1);
    assert_eq!(svc.get_children(&filed.node_id).await.unwrap().len(), 1);
}

/// The root and the body are two writes: when the body cannot be written the
/// root is deleted again and the call fails.
#[tokio::test]
async fn a_body_that_cannot_be_written_leaves_no_root() {
    let (svc, tmp) = test_service().await;
    let db = libsql::Builder::new_local(tmp.path().join("test.db"))
        .build()
        .await
        .unwrap();
    db.connect()
        .unwrap()
        .execute(
            "CREATE TRIGGER refuse_body BEFORE INSERT ON node \
             WHEN new.content = 'poisoned' \
             BEGIN SELECT RAISE(ABORT, 'refused'); END",
            (),
        )
        .await
        .unwrap();

    let result = create_document_node(&svc, input("decision", "# Doc\n\nfine\n\npoisoned")).await;
    assert!(result.is_err(), "the body write fails the call");
    assert_eq!(node_count(&svc).await, 0, "no root is left behind");
}
