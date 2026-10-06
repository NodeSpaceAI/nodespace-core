#![cfg(feature = "nlp")]
//! Integration coverage for the fix in this issue: `find_skills`'s
//! `schema_metadata` must carry field descriptions, relationship
//! descriptions, and the schema's own description-subtree content — not
//! silently drop them the way `EntityTypeDescriptor` did before.
//!
//! A fixture schema is authored with rich descriptions at all three levels
//! (field, relationship, schema-level markdown subtree), associated with a
//! skill linked to it by an `applies_to` edge, embedded with the real
//! embedding model, and retrieved through the real `find_skills`
//! semantic-search path — end to end, not a unit test of one internal helper
//! — so a regression that reintroduces the drop anywhere in the pipeline is
//! caught here.
//!
//! The same path decides *which* schemas a matched skill carries: its
//! `applies_to` targets and their subtypes, core schemas included, or the
//! unlinked fallback. The tests at the end cover each case.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::{Node, NodeUpdate, SkillFields, SKILL_APPLIES_TO},
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
async fn create_test_env() -> Result<(
    Arc<NodeService>,
    NodeEmbeddingService,
    Arc<SqliteStore>,
    TempDir,
)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);

    let node_service = NodeService::new(&mut store).await?;
    let nlp_engine = create_test_nlp_engine();
    let node_accessor: Arc<dyn NodeAccessor> = Arc::new(node_service.clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service =
        NodeEmbeddingService::new(nlp_engine, store.clone(), node_accessor, behaviors);

    Ok((Arc::new(node_service), embedding_service, store, temp_dir))
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
            "fields": [{ "name": "name", "type": "text" }]
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

/// Create an unlinked skill named `name`.
async fn seed_skill(service: &NodeService, name: &str, description: &str) -> Result<Node> {
    let mut node =
        SkillFields::new(description, &["create_node", "update_node"], 2).into_node(name);
    node.title = Some(name.to_string());
    service.create_node(node.clone()).await?;
    Ok(service
        .get_node(&node.id)
        .await?
        .expect("skill node should exist"))
}

/// Link `skill` to the schema it is about.
async fn link(service: &NodeService, skill: &Node, schema_id: &str) -> Result<()> {
    service
        .create_relationship(&skill.id, SKILL_APPLIES_TO, schema_id, json!({}))
        .await?;
    Ok(())
}

/// Create a skill linked to `invoice`, so `schema_metadata` deterministically
/// includes exactly the fixture schema regardless of the unlinked-fallback /
/// query-naming heuristics `find_skills` also has.
async fn seed_invoice_skill(service: &NodeService) -> Result<Node> {
    let skill = seed_skill(service, "Invoice Billing", SKILL_DESCRIPTION).await?;
    link(service, &skill, "invoice").await?;
    Ok(skill)
}

/// The type ids in the `schema_metadata` of `skill`'s entry in `output`.
fn carried_types(
    output: &nodespace_core::ops::skill_ops::FindSkillsOutput,
    skill: &Node,
) -> Vec<String> {
    output
        .skills
        .iter()
        .find(|s| s["id"] == json!(skill.id))
        .expect("the skill should be present in results")["schema_metadata"]
        .as_array()
        .expect("schema_metadata should be an array")
        .iter()
        .filter_map(|entry| entry["type_id"].as_str().map(str::to_string))
        .collect()
}

/// Whether `output` marks `skill`'s `schema_metadata` as its linked set.
fn schemas_linked(output: &nodespace_core::ops::skill_ops::FindSkillsOutput, skill: &Node) -> bool {
    output
        .skills
        .iter()
        .find(|s| s["id"] == json!(skill.id))
        .expect("the skill should be present in results")["schemas_linked"]
        .as_bool()
        .expect("schemas_linked should be a boolean")
}

#[tokio::test]
async fn find_skills_schema_metadata_carries_field_relationship_and_schema_descriptions(
) -> Result<()> {
    let (node_service, embedding_service, _store, _temp_dir) = create_test_env().await?;

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
        .expect("schema_metadata should include the invoice schema (linked via applies_to)");

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

/// A skill whose properties no longer decode is left out of the results
/// without failing the search: one malformed node must not take skill
/// retrieval down for every turn. `SkillNodeBehavior::validate` blocks the
/// shape on the service write path, so it is planted with a raw store write.
#[tokio::test]
async fn find_skills_skips_a_malformed_skill_and_keeps_the_rest() -> Result<()> {
    let (node_service, embedding_service, store, _temp_dir) = create_test_env().await?;
    create_fixture_schemas(&node_service).await?;

    let valid = seed_invoice_skill(&node_service).await?;
    let mut malformed = SkillFields::new(
        "Record a customer's payment against an invoice they owe.",
        &["update_node"],
        2,
    )
    .into_node("Invoice Payments");
    malformed.title = Some("Invoice Payments".to_string());
    node_service.create_node(malformed.clone()).await?;
    embedding_service.embed_root_node(&valid.id).await?;
    embedding_service.embed_root_node(&malformed.id).await?;

    let embedding_service = Arc::new(embedding_service);
    let search = || {
        find_skills(
            &embedding_service,
            &node_service,
            FindSkillsInput {
                query: MATCHING_QUERY.to_string(),
                limit: Some(3),
            },
        )
    };
    let ids = |output: &nodespace_core::ops::skill_ops::FindSkillsOutput| -> Vec<String> {
        output
            .skills
            .iter()
            .filter_map(|s| s["id"].as_str().map(str::to_string))
            .collect()
    };

    // Precondition: both skills reach the result set while well-formed, so
    // the later absence is the skip, not a retrieval miss.
    let before = ids(&search().await.expect("find_skills should succeed"));
    assert!(before.contains(&valid.id), "{before:?}");
    assert!(before.contains(&malformed.id), "{before:?}");

    store
        .update_node(
            &malformed.id,
            NodeUpdate {
                properties: Some(json!({ "skill": { "tool_whitelist": "update_node" } })),
                ..Default::default()
            },
            None,
        )
        .await?;

    let after = ids(&search()
        .await
        .expect("a malformed skill must not fail the search"));
    assert!(after.contains(&valid.id), "{after:?}");
    assert!(!after.contains(&malformed.id), "{after:?}");
    Ok(())
}

/// A subtype's `schema_metadata` entry — on both a skill scoped to it and its
/// own `kind: "schema"` result — carries what it inherits across its ADR-078
/// `extends` chain, not only what it declares itself.
#[tokio::test]
async fn find_skills_schema_metadata_includes_a_subtypes_inherited_declarations() -> Result<()> {
    let (node_service, embedding_service, _store, _temp_dir) = create_test_env().await?;

    create_fixture_schemas(&node_service).await?;
    handle_create_schema(
        &node_service,
        json!({
            "name": "retainer-invoice",
            "extends": "invoice",
            "fields": [{ "name": "retainer_months", "type": "number" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;

    let skill = seed_skill(
        &node_service,
        "Retainer Billing",
        "Bill a customer on a monthly retainer — create a retainer invoice record.",
    )
    .await?;
    link(&node_service, &skill, "retainer-invoice").await?;
    embedding_service.embed_root_node(&skill.id).await?;

    // Names the subtype outright, so its `kind: "schema"` result arrives via
    // the lexical backstop without waiting on a schema embedding.
    let output = find_skills(
        &Arc::new(embedding_service),
        &node_service,
        FindSkillsInput {
            query: "bill this customer a retainer invoice".to_string(),
            limit: Some(5),
        },
    )
    .await
    .expect("find_skills should succeed");

    let assert_inherited = |entry: &serde_json::Value, via: &str| {
        let names = |key: &str| -> Vec<String> {
            entry[key]
                .as_array()
                .unwrap_or_else(|| panic!("{via}: `{key}` should be an array: {entry}"))
                .iter()
                .filter_map(|v| v["name"].as_str().map(str::to_string))
                .collect()
        };
        let fields = names("fields");
        assert!(
            fields.contains(&"retainer_months".to_string()),
            "{via}: {fields:?}"
        );
        assert!(
            fields.contains(&"amount_due".to_string()),
            "{via}: {fields:?}"
        );
        assert_eq!(names("relationships"), ["billed_to"], "{via}: {entry}");
    };

    let skill_result = output
        .skills
        .iter()
        .find(|s| s["id"] == json!(skill.id))
        .expect("the seeded skill should be present in results");
    let scoped = skill_result["schema_metadata"]
        .as_array()
        .expect("schema_metadata should be an array")
        .iter()
        .find(|e| e["type_id"] == json!("retainer-invoice"))
        .expect("the skill's schema_metadata should include the subtype");
    assert_inherited(scoped, "skill result");

    let schema_result = output
        .skills
        .iter()
        .find(|s| s["kind"] == json!("schema") && s["id"] == json!("retainer-invoice"))
        .expect("the named subtype should come back as a schema result");
    assert_inherited(&schema_result["schema_metadata"][0], "schema result");

    Ok(())
}

/// A skill linked to a base type carries that type and every type extending
/// it: a subtype is its base type, so guidance about invoices is guidance
/// about retainer invoices. Nothing else comes along, however many other
/// custom types the workspace holds.
#[tokio::test]
async fn find_skills_carries_a_linked_type_and_its_subtypes() -> Result<()> {
    let (node_service, embedding_service, _store, _temp_dir) = create_test_env().await?;

    create_fixture_schemas(&node_service).await?;
    handle_create_schema(
        &node_service,
        json!({
            "name": "retainer-invoice",
            "extends": "invoice",
            "fields": [{ "name": "retainer_months", "type": "number" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;

    let skill = seed_invoice_skill(&node_service).await?;
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

    let mut carried = carried_types(&output, &skill);
    carried.sort();
    assert_eq!(
        carried,
        ["invoice", "retainer-invoice"],
        "a linked skill carries its target and the target's subtypes, and no other type"
    );
    assert!(
        schemas_linked(&output, &skill),
        "a skill carrying its linked schemas says so"
    );
    Ok(())
}

/// A link to a core schema is honoured: the skill carries `task`, which the
/// unlinked fallback never offers, along with the custom types extending it.
#[tokio::test]
async fn find_skills_carries_a_linked_core_type() -> Result<()> {
    let (node_service, embedding_service, _store, _temp_dir) = create_test_env().await?;

    create_fixture_schemas(&node_service).await?;
    handle_create_schema(
        &node_service,
        json!({
            "name": "Chore",
            "extends": "task",
            "fields": [{ "name": "room", "type": "text" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("chore schema: {e}"))?;

    let skill = seed_skill(
        &node_service,
        "Closing Out Work",
        "Close out a piece of work by recording how it was checked, then marking it done.",
    )
    .await?;
    link(&node_service, &skill, "task").await?;
    embedding_service.embed_root_node(&skill.id).await?;

    let output = find_skills(
        &Arc::new(embedding_service),
        &node_service,
        FindSkillsInput {
            query: "close out this work and mark it done".to_string(),
            limit: Some(3),
        },
    )
    .await
    .expect("find_skills should succeed");

    let mut carried = carried_types(&output, &skill);
    carried.sort();
    assert_eq!(carried, ["chore", "task"]);
    Ok(())
}

/// A skill with no links keeps the fallback: the one custom type the query
/// names, otherwise the workspace's custom types. Never a core type.
#[tokio::test]
async fn find_skills_falls_back_for_an_unlinked_skill() -> Result<()> {
    let (node_service, embedding_service, _store, _temp_dir) = create_test_env().await?;

    create_fixture_schemas(&node_service).await?;
    let skill = seed_skill(&node_service, "Invoice Billing", SKILL_DESCRIPTION).await?;
    embedding_service.embed_root_node(&skill.id).await?;
    let embedding_service = Arc::new(embedding_service);
    let search = |query: &str| {
        find_skills(
            &embedding_service,
            &node_service,
            FindSkillsInput {
                query: query.to_string(),
                limit: Some(3),
            },
        )
    };

    // The query names `invoice` and no other custom type.
    let named = search("add an invoice for this month's work")
        .await
        .unwrap();
    assert_eq!(carried_types(&named, &skill), ["invoice"]);
    // The same single type a link to `invoice` would carry: the types alone
    // cannot tell a fallback from a link.
    assert!(!schemas_linked(&named, &skill));

    // The query names no type: every custom type, and no core one.
    let unnamed = search("bill them for this month's work").await.unwrap();
    let mut carried = carried_types(&unnamed, &skill);
    carried.sort();
    assert_eq!(carried, ["customer", "invoice"]);
    assert!(!schemas_linked(&unnamed, &skill));
    Ok(())
}
