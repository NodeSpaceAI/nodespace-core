#![cfg(feature = "nlp")]
//! Integration coverage for the fix in this issue: `find_skills`'s
//! `schema_metadata` must carry field descriptions, relationship
//! descriptions, and the schema's own description-subtree content — not
//! silently drop them the way `EntityTypeDescriptor` did before.
//!
//! A fixture schema is authored with rich descriptions at all three levels
//! (field, relationship, schema-level markdown subtree), associated with a
//! skill scoped to it via `node_types`, embedded with the real embedding
//! model, and retrieved through the real `find_skills` semantic-search path
//! — end to end, not a unit test of one internal helper — so a regression
//! that reintroduces the drop anywhere in the pipeline is caught here.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::skill_ops::{find_skills, FindSkillsInput},
    schema::handle_create_schema,
    services::{embedding_service::NodeEmbeddingService, NodeAccessor, NodeService},
};
use nodespace_nlp_engine::{EmbeddingConfig as NlpConfig, EmbeddingService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

fn create_test_nlp_engine() -> Arc<EmbeddingService> {
    let config = NlpConfig::default();
    let mut service = EmbeddingService::new(config).unwrap();
    service
        .initialize()
        .expect("embedding model should load from ~/.nodespace/models/");
    Arc::new(service)
}

/// Shared-store `NodeService` + `NodeEmbeddingService` pair, mirroring
/// `embedding_service_test.rs`'s `create_unified_test_env` helper.
async fn create_test_env() -> Result<(Arc<NodeService>, NodeEmbeddingService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);

    let node_service = NodeService::new(&mut store).await?;
    let nlp_engine = create_test_nlp_engine();
    let node_accessor: Arc<dyn NodeAccessor> = Arc::new(node_service.clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service =
        NodeEmbeddingService::new(nlp_engine, store.clone(), node_accessor, behaviors);

    Ok((Arc::new(node_service), embedding_service, temp_dir))
}

const FIELD_DESCRIPTION: &str = "The outstanding balance the customer still owes, in the \
    invoice's currency. Zero once the invoice is fully paid.";
const RELATIONSHIP_DESCRIPTION: &str = "The customer responsible for paying this invoice — use \
    this instead of a generic mention when recording who owes the money.";
const SCHEMA_DESCRIPTION_MARKER: &str = "goods or services rendered";

const SKILL_DESCRIPTION: &str =
    "Bill a customer for an invoice — create or update an invoice record and link it to the \
     customer who owes payment.";
const MATCHING_QUERY: &str = "How do I bill a customer for an invoice?";

/// Create the `customer` (relationship target) and `invoice` (fixture under
/// test) schemas, the latter carrying a field description, a relationship
/// description, and a schema-level markdown description.
async fn create_fixture_schemas(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Customer",
            "fields": [{ "name": "name", "type": "string" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("customer schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "Invoice",
            "description": format!(
                "Tracks money owed by a customer for {SCHEMA_DESCRIPTION_MARKER}. \
                 Link every invoice to the customer who owes the balance via billed_to."
            ),
            "fields": [
                { "name": "amount_due", "type": "number", "description": FIELD_DESCRIPTION }
            ],
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many",
                "description": RELATIONSHIP_DESCRIPTION
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("invoice schema: {e}"))?;

    Ok(())
}

/// Create a skill scoped to `invoice` via `node_types`, so `schema_metadata`
/// deterministically includes exactly the fixture schema regardless of the
/// unscoped-fallback / query-naming heuristics `find_skills` also has.
async fn seed_invoice_skill(service: &NodeService) -> Result<Node> {
    let mut node = Node::new(
        "skill".to_string(),
        "Invoice Billing".to_string(),
        json!({
            "description": SKILL_DESCRIPTION,
            "tool_whitelist": ["create_node", "update_node"],
            "node_types": ["invoice"],
            "max_iterations": 2,
        }),
    );
    node.title = Some("Invoice Billing".to_string());
    service.create_node(node.clone()).await?;
    Ok(service
        .get_node(&node.id)
        .await?
        .expect("skill node should exist"))
}

#[tokio::test]
async fn find_skills_schema_metadata_carries_field_relationship_and_schema_descriptions(
) -> Result<()> {
    let (node_service, embedding_service, _temp_dir) = create_test_env().await?;

    create_fixture_schemas(&node_service).await?;
    let skill = seed_invoice_skill(&node_service).await?;

    // Real embedding, matching what production actually indexes — a mock
    // vector here would exercise KNN scoring, not this issue's actual
    // regression surface (the schema_metadata projection).
    embedding_service.embed_root_node(&skill.id).await?;

    let output = find_skills(
        &Arc::new(embedding_service),
        &node_service,
        FindSkillsInput {
            query: MATCHING_QUERY.to_string(),
            limit: Some(3),
        },
    )
    .await
    .expect("find_skills should succeed");

    assert!(
        output.total_results >= 1,
        "the seeded skill should match its own closely-related query"
    );

    let matched = output
        .skills
        .iter()
        .find(|s| s["id"] == json!(skill.id))
        .expect("the seeded skill should be present in results");

    let schema_metadata = matched["schema_metadata"]
        .as_array()
        .expect("schema_metadata should be an array");

    let invoice_entry = schema_metadata
        .iter()
        .find(|entry| entry["type_id"] == json!("invoice"))
        .expect("schema_metadata should include the invoice schema (scoped via node_types)");

    // 1. Field description reaches schema_metadata.
    let amount_due = invoice_entry["fields"]
        .as_array()
        .expect("fields should be an array")
        .iter()
        .find(|f| f["name"] == json!("amount_due"))
        .expect("amount_due field should be present");
    assert_eq!(
        amount_due["description"],
        json!(FIELD_DESCRIPTION),
        "field description must not be silently dropped from schema_metadata"
    );

    // 2. Relationship description reaches schema_metadata — previously
    // relationships weren't represented in EntityTypeDescriptor at all.
    let relationships = invoice_entry["relationships"]
        .as_array()
        .expect("relationships should be an array");
    let billed_to = relationships
        .iter()
        .find(|r| r["name"] == json!("billed_to"))
        .expect("billed_to relationship should be present");
    assert_eq!(billed_to["target_type"], json!("customer"));
    assert_eq!(billed_to["direction"], json!("out"));
    assert_eq!(billed_to["cardinality"], json!("one"));
    assert_eq!(billed_to["reverse_name"], json!("invoices"));
    assert_eq!(billed_to["reverse_cardinality"], json!("many"));
    assert_eq!(
        billed_to["description"],
        json!(RELATIONSHIP_DESCRIPTION),
        "relationship description must not be silently dropped from schema_metadata"
    );

    // 3. The schema's own description subtree (markdown, stored as child
    // nodes) reaches schema_metadata, reusing the shared subtree-render
    // utility rather than a third independent implementation.
    let schema_description = invoice_entry["description"]
        .as_str()
        .expect("schema-level description should be a string");
    assert!(
        schema_description.contains(SCHEMA_DESCRIPTION_MARKER),
        "schema-level description subtree content must reach schema_metadata, got: {:?}",
        schema_description
    );

    Ok(())
}
