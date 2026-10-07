//! A type id is kebab-case for every type, so the id `create_schema` derives
//! from "Customer Profile" is `customer-profile`. These tests follow such a
//! type through everything that reads its id: where it is stored, the bucket
//! its fields live in, the generic create path, a query, a title template, a
//! unique field's conflict check, relationships, `extends`, and a play.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::ops::query_ops::{execute_query_nodes, ExecuteQueryInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use nodespace_core::PlaybookEngine;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create_schema(svc: &Arc<NodeService>, params: Value) -> Result<Value> {
    let result = handle_create_schema(svc, params)
        .await
        .map_err(|e| anyhow::anyhow!("create_schema: {e}"))?;
    Ok(serde_json::to_value(result)?)
}

fn query(target_type: &str, filters: Value, sorting: Value) -> ExecuteQueryInput {
    serde_json::from_value(json!({
        "target_type": target_type,
        "filters": filters,
        "sorting": sorting,
    }))
    .expect("query input")
}

async fn profile_schema(svc: &Arc<NodeService>) -> Result<()> {
    create_schema(
        svc,
        json!({
            "name": "Customer Profile",
            "fields": [
                { "name": "full_name", "type": "text", "unique": true },
                { "name": "tier", "type": "number" }
            ],
            "title_template": "{full_name} (tier {tier})"
        }),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn create_schema_stores_the_schema_under_the_kebab_case_id() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    let created = create_schema(&svc, json!({ "name": "Customer Profile", "fields": [] })).await?;
    assert_eq!(created["schemaId"], "customer-profile");

    assert!(svc.get_schema_node("customer-profile").await?.is_some());
    assert!(svc.get_schema_node("customer_profile").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn nodes_of_the_type_carry_the_id_and_keep_their_fields_in_its_bucket() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    profile_schema(&svc).await?;

    let id = svc
        .create_node(Node::new(
            "customer-profile".to_string(),
            String::new(),
            json!({ "customer-profile": { "full_name": "Ada", "tier": 2 } }),
        ))
        .await?;
    let node = svc.get_node(&id).await?.expect("the node");

    assert_eq!(node.node_type, "customer-profile");
    assert_eq!(node.properties["customer-profile"]["full_name"], "Ada");
    // The title template reads the fields of that bucket.
    assert_eq!(node.title.as_deref(), Some("Ada (tier 2)"));
    Ok(())
}

#[tokio::test]
async fn flat_properties_of_the_type_are_bucketed_under_its_id() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    profile_schema(&svc).await?;

    let id = svc
        .create_node(Node::new(
            "customer-profile".to_string(),
            String::new(),
            json!({ "full_name": "Grace", "tier": 1 }),
        ))
        .await?;
    let node = svc.get_node(&id).await?.expect("the node");
    assert_eq!(node.properties["customer-profile"]["tier"], 1);
    assert_eq!(node.title.as_deref(), Some("Grace (tier 1)"));
    Ok(())
}

#[tokio::test]
async fn a_query_filters_and_sorts_on_a_field_of_the_type() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    profile_schema(&svc).await?;
    assert!(svc.get_schema_node("customer-profile").await?.is_some());
    for (name, tier) in [("A", 3), ("B", 1), ("C", 2)] {
        svc.create_node(Node::new(
            "customer-profile".to_string(),
            String::new(),
            json!({ "full_name": name, "tier": tier }),
        ))
        .await?;
    }

    let filtered = execute_query_nodes(
        &svc,
        query(
            "customer-profile",
            json!([{ "type": "property", "operator": "gte", "property": "tier", "value": 2 }]),
            json!([{ "field": "tier", "direction": "asc" }]),
        ),
    )
    .await?;
    let names: Vec<_> = filtered
        .iter()
        .map(|n| n.properties["customer-profile"]["full_name"].clone())
        .collect();
    assert_eq!(names, [json!("C"), json!("A")]);

    // A query over every type reads the same bucket from each row's own type.
    let across = execute_query_nodes(
        &svc,
        query(
            "*",
            json!([]),
            json!([{ "field": "tier", "direction": "desc" }]),
        ),
    )
    .await?;
    let first = across
        .iter()
        .find(|n| n.node_type == "customer-profile")
        .expect("a profile");
    assert_eq!(first.properties["customer-profile"]["full_name"], "A");
    Ok(())
}

#[tokio::test]
async fn a_unique_field_finds_its_conflict() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    profile_schema(&svc).await?;
    let existing = svc
        .create_node(Node::new(
            "customer-profile".to_string(),
            String::new(),
            json!({ "full_name": "Ada", "tier": 1 }),
        ))
        .await?;

    let duplicate = svc
        .find_duplicate_for("customer-profile", "full_name", "Ada", None)
        .await?;
    assert_eq!(duplicate.map(|n| n.id), Some(existing.clone()));
    assert!(svc
        .find_duplicate_for("customer-profile", "full_name", "Ada", Some(&existing))
        .await?
        .is_none());
    Ok(())
}

#[tokio::test]
async fn relationships_target_and_extend_types_by_their_kebab_case_ids() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    profile_schema(&svc).await?;

    // A relationship whose targetType is the type, a self-referential one,
    // and an `extends` of a core type.
    create_schema(
        &svc,
        json!({
            "name": "Purchase Order",
            "fields": [],
            "relationships": [
                { "name": "ordered_by", "targetType": "customer-profile", "direction": "out",
                  "cardinality": "one", "reverseName": "orders", "reverseCardinality": "many" },
                { "name": "replaces", "targetType": "purchase-order", "direction": "out",
                  "cardinality": "one", "reverseName": "replaced_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await?;
    create_schema(
        &svc,
        json!({ "name": "Work Item", "extends": "task", "fields": [] }),
    )
    .await?;

    let customer = svc
        .create_node(Node::new(
            "customer-profile".to_string(),
            String::new(),
            json!({ "full_name": "Ada", "tier": 1 }),
        ))
        .await?;
    let first = svc
        .create_node(Node::new(
            "purchase-order".to_string(),
            "PO-1".to_string(),
            json!({}),
        ))
        .await?;
    let second = svc
        .create_node(Node::new(
            "purchase-order".to_string(),
            "PO-2".to_string(),
            json!({}),
        ))
        .await?;
    svc.create_relationship(&first, "ordered_by", &customer, json!({}))
        .await?;
    svc.create_relationship(&second, "replaces", &first, json!({}))
        .await?;

    let work_item = svc
        .create_node(Node::new(
            "work-item".to_string(),
            "Ship it".to_string(),
            json!({}),
        ))
        .await?;
    // The subtype is a task: a query for tasks returns it.
    let tasks = execute_query_nodes(&svc, query("task", json!([]), json!([]))).await?;
    assert!(tasks.iter().any(|n| n.id == work_item));
    Ok(())
}

#[tokio::test]
async fn a_schema_node_must_have_the_derived_id_on_every_create_path() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    let schema = |id: &str| {
        Node::new_with_id(
            id.to_string(),
            "schema".to_string(),
            "Customer Profile".to_string(),
            json!({ "isCore": false, "schemaVersion": 1, "fields": [], "relationships": [] }),
        )
    };

    for refused in [
        "customer_profile",
        "Customer Profile",
        "customer--profile",
        "-x",
        "6B693B96-4FAD-5846-BCA3-545B1A7E53A7",
    ] {
        let error = svc
            .create_node(schema(refused))
            .await
            .expect_err(refused)
            .to_string();
        assert!(error.contains("kebab-case"), "{refused}: {error}");
    }
    svc.create_node(schema("customer-profile")).await?;
    assert!(svc.get_schema_node("customer-profile").await?.is_some());
    Ok(())
}

#[tokio::test]
async fn a_name_that_derives_a_core_types_id_is_refused_naming_the_conflict() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    let error = handle_create_schema(&svc, json!({ "name": "Code Block", "fields": [] }))
        .await
        .expect_err("`code-block` is a core type")
        .to_string();
    assert!(
        error.contains("code-block") && error.contains("core type"),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_name_that_derives_a_date_shaped_id_is_refused_and_leaves_nothing_behind() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    for name in ["2026-10-06", "2026_10_06", "2026 10 06"] {
        let error = handle_create_schema(&svc, json!({ "name": name, "fields": [] }))
            .await
            .expect_err(name)
            .to_string();
        assert!(
            error.contains("cannot be a schema name") && error.contains("date"),
            "{name}: {error}"
        );
    }
    // All three names derive the same id, so one lookup covers them.
    assert!(svc.get_schema_node("2026-10-06").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_name_that_merely_contains_digits_is_accepted() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    for (name, id) in [("Invoice 2026", "invoice-2026"), ("Q4-2026", "q4-2026")] {
        create_schema(&svc, json!({ "name": name, "fields": [] })).await?;
        assert!(svc.get_schema_node(id).await?.is_some(), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn a_play_selects_the_type_and_a_condition_reads_its_fields() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    create_schema(
        &svc,
        json!({
            "name": "Support Ticket",
            "fields": [{ "name": "state", "type": "text" }]
        }),
    )
    .await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(&svc)));
    svc.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.start(shutdown_rx).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    svc.create_node(Node::new(
        "play".to_string(),
        "close-new-tickets".to_string(),
        json!({ "rules": [{
            "name": "close",
            "description": "Close new tickets",
            "trigger": { "type": "graph_event", "on": "node_created",
                         "select": { "target_type": "support-ticket" } },
            "conditions": [{ "expr": "node.state == 'new'", "description": "is new" }],
            "actions": [{
                "description": "close it",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "state": "closed" } }
            }]
        }] }),
    ))
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let id = svc
        .create_node(Node::new(
            "support-ticket".to_string(),
            "Printer".to_string(),
            json!({ "state": "new" }),
        ))
        .await?;

    let mut closed = false;
    for _ in 0..80 {
        let node = svc.get_node(&id).await?.expect("the ticket");
        if node.properties["support-ticket"]["state"] == "closed" {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        closed,
        "the play must fire on a node of the hyphenated type"
    );

    let _ = shutdown_tx.send(true);
    let _ = task.await;
    Ok(())
}
