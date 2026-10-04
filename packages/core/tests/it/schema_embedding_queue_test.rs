//! Every path that creates or changes a schema queues it for embedding.
//!
//! A schema is found by meaning: skill search and the workspace context both
//! run a semantic search over `schema` nodes. That only works for a schema
//! that has an embedding, and an embedding is only built for a node the queue
//! holds. `create_schema` writes in a caller-held transaction and core
//! seeding writes straight to the store, and both used to queue nothing, so
//! no schema was ever embedded and every one of those searches returned
//! nothing.
//!
//! These run without an embedding model: they assert the queue, which is what
//! each path owns. That the processor then turns a queued schema into a
//! vector a search finds is covered, with the real model, by
//! `find_skills_schema_discovery_test`.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::methodology::{install_playbook, playbook_by_id};
use nodespace_core::models::NewEmbedding;
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

/// Whether `id` is waiting in the embedding queue.
async fn is_queued(service: &NodeService, id: &str) -> Result<bool> {
    Ok(service
        .store()
        .get_embeddings(id)
        .await?
        .iter()
        .any(|e| e.stale))
}

/// The ids of the schema nodes that have no embedding row at all, queued or
/// built: the ones a semantic search can never find.
async fn schemas_never_queued(service: &NodeService) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for schema in service.get_all_schemas().await? {
        if service
            .store()
            .get_embeddings(&schema.envelope.id)
            .await?
            .is_empty()
        {
            missing.push(schema.envelope.id);
        }
    }
    Ok(missing)
}

/// Give `id` a built, non-stale embedding, as the processor would.
async fn embed_fresh(service: &NodeService, id: &str) -> Result<()> {
    service
        .store()
        .upsert_embeddings(
            id,
            vec![NewEmbedding::single_chunk(id, vec![0.5; 768], "h", 1, 1)],
        )
        .await?;
    assert!(!is_queued(service, id).await?, "{id} must start fresh");
    Ok(())
}

async fn create_invoice(service: &Arc<NodeService>) -> Result<String> {
    let created = handle_create_schema(
        service,
        json!({
            "name": "Invoice",
            "description": "A bill sent to a client for work done",
            "fields": [{ "name": "amount", "type": "number" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_schema rejected: {e}"))?;
    Ok(created["schemaId"]
        .as_str()
        .expect("schemaId in create_schema output")
        .to_string())
}

#[tokio::test]
async fn a_fresh_database_queues_every_core_schema() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let schemas = service.get_all_schemas().await?;
    assert!(!schemas.is_empty(), "core schemas must be seeded");
    for schema in &schemas {
        assert!(
            is_queued(&service, &schema.envelope.id).await?,
            "core schema `{}` was seeded without being queued for embedding",
            schema.envelope.id
        );
    }
    Ok(())
}

#[tokio::test]
async fn create_schema_queues_the_new_schema() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let schema_id = create_invoice(&service).await?;

    assert!(
        is_queued(&service, &schema_id).await?,
        "create_schema committed `{schema_id}` without queueing it for embedding"
    );
    Ok(())
}

/// The property the acceptance criterion names: no schema create path leaves
/// a schema with no embedding row. Core seeding, `create_schema` and a
/// Playbook install are the three paths; a fourth that skips the queue shows
/// up here as an id in the list.
#[tokio::test]
async fn no_schema_create_path_leaves_a_schema_unqueued() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    create_invoice(&service).await?;
    let playbook = playbook_by_id("linear").expect("the linear playbook ships");
    let report = install_playbook(&service, &playbook).await;
    assert!(report.success, "the linear playbook must install");

    let installed: Vec<String> = service
        .get_all_schemas()
        .await?
        .into_iter()
        .map(|s| s.envelope.id)
        .collect();
    for id in ["invoice", "issue", "cycle"] {
        assert!(installed.iter().any(|s| s == id), "`{id}` must exist");
    }

    let missing = schemas_never_queued(&service).await?;
    assert!(
        missing.is_empty(),
        "these schemas were created without being queued for embedding, so no semantic search \
         can find them: {missing:?}"
    );
    Ok(())
}

#[tokio::test]
async fn changing_a_schemas_fields_requeues_it() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let schema_id = create_invoice(&service).await?;
    embed_fresh(&service, &schema_id).await?;

    handle_update_schema(
        &service,
        json!({
            "schema_id": schema_id,
            "add_fields": [{ "name": "paid_on", "type": "date" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_schema rejected: {e}"))?;

    assert!(
        is_queued(&service, &schema_id).await?,
        "a field was added and the schema's embedding was left as it was"
    );
    Ok(())
}

#[tokio::test]
async fn changing_a_schemas_description_requeues_it() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let schema_id = create_invoice(&service).await?;
    embed_fresh(&service, &schema_id).await?;

    handle_update_schema(
        &service,
        json!({
            "schema_id": schema_id,
            "description": "A request for payment, with its due date and line items"
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_schema rejected: {e}"))?;

    assert!(
        is_queued(&service, &schema_id).await?,
        "the description changed and the schema's embedding was left as it was"
    );
    Ok(())
}

#[tokio::test]
async fn changing_a_schemas_relationships_requeues_it() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    handle_create_schema(
        &service,
        json!({ "name": "Client", "fields": [{ "name": "region", "type": "text" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_schema rejected: {e}"))?;
    let schema_id = create_invoice(&service).await?;
    embed_fresh(&service, &schema_id).await?;

    handle_update_schema(
        &service,
        json!({
            "schema_id": schema_id,
            "add_relationships": [{
                "name": "billed_to",
                "targetType": "client",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_schema rejected: {e}"))?;

    assert!(
        is_queued(&service, &schema_id).await?,
        "a relationship was added and the schema's embedding was left as it was"
    );
    Ok(())
}

/// A rejected update changed nothing, so it queues nothing: a stale embedding
/// is out of the index until it is rebuilt, and a call that failed must not
/// take the schema out of search.
#[tokio::test]
async fn a_rejected_update_leaves_the_embedding_as_it_was() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let schema_id = create_invoice(&service).await?;
    embed_fresh(&service, &schema_id).await?;

    let rejected = handle_update_schema(
        &service,
        json!({
            "schema_id": schema_id,
            "add_relationships": [{
                "name": "billed_to",
                "targetType": "no_such_type",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await;

    assert!(
        rejected.is_err(),
        "a relationship to a missing type is rejected"
    );
    assert!(
        !is_queued(&service, &schema_id).await?,
        "a rejected update re-queued the schema"
    );
    Ok(())
}

/// What a schema is embedded as: its name and its fields with their
/// descriptions, so a request phrased in the user's words reaches a type
/// whose name it never uses.
#[tokio::test]
async fn a_schema_is_embedded_as_its_name_and_its_fields() -> Result<()> {
    use nodespace_core::behaviors::{NodeBehavior, SchemaNodeBehavior};

    let (service, _tmp) = test_service().await?;
    let created = handle_create_schema(
        &service,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "amount", "type": "number", "description": "What the client owes" },
                { "name": "paid_on", "type": "date" }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_schema rejected: {e}"))?;
    let schema_id = created["schemaId"].as_str().expect("schemaId").to_string();
    let node = service
        .get_node(&schema_id)
        .await?
        .expect("the schema node exists");

    let text = SchemaNodeBehavior
        .get_embeddable_content(&node)
        .expect("a schema is embeddable");

    assert_eq!(
        text, "Invoice\nAmount: What the client owes\nPaid on",
        "name first, then one line per field: its label, and its description where it has one"
    );
    Ok(())
}
