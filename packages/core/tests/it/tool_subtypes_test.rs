//! The tool type family, end to end against a real store (ADR-086 §12):
//! `tool` is an abstract base that is never instantiated, `tool-native` is the
//! core subtype NodeSpace's built-in tools are, the base's rules reach every
//! subtype, and no tool carries a `source`: its type says where it comes from.

use nodespace_core::behaviors::ToolOrigin;
use nodespace_core::db::{SqliteStore, TreeInvariantRule};
use nodespace_core::models::{node_to_typed_value, CoreNodeType, Node, NodeUpdate};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NodeService, NodeServiceError,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const NATIVE: &str = "tool-native";
/// A stand-in for a remote tool subtype: it extends `tool` and is not native.
const REMOTE: &str = "remote_tool";

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

async fn native_tool(svc: &Arc<NodeService>) -> String {
    create(
        svc,
        NATIVE,
        "search_nodes",
        json!({
            "handler": "search_nodes",
            "description": "Search nodes by keyword",
            "parameter_schema": { "type": "object", "properties": {} },
        }),
    )
    .await
    .expect("a native tool is created")
}

async fn seed_remote_subtype(svc: &Arc<NodeService>) {
    handle_create_schema(
        svc,
        json!({
            "name": "Remote Tool",
            "extends": "tool",
            "fields": [{ "name": "endpoint", "type": "text" }],
        }),
    )
    .await
    .expect("the fixture remote tool subtype");
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
async fn a_bare_tool_is_refused_on_every_create_path() {
    let (svc, _tmp) = test_service().await;
    let abstract_error = |e: String| assert!(e.contains("abstract type"), "{e}");
    let properties = json!({ "description": "Looks a thing up" });

    abstract_error(message(
        create(&svc, "tool", "lookup", properties.clone()).await,
    ));

    abstract_error(message(
        svc.create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "tool".to_string(),
            content: "lookup".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: properties.clone(),
            lifecycle_status: None,
        })
        .await,
    ));

    abstract_error(message(
        svc.bulk_create(vec![Node::new(
            "tool".to_string(),
            "lookup".to_string(),
            properties.clone(),
        )])
        .await,
    ));

    abstract_error(message(
        svc.bulk_create_hierarchy(vec![(
            uuid::Uuid::new_v4().to_string(),
            "tool".to_string(),
            "lookup".to_string(),
            None,
            0.0,
            properties,
        )])
        .await,
    ));

    assert!(svc
        .query_nodes_by_type("tool", true)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_tool_cannot_be_retyped_into_the_abstract_base() {
    let (svc, _tmp) = test_service().await;
    let id = native_tool(&svc).await;
    let version = get(&svc, &id).await.version;

    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_node_type("tool".to_string()),
        )
        .await,
    );
    assert!(error.contains("abstract type"), "{error}");
    assert_eq!(get(&svc, &id).await.node_type, NATIVE);
}

#[tokio::test]
async fn the_registry_and_the_store_agree_on_the_family() {
    let (svc, _tmp) = test_service().await;

    assert!(svc.store().is_abstract_type("tool").await.unwrap());
    assert!(!svc.store().is_abstract_type(NATIVE).await.unwrap());
    assert_eq!(
        svc.store().type_chain(NATIVE).await.unwrap(),
        vec![NATIVE, "tool"]
    );
    assert!(svc.type_is_a(NATIVE, CoreNodeType::Tool).await.unwrap());
    // The seeded `extends` edge is what the registry says it is, so the
    // ancestry table SQL rules read agrees with it.
    let parents = svc.store().get_extends_parent_map().await.unwrap();
    assert_eq!(parents.get(NATIVE).map(String::as_str), Some("tool"));
}

// ---------------------------------------------------------------------------
// The native subtype: fields, buckets and the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_native_tool_stores_each_field_in_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = native_tool(&svc).await;

    let node = get(&svc, &id).await;
    // Base fields live in the base's bucket, with the schema default filled
    // in; the handler lives in the subtype's own. A tool is not enabled until
    // something says so.
    assert_eq!(
        node.properties["tool"],
        json!({
            "description": "Search nodes by keyword",
            "parameter_schema": { "type": "object", "properties": {} },
            "enabled": false,
        })
    );
    assert_eq!(
        node.properties[NATIVE],
        json!({ "handler": "search_nodes" })
    );
}

/// A tool travels as the generic node: its whole chain's fields inside
/// `properties`, which is where the agent's tool list reads them.
#[tokio::test]
async fn a_native_tool_travels_with_its_chains_fields_in_properties() {
    let (svc, _tmp) = test_service().await;
    let id = native_tool(&svc).await;

    let collapsed = svc
        .collapse_chain_for_wire(vec![get(&svc, &id).await])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let wire = node_to_typed_value(collapsed).unwrap();

    assert_eq!(wire["nodeType"], NATIVE);
    assert_eq!(wire["content"], "search_nodes");
    assert_eq!(
        wire["properties"],
        json!({
            "handler": "search_nodes",
            "description": "Search nodes by keyword",
            "parameter_schema": { "type": "object", "properties": {} },
            "enabled": false,
        })
    );
}

#[tokio::test]
async fn a_native_tool_without_a_handler_is_refused() {
    let (svc, _tmp) = test_service().await;
    for properties in [
        json!({ "description": "No handler" }),
        json!({ "description": "Blank handler", "handler": "  " }),
    ] {
        let error = message(create(&svc, NATIVE, "lookup", properties).await);
        assert!(error.contains("handler"), "{error}");
    }
}

/// Both buckets are closed. `source` is gone: the subtype says where a tool
/// comes from.
#[tokio::test]
async fn a_tool_takes_only_its_declared_fields() {
    let (svc, _tmp) = test_service().await;

    for foreign in [
        json!({ "source": "internal" }),
        json!({ "source": "external" }),
        json!({ "anything": 1 }),
    ] {
        let mut properties = foreign.clone();
        properties["handler"] = json!("lookup");
        let error = message(create(&svc, NATIVE, "lookup", properties).await);
        let key = foreign.as_object().unwrap().keys().next().unwrap().clone();
        assert!(error.contains(&key), "{foreign}: {error}");
    }

    // A namespaced extension field is still welcome.
    create(
        &svc,
        NATIVE,
        "lookup",
        json!({ "handler": "lookup", "custom:pinned": true }),
    )
    .await
    .expect("an extension field is accepted");
}

/// A flat patch names base and subtype fields alike; each lands in the
/// bucket of the schema that declares it.
#[tokio::test]
async fn a_flat_update_lands_each_field_in_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = native_tool(&svc).await;
    let version = get(&svc, &id).await.version;

    svc.update_node(
        &id,
        version,
        NodeUpdate::new().with_properties(json!({
            "description": "Edited",
            "enabled": false,
            "handler": "search_semantic",
        })),
    )
    .await
    .unwrap();

    let node = get(&svc, &id).await;
    assert_eq!(node.properties["tool"]["description"], "Edited");
    assert_eq!(node.properties["tool"]["enabled"], false);
    assert_eq!(
        node.properties[NATIVE],
        json!({ "handler": "search_semantic" })
    );
}

/// A native tool's CLI command is its subtype's field: optional, stored in
/// the subtype's bucket beside the handler, set and cleared by a flat update,
/// and carried in `properties` on the wire like the rest of the chain's
/// fields.
#[tokio::test]
async fn a_native_tools_cli_command_is_an_optional_field_of_its_own_bucket() {
    let (svc, _tmp) = test_service().await;

    // Unset unless stated: the schema gives it no default.
    let plain = native_tool(&svc).await;
    assert!(get(&svc, &plain).await.properties[NATIVE]
        .get("cli_command")
        .is_none());

    let id = create(
        &svc,
        NATIVE,
        "get_node",
        json!({ "handler": "get_node", "cli_command": "nodespace node get" }),
    )
    .await
    .expect("a native tool with a command is created");
    let node = get(&svc, &id).await;
    assert_eq!(
        node.properties[NATIVE],
        json!({ "handler": "get_node", "cli_command": "nodespace node get" })
    );
    assert!(node.properties["tool"].get("cli_command").is_none());

    let collapsed = svc
        .collapse_chain_for_wire(vec![node.clone()])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let wire = node_to_typed_value(collapsed).unwrap();
    assert_eq!(wire["properties"]["cli_command"], "nodespace node get");

    svc.update_node(
        &id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "cli_command": "nodespace node export" })),
    )
    .await
    .expect("a flat update sets the command");
    let node = get(&svc, &id).await;
    assert_eq!(
        node.properties[NATIVE]["cli_command"],
        "nodespace node export"
    );

    svc.update_node(
        &id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "cli_command": null })),
    )
    .await
    .expect("a flat update clears the command");
    let node = get(&svc, &id).await;
    assert!(node.properties[NATIVE]
        .get("cli_command")
        .is_none_or(serde_json::Value::is_null));
    assert_eq!(node.properties[NATIVE]["handler"], "get_node");

    // Text, like the schema says.
    let error = message(
        create(
            &svc,
            NATIVE,
            "lookup",
            json!({ "handler": "lookup", "cli_command": 7 }),
        )
        .await,
    );
    assert!(error.contains("cli_command"), "{error}");
}

// ---------------------------------------------------------------------------
// The base's rules reach every subtype
// ---------------------------------------------------------------------------

/// A query for the abstract base returns every subtype's nodes, each under
/// its own type.
#[tokio::test]
async fn a_query_for_tool_returns_every_subtype() {
    let (svc, _tmp) = test_service().await;
    seed_remote_subtype(&svc).await;
    let native = native_tool(&svc).await;
    let remote = create(
        &svc,
        REMOTE,
        "remote_lookup",
        json!({ "endpoint": "https://example.invalid/call", "enabled": false }),
    )
    .await
    .expect("a remote-style tool needs no handler");
    create(&svc, "text", "Not a tool", json!({})).await.unwrap();

    let found = svc.query_nodes_by_type("tool", false).await.unwrap();
    let mut types: Vec<(&str, &str)> = found
        .iter()
        .map(|n| (n.id.as_str(), n.node_type.as_str()))
        .collect();
    types.sort_unstable();
    let mut expected = vec![(native.as_str(), NATIVE), (remote.as_str(), REMOTE)];
    expected.sort_unstable();
    assert_eq!(types, expected);

    let only_native = svc.query_nodes_by_type(NATIVE, false).await.unwrap();
    assert_eq!(
        only_native
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec![native.as_str()]
    );
}

/// The trust gate is decided by the subtype, from the chain the store
/// resolves: a native tool is always offered, and a subtype that is not
/// native only when `enabled`.
#[tokio::test]
async fn the_trust_gate_follows_the_stored_chain() {
    let (svc, _tmp) = test_service().await;
    seed_remote_subtype(&svc).await;

    let native = svc.resolve_type_chain(NATIVE).await.unwrap();
    let remote = svc.resolve_type_chain(REMOTE).await.unwrap();
    assert_eq!(remote, vec![REMOTE, "tool"]);
    assert_eq!(ToolOrigin::of(&native), Some(ToolOrigin::Native));
    let remote_origin = ToolOrigin::of(&remote).expect("a type extending tool is a tool");
    assert_eq!(remote_origin, ToolOrigin::External);

    assert!(ToolOrigin::Native.is_offered(false));
    assert!(!remote_origin.is_offered(false));
    assert!(remote_origin.is_offered(true));

    // A tool that doesn't say is stored as not enabled, so the gate is closed
    // until someone opens it.
    let unstated = create(&svc, REMOTE, "remote_lookup", json!({}))
        .await
        .unwrap();
    let stored = get(&svc, &unstated).await;
    let enabled = stored.properties["tool"]["enabled"]
        .as_bool()
        .expect("the default is stored");
    assert!(!enabled);
    assert!(!remote_origin.is_offered(enabled));
}

/// The parameter-schema guard is the base's, so it holds for a native tool
/// and for a subtype that is not native alike.
#[tokio::test]
async fn an_unbounded_parameter_schema_is_refused_for_every_subtype() {
    let (svc, _tmp) = test_service().await;
    seed_remote_subtype(&svc).await;
    let unbounded = json!({ "type": "object", "additionalProperties": true });

    let error = message(
        create(
            &svc,
            NATIVE,
            "lookup",
            json!({ "handler": "lookup", "parameter_schema": unbounded.clone() }),
        )
        .await,
    );
    assert!(error.contains("additionalProperties"), "{error}");

    let error = message(
        create(
            &svc,
            REMOTE,
            "remote_lookup",
            json!({ "parameter_schema": unbounded }),
        )
        .await,
    );
    assert!(error.contains("additionalProperties"), "{error}");

    // So is the name rule.
    for (node_type, properties) in [
        (NATIVE, json!({ "handler": "lookup" })),
        (REMOTE, json!({})),
    ] {
        message(create(&svc, node_type, "  ", properties).await);
    }
}

/// A tool takes no children, whichever subtype it is: the base's structural
/// rule, enforced through the type ancestry.
#[tokio::test]
async fn no_tool_takes_children() {
    let (svc, _tmp) = test_service().await;
    seed_remote_subtype(&svc).await;
    let native = native_tool(&svc).await;
    let remote = create(&svc, REMOTE, "remote_lookup", json!({}))
        .await
        .unwrap();

    for (node_type, parent) in [(NATIVE, native), (REMOTE, remote)] {
        let refusal = svc
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "child".to_string(),
                parent_id: Some(parent),
                position: InsertPositionOwned::End,
                properties: json!({}),
                lifecycle_status: None,
            })
            .await
            .expect_err("a tool takes no children");
        match refusal {
            NodeServiceError::TreeInvariantViolation(v) => {
                assert_eq!(v.rule, TreeInvariantRule::ChildrenNone, "{node_type}")
            }
            other => panic!("{node_type}: expected a tree invariant violation, got {other:?}"),
        }
    }
}
