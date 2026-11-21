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
const REMOTE: &str = "remote-tool";

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

// ---------------------------------------------------------------------------
// The remote subtypes: tool-mcp, tool-rest, tool-cli
// ---------------------------------------------------------------------------

const REMOTE_KINDS: [&str; 3] = ["tool-mcp", "tool-rest", "tool-cli"];

/// A closed root with the parameters the fixtures below use.
fn closed_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "id": { "type": "string" }, "q": { "type": "string" } },
    })
}

/// A complete binding for each kind, to be enabled or left as a draft.
fn complete_binding(kind: &str) -> serde_json::Value {
    match kind {
        "tool-mcp" => json!({
            "transport": "http",
            "url": "https://mcp.example.com/rpc",
            "remote_tool_name": "search",
            "auth": { "scheme": "bearer", "secret": "keychain:mcp-token" },
        }),
        "tool-rest" => json!({
            "method": "GET",
            "url_template": "https://api.example.com/items/{id}",
            "query_params": ["q"],
        }),
        _ => json!({ "program": "git", "args": ["log", "{q}"] }),
    }
}

fn remote_properties(kind: &str, enabled: bool) -> serde_json::Value {
    let mut properties = complete_binding(kind);
    properties["description"] = json!("Looks a thing up");
    properties["parameter_schema"] = closed_schema();
    properties["enabled"] = json!(enabled);
    properties
}

#[tokio::test]
async fn the_remote_kinds_are_tools_found_by_a_query_for_the_base() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        assert_eq!(
            svc.store().type_chain(kind).await.unwrap(),
            vec![kind, "tool"]
        );
        assert!(!svc.store().is_abstract_type(kind).await.unwrap());
        assert!(svc.type_is_a(kind, CoreNodeType::Tool).await.unwrap());
        create(
            &svc,
            kind,
            &format!("lookup_{}", kind.replace('-', "_")),
            remote_properties(kind, false),
        )
        .await
        .unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
    let tools = svc.query_nodes_by_type("tool", true).await.unwrap();
    let mut found: Vec<&str> = tools.iter().map(|n| n.node_type.as_str()).collect();
    found.sort();
    assert_eq!(found, vec!["tool-cli", "tool-mcp", "tool-rest"]);
    for kind in REMOTE_KINDS {
        assert_eq!(
            ToolOrigin::of(&svc.store().type_chain(kind).await.unwrap()),
            Some(ToolOrigin::External)
        );
    }
}

#[tokio::test]
async fn a_remote_tool_stores_its_binding_in_its_own_bucket() {
    let (svc, _tmp) = test_service().await;
    let id = create(
        &svc,
        "tool-cli",
        "git_log",
        remote_properties("tool-cli", false),
    )
    .await
    .unwrap();
    let node = get(&svc, &id).await;
    assert_eq!(node.properties["tool"]["enabled"], false);
    assert_eq!(node.properties["tool-cli"]["program"], "git");
    // The schema default is the only source of a default.
    assert_eq!(node.properties["tool-cli"]["timeout_seconds"], 30);
    assert_eq!(node.properties["tool-cli"]["max_output_bytes"], 1_048_576);

    let rest = create(
        &svc,
        "tool-rest",
        "get_item",
        remote_properties("tool-rest", false),
    )
    .await
    .unwrap();
    let rest = get(&svc, &rest).await;
    assert_eq!(rest.properties["tool-rest"]["body"], "none");
    assert_eq!(
        rest.properties["tool-rest"]["max_response_bytes"],
        1_048_576
    );
}

#[tokio::test]
async fn the_approved_binding_is_a_fingerprint_or_unset() {
    let (svc, _tmp) = test_service().await;
    let fingerprint = "0123456789abcdef".repeat(4);
    let mut ok = remote_properties("tool-cli", false);
    ok["approved_binding"] = json!(fingerprint);
    create(&svc, "tool-cli", "approved", ok).await.unwrap();
    for bad in [json!("not-a-hash"), json!("ABCDEF".repeat(11)), json!(7)] {
        let mut properties = remote_properties("tool-cli", false);
        properties["approved_binding"] = bad.clone();
        let error = message(create(&svc, "tool-cli", "bad", properties).await);
        assert!(error.contains("approved_binding"), "{bad}: {error}");
    }
}

/// A well-formed approval tag (64 lowercase hex), for writes that stand in
/// for an approval.
const TAG: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Writes an approval of `id` as the daemon's approval does: its record opened
/// and made live by this write. Returns the node as it is then.
async fn approve_here(svc: &Arc<NodeService>, id: &str) -> Node {
    let node = get(svc, id).await;
    svc.write_tool_approval(
        id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "enabled": true, "approved_binding": TAG })),
        TAG,
        None,
    )
    .await
    .unwrap();
    let node = get(svc, id).await;
    assert!(svc.live_tool_approval(&node, TAG).is_some());
    node
}

/// Every write the node service reports after the approval's own ends its
/// record: an update (through the service or through the store itself) and a
/// delete, on every delete path.
#[tokio::test]
async fn a_delete_ends_the_approval_record_on_every_delete_path() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        let mut ids = Vec::new();
        for name in ["versioned", "unchecked", "bulk", "pruned"] {
            let id = create(&svc, kind, name, remote_properties(kind, true))
                .await
                .unwrap();
            approve_here(&svc, &id).await;
            assert!(svc.tool_approval_recorded(&id), "{kind} {name}");
            ids.push(id);
        }
        // An update is not the approval's own write: it ends the record too.
        let node = get(&svc, &ids[0]).await;
        svc.update_node(
            &ids[0],
            node.version,
            NodeUpdate::new().with_properties(json!({ "description": "Reworded" })),
        )
        .await
        .unwrap();
        assert!(!svc.tool_approval_recorded(&ids[0]), "{kind}: update");
        approve_here(&svc, &ids[0]).await;
        // So does a write made through the store itself (which reports it
        // through the store's notifier rather than as an emitted event).
        let mut properties = get(&svc, &ids[0]).await.properties;
        properties["tool"]["description"] = json!("Again");
        svc.store()
            .update_node(
                &ids[0],
                NodeUpdate::new().with_properties(properties),
                None,
                nodespace_core::db::WriteContext::default(),
            )
            .await
            .unwrap();
        assert!(!svc.tool_approval_recorded(&ids[0]), "{kind}: store update");
        let node = approve_here(&svc, &ids[0]).await;
        svc.delete_node(&ids[0], node.version).await.unwrap();
        svc.delete_node_unchecked(&ids[1]).await.unwrap();
        svc.bulk_delete(vec![ids[2].clone()]).await.unwrap();
        // Pruned by an import, in its transaction.
        svc.bulk_create_hierarchy_trusted_with(
            vec![(
                uuid::Uuid::new_v4().to_string(),
                "text".to_string(),
                "imported".to_string(),
                None,
                1.0,
                json!({}),
            )],
            nodespace_core::services::TrustedImportFollowUp {
                archive_ids: vec![],
                refresh_root: None,
                prune_ids: vec![ids[3].clone()],
            },
        )
        .await
        .unwrap();
        for (id, path) in ids.iter().zip(["versioned", "unchecked", "bulk", "pruned"]) {
            assert!(!svc.tool_approval_recorded(id), "{kind}: {path} delete");
        }
    }
}

/// A tool deleted through the node service and then put back exactly as it was
/// (its old version and creation time, by writing the database file directly)
/// is not approved: the delete took the record with it.
#[tokio::test]
async fn a_tool_put_back_by_writing_the_database_directly_is_not_approved() {
    let (svc, tmp) = test_service().await;
    let id = create(
        &svc,
        "tool-rest",
        "lookup",
        remote_properties("tool-rest", true),
    )
    .await
    .unwrap();
    let node = approve_here(&svc, &id).await;
    let db = libsql::Builder::new_local(tmp.path().join("test.db"))
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn
        .query(
            "SELECT node_type, content, properties, title, lifecycle_status, version, created_at, modified_at \
             FROM node WHERE id = ?1",
            libsql::params![id.clone()],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let columns: Vec<libsql::Value> = (0..8).map(|i| row.get_value(i).unwrap()).collect();
    drop(rows);
    drop(conn);
    svc.delete_node(&id, node.version).await.unwrap();
    let mut params = vec![libsql::Value::Text(id.clone())];
    params.extend(columns);
    // A fresh connection: the read above holds an older snapshot.
    db.connect().unwrap().execute(
        "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params,
    )
    .await
    .unwrap();
    let back = get(&svc, &id).await;
    assert_eq!(
        (back.version, back.created_at),
        (node.version, node.created_at)
    );
    assert!(svc.live_tool_approval(&back, TAG).is_none());
}

/// A merge rewrites the survivor and archives the loser without a node event:
/// it ends the approval of both.
#[tokio::test]
async fn a_merge_ends_the_approval_of_both_tools() {
    let (svc, _tmp) = test_service().await;
    let survivor = create(
        &svc,
        "tool-rest",
        "lookup",
        remote_properties("tool-rest", true),
    )
    .await
    .unwrap();
    let loser = create(
        &svc,
        "tool-rest",
        "lookup_two",
        remote_properties("tool-rest", true),
    )
    .await
    .unwrap();
    approve_here(&svc, &survivor).await;
    approve_here(&svc, &loser).await;
    svc.merge_nodes(&survivor, &loser, None).await.unwrap();
    assert!(!svc.tool_approval_recorded(&survivor));
    assert!(!svc.tool_approval_recorded(&loser));
}

/// A merge of two schemas re-points what extends the loser onto the
/// survivor: like any schema write, it ends every approval of the database.
#[tokio::test]
async fn a_merge_of_two_schemas_ends_every_approval_of_the_database() {
    let (svc, _tmp) = test_service().await;
    for name in ["Kind A", "Kind B"] {
        handle_create_schema(
            &svc,
            json!({ "name": name, "extends": "tool-rest", "fields": [] }),
        )
        .await
        .unwrap();
    }
    let id = create(
        &svc,
        "tool-rest",
        "lookup",
        remote_properties("tool-rest", true),
    )
    .await
    .unwrap();
    approve_here(&svc, &id).await;
    svc.merge_nodes("kind-a", "kind-b", None).await.unwrap();
    assert!(!svc.tool_approval_recorded(&id));
}

/// A write made through the store ends the approval when it is made, not
/// when its read-back reports it: on each of the store's update paths, a
/// write that lands but whose read-back fails (here, of a row the read cannot
/// parse) still leaves no approval.
#[tokio::test]
async fn a_store_update_ends_the_approval_even_when_its_read_back_fails() {
    let (svc, tmp) = test_service().await;
    let mut ids = Vec::new();
    let mut versions = Vec::new();
    for name in ["plain", "versioned", "retyped"] {
        let id = create(
            &svc,
            "tool-rest",
            name,
            remote_properties("tool-rest", true),
        )
        .await
        .unwrap();
        versions.push(approve_here(&svc, &id).await.version);
        ids.push(id);
    }
    // Any later write of these rows lands, and leaves them unreadable (a
    // modification time the read cannot parse).
    let db = libsql::Builder::new_local(tmp.path().join("test.db"))
        .build()
        .await
        .unwrap();
    db.connect()
        .unwrap()
        .execute(
            &format!(
                "CREATE TRIGGER break_read_back AFTER UPDATE OF version ON node \
                 WHEN NEW.id IN ('{}', '{}', '{}') \
                 BEGIN UPDATE node SET modified_at = 'not a time' WHERE id = NEW.id; END",
                ids[0], ids[1], ids[2]
            ),
            (),
        )
        .await
        .unwrap();
    let store = svc.store();
    let renamed = || NodeUpdate::new().with_content("renamed".to_string());
    assert!(store
        .update_node(
            &ids[0],
            renamed(),
            None,
            nodespace_core::db::WriteContext::default()
        )
        .await
        .is_err());
    assert!(store
        .update_node_with_version_check(
            &ids[1],
            versions[1],
            renamed(),
            None,
            nodespace_core::db::WriteContext::default(),
            None
        )
        .await
        .is_err());
    let properties = get(&svc, &ids[2]).await.properties;
    assert!(store
        .switch_node_type_atomic(&ids[2], "tool-cli", properties, None)
        .await
        .is_err());
    for ((id, version), path) in
        ids.iter()
            .zip(&versions)
            .zip(["update", "versioned update", "type switch"])
    {
        // The write landed: the row moved on.
        let mut rows = db
            .connect()
            .unwrap()
            .query(
                "SELECT version FROM node WHERE id = ?1",
                libsql::params![id.clone()],
            )
            .await
            .unwrap();
        let stored: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(stored, version + 1, "{path}");
        assert!(!svc.tool_approval_recorded(id), "{path}");
    }
}

/// A change to a schema can reach any tool of the database (a renamed field
/// is rewritten in every node of its type without a node event; a type can
/// change what it extends): it ends every approval of the database.
#[tokio::test]
async fn a_schema_change_ends_every_approval_of_the_database() {
    let (svc, _tmp) = test_service().await;
    seed_remote_subtype(&svc).await;
    let id = create(
        &svc,
        "tool-rest",
        "lookup",
        remote_properties("tool-rest", true),
    )
    .await
    .unwrap();
    approve_here(&svc, &id).await;
    svc.rename_schema_field(REMOTE, "endpoint", "address")
        .await
        .unwrap();
    assert!(!svc.tool_approval_recorded(&id), "a field rename");
    approve_here(&svc, &id).await;
    nodespace_core::schema::handle_update_schema(
        &svc,
        json!({ "schema_id": REMOTE, "add_fields": [{ "name": "extra", "type": "text" }] }),
    )
    .await
    .unwrap();
    assert!(!svc.tool_approval_recorded(&id), "a schema update");
    approve_here(&svc, &id).await;
    nodespace_core::schema::handle_update_schema(
        &svc,
        json!({ "schema_id": REMOTE, "rename_fields": [{ "from": "extra", "to": "more" }] }),
    )
    .await
    .unwrap();
    assert!(
        !svc.tool_approval_recorded(&id),
        "a field rename in an update"
    );
    approve_here(&svc, &id).await;
    handle_create_schema(
        &svc,
        json!({ "name": "Other Tool", "extends": "tool", "fields": [] }),
    )
    .await
    .unwrap();
    assert!(!svc.tool_approval_recorded(&id), "a new schema");
    approve_here(&svc, &id).await;
    nodespace_core::schema::handle_update_schema(
        &svc,
        json!({ "schema_id": REMOTE, "extends": "other-tool" }),
    )
    .await
    .unwrap();
    assert!(!svc.tool_approval_recorded(&id), "what a type extends");
    // A schema written through the store itself (reported by its notifier).
    approve_here(&svc, &id).await;
    let schema = get(&svc, REMOTE).await;
    svc.store()
        .update_node(
            REMOTE,
            NodeUpdate::new().with_properties(schema.properties.clone()),
            None,
            nodespace_core::db::WriteContext::default(),
        )
        .await
        .unwrap();
    assert!(
        !svc.tool_approval_recorded(&id),
        "a schema written by the store"
    );
}

/// Turning a tool off ends its approval: every write that leaves `enabled`
/// anything but `true` removes `approved_binding`, so writing `enabled: true`
/// again brings no approval back with it.
#[tokio::test]
async fn turning_a_tool_off_removes_its_approval_on_every_write_path() {
    let (svc, _tmp) = test_service().await;
    let tag = "0123456789abcdef".repeat(4);
    let approval = |svc: Arc<NodeService>, id: String| async move {
        get(&svc, &id).await.properties["tool"]
            .get("approved_binding")
            .cloned()
    };
    for kind in REMOTE_KINDS {
        let mut approved = remote_properties(kind, true);
        approved["approved_binding"] = json!(tag);
        let id = create(&svc, kind, "lookup", approved.clone())
            .await
            .unwrap();
        // A write that leaves the tool on keeps the approval.
        let node = get(&svc, &id).await;
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "description": "Reworded" })),
        )
        .await
        .unwrap();
        assert_eq!(
            approval(svc.clone(), id.clone()).await,
            Some(json!(tag)),
            "{kind}"
        );

        // Off through a versioned update, then on again: no approval.
        let node = get(&svc, &id).await;
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "enabled": false })),
        )
        .await
        .unwrap();
        assert_eq!(approval(svc.clone(), id.clone()).await, None, "{kind}");
        let node = get(&svc, &id).await;
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "enabled": true })),
        )
        .await
        .unwrap();
        assert_eq!(approval(svc.clone(), id.clone()).await, None, "{kind}");

        // Through a batch update and an unversioned update.
        let id = create(&svc, kind, "batch", approved.clone()).await.unwrap();
        svc.bulk_update(vec![(
            id.clone(),
            NodeUpdate::new().with_properties(json!({ "enabled": false })),
        )])
        .await
        .unwrap();
        assert_eq!(approval(svc.clone(), id.clone()).await, None, "{kind}");
        let id = create(&svc, kind, "unversioned", approved.clone())
            .await
            .unwrap();
        svc.update_node_unchecked(
            &id,
            NodeUpdate::new().with_properties(json!({ "enabled": false })),
        )
        .await
        .unwrap();
        assert_eq!(approval(svc.clone(), id.clone()).await, None, "{kind}");

        // A tool created off holds no approval either.
        let mut off = approved.clone();
        off["enabled"] = json!(false);
        let id = create(&svc, kind, "created_off", off).await.unwrap();
        assert_eq!(approval(svc.clone(), id.clone()).await, None, "{kind}");
    }
}

#[tokio::test]
async fn a_remote_tool_takes_only_its_declared_fields() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        let mut properties = remote_properties(kind, false);
        properties["shell"] = json!(true);
        let error = message(create(&svc, kind, "lookup", properties).await);
        assert!(error.contains("shell"), "{kind}: {error}");
    }
    // A field of another kind is undeclared here.
    let mut properties = remote_properties("tool-cli", false);
    properties["method"] = json!("GET");
    let error = message(create(&svc, "tool-cli", "lookup", properties).await);
    assert!(error.contains("method"), "{error}");
}

#[tokio::test]
async fn a_remote_tool_with_an_invalid_binding_is_refused_naming_the_rule() {
    let (svc, _tmp) = test_service().await;
    let cases: [(&str, serde_json::Value, &str); 13] = [
        (
            "tool-mcp",
            json!({"transport":"http","command":"server"}),
            "stdio transport",
        ),
        (
            "tool-mcp",
            json!({"transport":"http","url":"http://example.com/x"}),
            "https",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"https://{host}/x"}),
            "host",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"http://example.com/x"}),
            "https",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"https://a.test/{nope}"}),
            "nope",
        ),
        ("tool-cli", json!({"program":"git status"}), "program"),
        (
            "tool-cli",
            json!({"program":"git","args":["--n={q}"]}),
            "placeholder",
        ),
        (
            "tool-cli",
            json!({"program":"git","timeout_seconds":301}),
            "300",
        ),
        (
            "tool-cli",
            json!({"program":"git","max_output_bytes":10_485_761u64}),
            "max_output_bytes",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"https:{h}/p"}),
            "url_template",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"https:/{h}/p"}),
            "url_template",
        ),
        (
            "tool-rest",
            json!({"method":"GET","url_template":"https:///{a}"}),
            "url_template",
        ),
        ("tool-cli", json!({"program":"../../bin/sh"}), "program"),
    ];
    for (kind, binding, rule) in cases {
        let mut properties = binding.clone();
        properties["parameter_schema"] = closed_schema();
        properties["enabled"] = json!(true);
        let error = message(create(&svc, kind, "lookup", properties).await);
        assert!(error.contains(rule), "{kind} {binding}: {error}");
    }
}

#[tokio::test]
async fn enabling_needs_a_complete_binding_and_a_draft_may_be_incomplete() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        // A draft with nothing set is stored.
        let id = create(&svc, kind, "draft", json!({ "description": "Soon" }))
            .await
            .unwrap_or_else(|e| panic!("{kind} draft: {e}"));
        // Enabling it while incomplete is refused.
        let version = get(&svc, &id).await.version;
        let error = message(
            svc.update_node(
                &id,
                version,
                NodeUpdate::new().with_properties(json!({ "enabled": true })),
            )
            .await,
        );
        assert!(
            error.contains("before the tool is enabled"),
            "{kind}: {error}"
        );
        assert_eq!(get(&svc, &id).await.properties["tool"]["enabled"], false);
        // Completing it and enabling in one write is accepted.
        let mut patch = remote_properties(kind, true);
        patch["description"] = json!("Soon");
        svc.update_node(&id, version, NodeUpdate::new().with_properties(patch))
            .await
            .unwrap_or_else(|e| panic!("{kind} complete: {e}"));
        assert_eq!(get(&svc, &id).await.properties["tool"]["enabled"], true);
    }
}

#[tokio::test]
async fn a_literal_credential_is_refused_at_create_and_at_update() {
    let (svc, _tmp) = test_service().await;
    let literal = json!({ "scheme": "bearer", "secret": "ghp_0123456789abcdefghij" });
    let mut properties = remote_properties("tool-rest", false);
    properties["auth"] = literal.clone();
    let error = message(create(&svc, "tool-rest", "lookup", properties).await);
    assert!(error.contains("credential reference"), "{error}");

    let id = create(
        &svc,
        "tool-rest",
        "lookup",
        remote_properties("tool-rest", false),
    )
    .await
    .unwrap();
    let version = get(&svc, &id).await.version;
    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_properties(json!({ "auth": literal })),
        )
        .await,
    );
    assert!(error.contains("credential reference"), "{error}");

    // A reference is accepted, in an environment entry too.
    create(
        &svc,
        "tool-cli",
        "with_env",
        json!({ "program": "git", "env": [{ "name": "TOKEN", "secret": "env:GIT_TOKEN" }] }),
    )
    .await
    .unwrap();
    let error = message(
        create(
            &svc,
            "tool-cli",
            "with_literal_env",
            json!({ "program": "git", "env": [{ "name": "TOKEN", "secret": "hunter2" }] }),
        )
        .await,
    );
    assert!(error.contains("credential reference"), "{error}");
}

#[tokio::test]
async fn a_remote_tools_parameter_schema_has_a_closed_root() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        for schema in [
            json!({ "type": "object", "properties": {} }),
            json!({ "properties": {}, "additionalProperties": false }),
            json!({ "type": "object", "additionalProperties": true }),
        ] {
            let mut properties = remote_properties(kind, false);
            properties["parameter_schema"] = schema.clone();
            let error = message(create(&svc, kind, "lookup", properties).await);
            assert!(
                error.contains("parameter_schema"),
                "{kind} {schema}: {error}"
            );
        }
    }
    // The base is unchanged: a native tool may leave its root open.
    native_tool(&svc).await;
}

#[tokio::test]
async fn a_remote_tools_name_is_an_identifier() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        for name in ["has space", "2fast", "dotted.name", &"a".repeat(65)] {
            let error = message(create(&svc, kind, name, remote_properties(kind, false)).await);
            assert!(error.contains("name"), "{kind} {name}: {error}");
        }
        create(&svc, kind, "good_name-1", remote_properties(kind, false))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_remote_tool_is_embedded_as_its_name_and_description_only() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        let id = create(&svc, kind, "lookup", remote_properties(kind, false))
            .await
            .unwrap();
        let node = get(&svc, &id).await;
        let chain = svc.store().type_chain(kind).await.unwrap();
        let text = nodespace_core::behaviors::NodeBehaviorRegistry::new()
            .resolve(&chain)
            .get_embeddable_content(&node)
            .expect("embedded");
        assert_eq!(text, "lookup\n\nLooks a thing up", "{kind}");
    }
}

#[tokio::test]
async fn a_user_subtype_of_a_remote_kind_keeps_its_rules() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Internal Rest",
            "extends": "tool-rest",
            "fields": [{ "name": "owner", "type": "text" }],
        }),
    )
    .await
    .expect("a user subtype of tool-rest");
    let ty = "internal-rest";
    let mut open = remote_properties("tool-rest", false);
    open["parameter_schema"] = json!({ "type": "object" });
    let error = message(create(&svc, ty, "lookup", open).await);
    assert!(error.contains("parameter_schema"), "{error}");
    let mut bad = remote_properties("tool-rest", false);
    bad["url_template"] = json!("http://example.com/{id}");
    let error = message(create(&svc, ty, "lookup", bad).await);
    assert!(error.contains("https"), "{error}");
    let id = create(&svc, ty, "lookup", remote_properties("tool-rest", true))
        .await
        .expect("a complete binding is accepted");
    let chain = svc.store().type_chain(ty).await.unwrap();
    assert_eq!(ToolOrigin::of(&chain), Some(ToolOrigin::External));
    assert_eq!(
        get(&svc, &id).await.properties["tool-rest"]["method"],
        "GET"
    );
}

#[tokio::test]
async fn retyping_between_remote_kinds_applies_the_new_kinds_rules() {
    let (svc, _tmp) = test_service().await;
    let id = create(
        &svc,
        "tool-cli",
        "lookup",
        json!({ "program": "git", "description": "d" }),
    )
    .await
    .unwrap();
    let version = get(&svc, &id).await.version;
    svc.update_node(
        &id,
        version,
        NodeUpdate::new().with_node_type("tool-rest".to_string()),
    )
    .await
    .expect("retyped");
    let node = get(&svc, &id).await;
    assert_eq!(node.node_type, "tool-rest");
    // The base's fields stay in the base's bucket.
    assert_eq!(node.properties["tool"]["description"], "d");
    // The node is now held to the REST binding, not the CLI one.
    let error = message(
        svc.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({ "enabled": true })),
        )
        .await,
    );
    assert!(error.contains("method"), "{error}");
}

#[tokio::test]
async fn the_trust_fields_are_kept_on_the_device() {
    let (svc, _tmp) = test_service().await;
    let schema = svc
        .get_schema_node("tool")
        .await
        .unwrap()
        .expect("tool schema");
    for name in ["enabled", "approved_binding"] {
        let field = schema.fields.iter().find(|f| f.name == name).unwrap();
        assert!(field.local_only, "{name} must be local_only");
    }
    // A tool with neither set is valid.
    create(&svc, "tool-cli", "lookup", json!({ "program": "git" }))
        .await
        .unwrap();
}

/// A tool that arrives through a path that applies no schema defaults (a bulk
/// create) may carry no `enabled` at all: absent means off, so an incomplete
/// binding is a draft and is accepted, and the stored node has no `enabled`.
#[tokio::test]
async fn an_absent_enabled_means_off() {
    let (svc, _tmp) = test_service().await;
    for kind in REMOTE_KINDS {
        let node = Node::new(
            kind.to_string(),
            "arrived".to_string(),
            json!({ "tool": { "description": "From elsewhere" } }),
        );
        let ids = svc
            .bulk_create(vec![node])
            .await
            .unwrap_or_else(|e| panic!("{kind}: {e}"));
        let stored = get(&svc, &ids[0]).await;
        let enabled = stored.properties["tool"].get("enabled");
        assert!(
            enabled.is_none_or(|v| v.is_null() || v == &json!(false)),
            "{kind}: {}",
            stored.properties
        );
        let enabled = enabled.and_then(|v| v.as_bool()).unwrap_or(false);
        let chain = svc.store().type_chain(kind).await.unwrap();
        assert!(!ToolOrigin::of(&chain).unwrap().is_offered(enabled));
    }
}

/// An invalid binding on an enabled tool is refused on every write path, not
/// only `create_node`: the batch creates, the trusted import and the batch
/// update hold the same rules.
#[tokio::test]
async fn an_invalid_binding_is_refused_on_every_write_path() {
    let (svc, _tmp) = test_service().await;
    let bad = || {
        json!({
            "tool": { "enabled": true, "parameter_schema": closed_schema() },
            "tool-rest": { "method": "GET", "url_template": "https:{h}/p" },
        })
    };
    let refused = |e: String| assert!(e.contains("url_template"), "{e}");

    refused(message(
        svc.bulk_create(vec![Node::new(
            "tool-rest".to_string(),
            "lookup".to_string(),
            bad(),
        )])
        .await,
    ));
    let row = |properties: serde_json::Value| {
        vec![(
            uuid::Uuid::new_v4().to_string(),
            "tool-rest".to_string(),
            "lookup".to_string(),
            None,
            1.0,
            properties,
        )]
    };
    refused(message(svc.bulk_create_hierarchy(row(bad())).await));
    refused(message(svc.bulk_create_hierarchy_trusted(row(bad())).await));

    // A draft is stored, and a batch update that enables it with a bad
    // template is refused.
    let id = create(&svc, "tool-rest", "draft", json!({ "description": "d" }))
        .await
        .unwrap();
    let error = message(
        svc.bulk_update(vec![(
            id.clone(),
            NodeUpdate::new().with_properties(json!({
                "enabled": true,
                "method": "GET",
                "url_template": "https:/{h}/p",
                "parameter_schema": closed_schema(),
            })),
        )])
        .await,
    );
    refused(error);
    assert_eq!(get(&svc, &id).await.properties["tool"]["enabled"], false);
}
