//! The model-facing descriptions of a type — the `EXISTING SCHEMAS` block of
//! the workspace context and the definition `create_schema` quotes back when
//! a type already exists — must describe a subtype across its ADR-078
//! `extends` chain. A `SchemaNode` carries only its own declarations, so a
//! description built from one alone tells the model a subtype lacks every
//! field and relationship it inherits, while `NodeService` reads and writes
//! the merged set.
//!
//! The `search_skills` counterpart needs a real embedding model and lives in
//! `find_skills_schema_metadata_test.rs`.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore, ops::context_ops::build_workspace_context, schema::handle_create_schema,
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

/// `ledger-invoice` declares `amount_due` and a `billed_to` relationship;
/// `annual-ledger-invoice` extends it, declaring only `fiscal_year`.
async fn create_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "ledger-customer",
            "fields": [{ "name": "name", "type": "text" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("customer schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "ledger-invoice",
            "fields": [{ "name": "amount_due", "type": "number" }],
            "relationships": [{
                "name": "billed_to",
                "targetType": "ledger-customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "ledger_invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "annual-ledger-invoice",
            "extends": "ledger-invoice",
            "fields": [{ "name": "fiscal_year", "type": "number" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

/// The prompt line for `type_id` within the rendered workspace context.
fn schema_line<'a>(prompt: &'a str, type_id: &str) -> &'a str {
    let prefix = format!("- {type_id} ");
    prompt
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no EXISTING SCHEMAS line for {type_id}:\n{prompt}"))
}

#[tokio::test]
async fn workspace_context_describes_a_subtype_with_its_inherited_declarations() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    // No embedding service: the subtype reaches `relevant_schemas` through the
    // lexical backstop for a type named outright in the query.
    let query = "add an annual ledger invoice for this year";
    let ctx = build_workspace_context(&svc, None, Some(query), None).await?;
    let prompt = ctx.format_for_prompt(8000);
    let line = schema_line(&prompt, "annual-ledger-invoice");

    assert!(line.contains("fiscal_year: number"), "own field: {line}");
    assert!(
        line.contains("amount_due: number"),
        "inherited field: {line}"
    );
    assert!(
        line.contains("~> billed_to"),
        "inherited relationship: {line}"
    );
    assert!(
        !line.contains("extends"),
        "the type-system `extends` row is not a traversable relationship: {line}"
    );
    Ok(())
}

#[tokio::test]
async fn create_schema_already_exists_quotes_the_inherited_definition() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "annual-ledger-invoice",
            "fields": [{ "name": "fiscal_year", "type": "number" }]
        }),
    )
    .await
    .expect_err("re-creating an existing schema must be rejected");
    let message = err.to_string();

    assert!(message.contains("amount_due: number"), "{message}");
    assert!(message.contains("~> billed_to"), "{message}");
    Ok(())
}
