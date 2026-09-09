//! Integration tests for schema creation and updates
//!
//! Tests exercise handle_create_schema and handle_update_schema end-to-end
//! against a real NodeService / SqliteStore, covering title_template
//! validation including the field-removal-while-template-exists edge case.

use super::*;
use crate::db::SqliteStore;
use crate::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let db_path = temp_dir.path().join("test.db");
    // NodeService::new takes &mut Arc<SqliteStore> to allow internal Arc replacement during init
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

// ============================================================================
// create_schema + title_template
// ============================================================================

#[tokio::test]
async fn test_create_schema_with_valid_title_template() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Customer",
            "fields": [
                { "name": "first_name", "type": "string", "protection": "user", "indexed": false },
                { "name": "last_name",  "type": "string", "protection": "user", "indexed": false }
            ],
            "title_template": "{first_name} {last_name}"
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "Valid title_template should succeed: {:?}",
        result
    );
    let val = result.expect("valid create_schema should return Ok");
    assert_eq!(val["schemaId"], "customer");
}

#[tokio::test]
async fn test_create_schema_title_template_undefined_field_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // title_template references "nonexistent" which is not in fields
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Customer",
            "fields": [
                { "name": "first_name", "type": "string", "protection": "user", "indexed": false }
            ],
            "title_template": "{nonexistent}"
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "title_template referencing undefined field should fail"
    );
    let err = result.unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("nonexistent"),
        "Error should name the bad field: {}",
        msg
    );
}

#[tokio::test]
async fn test_create_schema_without_title_template_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "amount", "type": "number", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "Schema without title_template should succeed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_create_schema_duplicate_rejection_carries_existing_definition() {
    let (svc, _tmp) = create_test_service().await;

    // Seed a "ticket" schema with a specific, deliberately distinctive set of
    // fields — these are the ONLY fields the rejection is allowed to name.
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [
                { "name": "title", "type": "text", "protection": "user", "indexed": false, "required": true },
                { "name": "owner", "type": "text", "protection": "user", "indexed": false },
                {
                    "name": "status",
                    "type": "enum",
                    "protection": "user",
                    "indexed": false,
                    "required": true,
                    "coreValues": [
                        { "value": "triage", "label": "Triage" },
                        { "value": "shipped", "label": "Shipped" }
                    ]
                }
            ]
        }),
    )
    .await
    .expect("seed schema creation should succeed");

    // A second call names the same type but with an entirely different,
    // invented field set — the exact shape an agent sends when it never saw
    // the real definition and is guessing from the user's request.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [
                { "name": "assignee", "type": "text", "protection": "user", "indexed": false },
                { "name": "sprint", "type": "text", "protection": "user", "indexed": false },
                {
                    "name": "status",
                    "type": "enum",
                    "protection": "user",
                    "indexed": false,
                    "coreValues": [
                        { "value": "ready for dev", "label": "Ready for dev" },
                        { "value": "done", "label": "Done" }
                    ]
                }
            ]
        }),
    )
    .await;

    let msg = result
        .expect_err("creating a schema that already exists must be rejected")
        .to_string();

    // States the call was not applied.
    assert!(
        msg.contains("NOT modified") && msg.contains("NOT applied"),
        "rejection must state the existing type was not modified and this call's \
         fields were not applied: {msg}"
    );

    // Carries the EXISTING type's real fields...
    assert!(
        msg.contains("title")
            && msg.contains("owner")
            && msg.contains("triage")
            && msg.contains("shipped"),
        "rejection must render the existing type's actual definition: {msg}"
    );

    // ...and not the invented fields from the rejected call.
    assert!(
        !msg.contains("assignee") && !msg.contains("sprint") && !msg.contains("ready for dev"),
        "rejection must not describe the fields from the rejected call as if they \
         belonged to the existing type: {msg}"
    );
}

// ============================================================================
// update_schema + title_template
// ============================================================================

/// Helper: create a schema with the given fields (no title_template)
async fn create_base_schema(svc: &Arc<NodeService>, name: &str, field_names: &[&str]) -> String {
    let fields: Vec<_> = field_names
        .iter()
        .map(|n| json!({ "name": n, "type": "string", "protection": "user", "indexed": false }))
        .collect();

    let result = handle_create_schema(svc, json!({ "name": name, "fields": fields }))
        .await
        .expect("create_base_schema: schema creation failed");

    result["schemaId"]
        .as_str()
        .expect("create_base_schema: schemaId missing in response")
        .to_string()
}

#[tokio::test]
async fn test_update_schema_add_valid_title_template() {
    let (svc, _tmp) = create_test_service().await;
    // NB: not "Person" — that now collides with the core `person` schema (ADR-037).
    let schema_id = create_base_schema(&svc, "Employee", &["first_name", "last_name"]).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "title_template": "{first_name} {last_name}"
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "Adding valid title_template should succeed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_update_schema_title_template_undefined_field_rejected() {
    let (svc, _tmp) = create_test_service().await;
    let schema_id = create_base_schema(&svc, "Contact", &["email"]).await;

    // Template references "name" which doesn't exist in this schema
    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "title_template": "{name}"
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "title_template referencing undefined field should fail"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("name"),
        "Error should name the bad field: {}",
        msg
    );
}

#[tokio::test]
async fn test_update_schema_remove_field_referenced_by_existing_template_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // Create schema with both fields and a title_template
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Employee",
            "fields": [
                { "name": "first_name", "type": "string", "protection": "user", "indexed": false },
                { "name": "last_name",  "type": "string", "protection": "user", "indexed": false }
            ],
            "title_template": "{first_name} {last_name}"
        }),
    )
    .await
    .expect("Employee schema creation failed");
    let schema_id = result["schemaId"].as_str().expect("schemaId missing");

    // Now try to remove first_name — template still references it
    let update_result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "remove_fields": ["first_name"]
        }),
    )
    .await;

    assert!(
        update_result.is_err(),
        "Removing a field still referenced by title_template should be rejected"
    );
    let msg = format!("{:?}", update_result.unwrap_err());
    assert!(
        msg.contains("first_name"),
        "Error should identify the dangling field: {}",
        msg
    );
}

#[tokio::test]
async fn test_update_schema_remove_field_and_clear_template_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    // Create schema with title_template
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Widget",
            "fields": [
                { "name": "sku",   "type": "string", "protection": "user", "indexed": false },
                { "name": "color", "type": "string", "protection": "user", "indexed": false }
            ],
            "title_template": "{sku}"
        }),
    )
    .await
    .expect("Widget schema creation failed");
    let schema_id = result["schemaId"].as_str().expect("schemaId missing");

    // Clearing the template (empty string) while removing sku should succeed:
    // the empty template has no tokens so validation passes.
    // Note: we pass an empty string because Option<String> with serde default
    // can't distinguish "omit" from "clear" — this tests the case where
    // the caller explicitly sets an empty template to clear it.
    let update_result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "remove_fields": ["sku"],
            "title_template": ""
        }),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Removing field after clearing template should succeed: {:?}",
        update_result
    );
}

#[tokio::test]
async fn test_update_schema_remove_unrelated_field_with_template_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    // Create schema with three fields; template only uses two
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Product",
            "fields": [
                { "name": "name",  "type": "string", "protection": "user", "indexed": false },
                { "name": "sku",   "type": "string", "protection": "user", "indexed": false },
                { "name": "notes", "type": "string", "protection": "user", "indexed": false }
            ],
            "title_template": "{name} ({sku})"
        }),
    )
    .await
    .expect("Product schema creation failed");
    let schema_id = result["schemaId"].as_str().expect("schemaId missing");

    // Remove "notes" — not in the template — should succeed
    let update_result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "remove_fields": ["notes"]
        }),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Removing a field not referenced by title_template should succeed: {:?}",
        update_result
    );
}

// ============================================================================
// rename_fields
// ============================================================================

#[tokio::test]
async fn test_rename_field_updates_schema_definition() {
    let (svc, _tmp) = create_test_service().await;

    // Create a schema with a field to rename
    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "RenameTest",
            "fields": [
                { "name": "old_name", "type": "string", "protection": "user", "indexed": false },
                { "name": "keep_me",  "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "old_name", "to": "new_name" }]
        }),
    )
    .await;

    assert!(result.is_ok(), "rename_fields should succeed: {:?}", result);
    let output = result.unwrap();
    assert_eq!(output["fieldsRenamed"], serde_json::json!(1));

    // Schema definition should reflect the rename
    let schema = svc
        .get_schema_node(schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");

    let field_names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        field_names.contains(&"new_name"),
        "new_name should exist in schema fields: {:?}",
        field_names
    );
    assert!(
        !field_names.contains(&"old_name"),
        "old_name should no longer exist in schema fields: {:?}",
        field_names
    );
    assert!(
        field_names.contains(&"keep_me"),
        "keep_me should be unchanged: {:?}",
        field_names
    );
}

#[tokio::test]
async fn test_rename_field_not_found_returns_error() {
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "RenameErrorTest",
            "fields": [
                { "name": "existing", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "does_not_exist", "to": "new_name" }]
        }),
    )
    .await;

    assert!(result.is_err(), "Renaming a nonexistent field should fail");
}

#[tokio::test]
async fn test_rename_field_to_existing_field_returns_error() {
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "RenameConflictTest",
            "fields": [
                { "name": "field_a", "type": "string", "protection": "user", "indexed": false },
                { "name": "field_b", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "field_a", "to": "field_b" }]
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "Renaming to an existing field name should fail"
    );
}

#[tokio::test]
async fn test_rename_field_migrates_node_data() {
    use crate::services::CreateNodeParams;

    let (svc, _tmp) = create_test_service().await;

    // Create a schema type
    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "DataMigrateTest",
            "fields": [
                { "name": "old_field", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing")
        .to_string();

    // Create a node instance with data in old_field
    let node_params = CreateNodeParams {
        id: None,
        node_type: schema_id.clone(),
        content: "test node".to_string(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties: serde_json::json!({
            &schema_id: { "old_field": "my_value" }
        }),
        lifecycle_status: None,
    };
    let node_id = svc
        .create_node_with_parent(node_params)
        .await
        .expect("create_node_with_parent failed");

    // Rename the field
    handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "old_field", "to": "new_field" }]
        }),
    )
    .await
    .expect("rename_fields should succeed");

    // Verify the node data was migrated
    let node = svc
        .get_node(&node_id)
        .await
        .expect("get_node failed")
        .expect("node not found");

    let ns_props = node.properties.get(&schema_id);
    assert!(
        ns_props.is_some(),
        "Namespaced properties should exist after rename"
    );
    let ns_props = ns_props.unwrap();
    assert_eq!(
        ns_props.get("new_field").and_then(|v| v.as_str()),
        Some("my_value"),
        "Value should be migrated to new_field"
    );
    assert!(
        ns_props.get("old_field").is_none(),
        "old_field should be removed after rename"
    );
}

// ============================================================================
// Protection-level enforcement: Core/System fields must survive remove_fields
// and rename_fields, and a malformed rename destination must never persist
// ============================================================================

#[tokio::test]
async fn test_update_schema_remove_fields_rejects_core_protected_field() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "remove_fields": ["status"]
        }),
    )
    .await;

    let err = result
        .expect_err("removing a Core-protected field must be rejected")
        .to_string();
    assert!(
        err.contains("status") && err.contains("core"),
        "error should name the field and its protection level: {err}"
    );

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    assert!(
        schema.fields.iter().any(|f| f.name == "status"),
        "status must survive a rejected removal: {:?}",
        schema.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_update_schema_remove_fields_rejects_system_protected_field() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "ai-chat",
            "remove_fields": ["last_active"]
        }),
    )
    .await;

    let err = result
        .expect_err("removing a System-protected field must be rejected")
        .to_string();
    assert!(
        err.contains("last_active") && err.contains("system"),
        "error should name the field and its protection level: {err}"
    );

    let schema = svc
        .get_schema_node("ai-chat")
        .await
        .expect("get_schema_node should succeed")
        .expect("core ai-chat schema should exist");
    assert!(
        schema.fields.iter().any(|f| f.name == "last_active"),
        "System-protected field must survive a rejected removal"
    );
}

/// The positive-path counterpart to the two tests above: the guard is
/// field-level, not schema-level — a User-protected field must remain
/// removable even on a schema NodeSpace ships.
#[tokio::test]
async fn test_update_schema_remove_fields_allows_user_protected_field_on_core_schema() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "remove_fields": ["priority"]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "removing a User-protected field, even on a core schema, must still succeed: {:?}",
        result
    );

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    assert!(
        !schema.fields.iter().any(|f| f.name == "priority"),
        "priority should have been removed"
    );
}

/// A batch mixing an allowed removal with a forbidden one must reject the
/// whole call rather than applying the allowed half before hitting the
/// protected one — the same all-or-nothing guarantee `update_schema` already
/// gives the namespace-prefix and playbook-impact checks.
#[tokio::test]
async fn test_update_schema_remove_fields_rejects_whole_batch_when_one_field_is_protected() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "remove_fields": ["priority", "status"]
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "a batch containing a protected field must be rejected entirely"
    );

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    let field_names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        field_names.contains(&"priority"),
        "priority must survive — the batch must not partially apply: {field_names:?}"
    );
    assert!(
        field_names.contains(&"status"),
        "status must survive: {field_names:?}"
    );
}

#[tokio::test]
async fn test_update_schema_rename_fields_rejects_core_protected_field_and_preserves_node_data() {
    use crate::services::CreateNodeParams;

    let (svc, _tmp) = create_test_service().await;

    // A real task instance with a real status value — a bypassed check would
    // migrate this exact value to the rejected destination key.
    let node_params = CreateNodeParams {
        id: None,
        node_type: "task".to_string(),
        content: "test task".to_string(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties: serde_json::json!({
            "task": { "status": "open" }
        }),
        lifecycle_status: None,
    };
    let node_id = svc
        .create_node_with_parent(node_params)
        .await
        .expect("create_node_with_parent failed");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "status", "to": "custom:status" }]
        }),
    )
    .await;

    let err = result
        .expect_err("renaming a Core-protected field must be rejected")
        .to_string();
    assert!(
        err.contains("status") && err.contains("core"),
        "error should name the field and its protection level: {err}"
    );

    // Schema definition unchanged.
    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    assert!(
        schema.fields.iter().any(|f| f.name == "status"),
        "status must survive a rejected rename"
    );
    assert!(
        !schema.fields.iter().any(|f| f.name == "custom:status"),
        "the rejected destination name must not appear in the schema"
    );

    // Node data untouched — the migration must never have run.
    let node = svc
        .get_node(&node_id)
        .await
        .expect("get_node failed")
        .expect("node not found");
    let task_props = node
        .properties
        .get("task")
        .expect("task-namespaced properties should exist");
    assert_eq!(
        task_props.get("status").and_then(|v| v.as_str()),
        Some("open"),
        "status value must be untouched by a rejected rename"
    );
    assert!(
        task_props.get("custom:status").is_none(),
        "no data should have been migrated to the rejected destination key"
    );
}

/// A display-only relabel (`from == to` with `friendlyName` set) is still a
/// modification of a protected field's definition and must be rejected the
/// same way an identity rename is.
#[tokio::test]
async fn test_update_schema_rename_fields_rejects_friendly_name_only_relabel_of_core_field() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "status", "to": "status", "friendlyName": "State" }]
        }),
    )
    .await;

    let err = result
        .expect_err("relabeling a Core-protected field must be rejected")
        .to_string();
    assert!(
        err.contains("status") && err.contains("core"),
        "error should name the field and its protection level: {err}"
    );

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    let status_field = schema
        .fields
        .iter()
        .find(|f| f.name == "status")
        .expect("status field should still exist");
    assert_eq!(
        status_field.friendly_name, "Status",
        "friendly_name must be unchanged by a rejected relabel"
    );
}

/// A malformed destination name must be rejected before the rename runs, not
/// after — `rename_schema_field` persists the schema rewrite and migrates
/// every node's property data immediately, so validating only the final
/// field list (as Phase 2 already did) catches the problem after the damage
/// is already committed.
#[tokio::test]
async fn test_update_schema_rename_fields_validates_destination_grammar_before_persisting() {
    use crate::services::CreateNodeParams;

    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [
                { "name": "capacity", "type": "number", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("create_schema should succeed");

    // A real venue instance, so a bypassed check would be visible in node
    // data too, not just the schema definition.
    let node_params = CreateNodeParams {
        id: None,
        node_type: "venue".to_string(),
        content: "test venue".to_string(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties: serde_json::json!({ "venue": { "capacity": 100 } }),
        lifecycle_status: None,
    };
    let node_id = svc
        .create_node_with_parent(node_params)
        .await
        .expect("create_node_with_parent failed");

    // A space is not valid field-name grammar — this is the exact kind of
    // malformed destination a caller can send.
    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "venue",
            "rename_fields": [{ "from": "capacity", "to": "max capacity" }]
        }),
    )
    .await;

    let err = result
        .expect_err("a malformed destination name must be rejected")
        .to_string();
    assert!(
        err.contains("max capacity"),
        "error should name the bad destination: {err}"
    );

    // The schema must not have been rewritten with the bad name.
    let schema = svc
        .get_schema_node("venue")
        .await
        .expect("get_schema_node should succeed")
        .expect("venue schema should exist");
    let field_names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        field_names.contains(&"capacity"),
        "capacity must survive a rejected rename: {field_names:?}"
    );
    assert!(
        !field_names.iter().any(|n| n.contains("max capacity")),
        "the malformed destination name must never reach the schema: {field_names:?}"
    );

    // Node data must not have been migrated to the bad key either.
    let node = svc
        .get_node(&node_id)
        .await
        .expect("get_node failed")
        .expect("node not found");
    let props = node
        .properties
        .get("venue")
        .expect("venue-namespaced properties should exist");
    assert_eq!(
        props.get("capacity").and_then(|v| v.as_i64()),
        Some(100),
        "capacity value must be untouched by a rejected rename"
    );
    assert!(
        props.get("max capacity").is_none(),
        "no data should have been migrated to the rejected destination key"
    );

    // A subsequent, valid update on the same schema must still succeed —
    // proving the schema was never left wedged on the rejected name.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "venue",
            "add_fields": [
                { "name": "address", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("a schema untouched by the rejected rename should accept a further update");
}

#[tokio::test]
async fn test_rename_field_friendly_name_only_updates_label_without_migrating_data() {
    use crate::services::CreateNodeParams;

    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "DisplayRenameTest",
            "fields": [
                { "name": "priority", "type": "string", "protection": "user", "indexed": false, "friendlyName": "Priority" }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing")
        .to_string();

    let node_params = CreateNodeParams {
        id: None,
        node_type: schema_id.clone(),
        content: "test node".to_string(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties: serde_json::json!({
            &schema_id: { "priority": "high" }
        }),
        lifecycle_status: None,
    };
    let node_id = svc
        .create_node_with_parent(node_params)
        .await
        .expect("create_node_with_parent failed");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "priority", "to": "priority", "friendlyName": "Urgency Level" }]
        }),
    )
    .await;
    assert!(
        result.is_ok(),
        "display-only rename should succeed: {:?}",
        result
    );
    assert_eq!(result.unwrap()["fieldsRenamed"], serde_json::json!(1));

    // The schema definition's label changed, but the storage key did not.
    let schema = svc
        .get_schema_node(&schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");
    let field = schema
        .get_field("priority")
        .expect("field 'priority' (the name) must still exist — only the label changed");
    assert_eq!(field.friendly_name, "Urgency Level");
    assert_eq!(field.name, "priority", "name must be unchanged");

    // No node property data was touched.
    let node = svc
        .get_node(&node_id)
        .await
        .expect("get_node failed")
        .expect("node not found");
    let ns_props = node
        .properties
        .get(&schema_id)
        .expect("namespaced properties should still exist");
    assert_eq!(
        ns_props.get("priority").and_then(|v| v.as_str()),
        Some("high"),
        "node property data must be completely untouched by a display-only rename"
    );
}

/// `SchemaField::friendly_name` is documented as always populated in
/// storage, with every reader assuming so unconditionally. An explicit
/// empty-string `friendlyName` on a display-only rename must never persist
/// blank — it must fall back to the same derived label
/// `apply_friendly_name_defaults` would produce for an omitted value on
/// create/`add_fields`, not skip validation just because it arrived through
/// a different code path.
#[tokio::test]
async fn test_rename_field_friendly_name_empty_string_derives_a_label_instead_of_persisting_blank()
{
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "BlankLabelTest",
            "fields": [
                { "name": "due_date", "type": "date", "protection": "user", "indexed": false, "friendlyName": "Original Label" }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "due_date", "to": "due_date", "friendlyName": "" }]
        }),
    )
    .await;
    assert!(
        result.is_ok(),
        "an empty-string friendlyName must not be rejected outright — it derives a label \
         instead: {:?}",
        result
    );

    let schema = svc
        .get_schema_node(schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");
    let field = schema.get_field("due_date").expect("field must exist");
    assert!(
        !field.friendly_name.trim().is_empty(),
        "friendly_name must never be persisted blank"
    );
    assert_eq!(
        field.friendly_name, "Due date",
        "an empty-string friendlyName derives the same label an omitted one would"
    );
}

/// Same guard, exercised with a whitespace-only value — `"   "` is not
/// caught by `Option::is_none()` any more than `""` is, and both must be
/// treated identically (derive, don't persist).
#[tokio::test]
async fn test_rename_field_friendly_name_whitespace_only_derives_a_label_instead_of_persisting_blank(
) {
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "WhitespaceLabelTest",
            "fields": [
                { "name": "due_date", "type": "date", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "due_date", "to": "due_date", "friendlyName": "   " }]
        }),
    )
    .await;
    assert!(result.is_ok(), "got: {:?}", result);

    let schema = svc
        .get_schema_node(schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");
    let field = schema.get_field("due_date").expect("field must exist");
    assert!(
        !field.friendly_name.trim().is_empty(),
        "friendly_name must never be persisted as whitespace-only"
    );
}

/// A derived label from an empty-string relabel must still disambiguate
/// against a sibling field's existing label, exactly like
/// `apply_friendly_name_defaults` does for an omitted value on create.
#[tokio::test]
async fn test_rename_field_friendly_name_empty_string_disambiguates_against_a_sibling_field() {
    let (svc, _tmp) = create_test_service().await;

    // "due_date" derives to "Due date"; pre-seed a second field that already
    // holds that exact label so the fallback must disambiguate, not collide.
    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "CollidingLabelTest",
            "fields": [
                { "name": "due_date", "type": "date", "protection": "user", "indexed": false, "friendlyName": "Something Else" },
                { "name": "custom:due_date", "type": "date", "protection": "user", "indexed": false, "friendlyName": "Due date" }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "due_date", "to": "due_date", "friendlyName": "" }]
        }),
    )
    .await;
    assert!(result.is_ok(), "got: {:?}", result);

    let schema = svc
        .get_schema_node(schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");
    let field = schema.get_field("due_date").expect("field must exist");
    assert_ne!(
        field.friendly_name, "Due date",
        "the derived label collides with the sibling field's label and must be disambiguated"
    );
    assert!(
        field.friendly_name.starts_with("Due date"),
        "expected a disambiguated form of the derived label, got: {:?}",
        field.friendly_name
    );
}

#[tokio::test]
async fn test_rename_field_from_equals_to_with_no_friendly_name_is_rejected() {
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "NoOpRenameTest",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "status", "to": "status" }]
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "from == to with no friendly_name changes nothing and must be rejected"
    );
}

#[tokio::test]
async fn test_rename_field_friendly_name_only_field_not_found_returns_error() {
    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "DisplayRenameMissingFieldTest",
            "fields": [
                { "name": "existing", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "does_not_exist", "to": "does_not_exist", "friendlyName": "Label" }]
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "a display-only rename of a nonexistent field must fail"
    );
}

#[tokio::test]
async fn test_rename_field_combined_identity_and_friendly_name_rename() {
    use crate::services::CreateNodeParams;

    let (svc, _tmp) = create_test_service().await;

    let create_result = handle_create_schema(
        &svc,
        json!({
            "name": "CombinedRenameTest",
            "fields": [
                { "name": "old_name", "type": "string", "protection": "user", "indexed": false, "friendlyName": "Old Name" }
            ]
        }),
    )
    .await
    .expect("Schema creation failed");
    let schema_id = create_result["schemaId"]
        .as_str()
        .expect("schemaId missing")
        .to_string();

    // A real node instance, so the combined path's data-migration half (not
    // just the schema-definition half) is actually exercised.
    let node_params = CreateNodeParams {
        id: None,
        node_type: schema_id.clone(),
        content: "test node".to_string(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties: serde_json::json!({
            &schema_id: { "old_name": "my_value" }
        }),
        lifecycle_status: None,
    };
    let node_id = svc
        .create_node_with_parent(node_params)
        .await
        .expect("create_node_with_parent failed");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "rename_fields": [{ "from": "old_name", "to": "new_name", "friendlyName": "Brand New Label" }]
        }),
    )
    .await;
    assert!(
        result.is_ok(),
        "a combined identity + display rename should succeed in one entry: {:?}",
        result
    );

    let schema = svc
        .get_schema_node(&schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema not found");
    let field = schema
        .get_field("new_name")
        .expect("field must exist under its new name");
    assert_eq!(field.friendly_name, "Brand New Label");
    assert!(
        schema.get_field("old_name").is_none(),
        "old_name must no longer exist"
    );

    // The identity-rename half of the combined path must migrate node data
    // exactly like a plain rename does — the friendly_name update riding
    // along in the same entry must not short-circuit it.
    let node = svc
        .get_node(&node_id)
        .await
        .expect("get_node failed")
        .expect("node not found");
    let ns_props = node
        .properties
        .get(&schema_id)
        .expect("namespaced properties should exist after the combined rename");
    assert_eq!(
        ns_props.get("new_name").and_then(|v| v.as_str()),
        Some("my_value"),
        "node data must be migrated to the new key by the combined rename+relabel path"
    );
    assert!(
        ns_props.get("old_name").is_none(),
        "old_name must be removed from node properties after the combined rename"
    );
}

// ============================================================================
// Description subtree tests
// ============================================================================

#[tokio::test]
async fn test_create_schema_stores_description_as_child_subtree() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "description": "Tracks money owed by clients for goods or services rendered.",
            "fields": []
        }),
    )
    .await
    .expect("create_schema should succeed");

    let schema_id = result["schemaId"].as_str().expect("schemaId missing");
    assert_eq!(schema_id, "invoice");

    // Description must NOT be stored in properties
    let node = svc
        .get_node(schema_id)
        .await
        .expect("get_node failed")
        .expect("schema node not found");
    assert!(
        node.properties.get("description").is_none(),
        "description should not be in properties after #1351"
    );

    // Description must be stored as child text node(s)
    let children = svc
        .get_children(schema_id)
        .await
        .expect("get_children failed");
    assert!(
        !children.is_empty(),
        "Schema should have child description nodes"
    );

    // The child content should contain the description text
    let combined: String = children
        .iter()
        .map(|c| c.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        combined.contains("money owed") || combined.contains("clients"),
        "Description text should appear in children: {:?}",
        combined
    );
}

#[tokio::test]
async fn test_create_schema_description_not_in_properties() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Campaign",
            "description": "Organizes outreach into phases and channels.",
            "fields": []
        }),
    )
    .await
    .expect("create_schema should succeed");

    let node = svc
        .get_node("campaign")
        .await
        .expect("get_node failed")
        .expect("schema node not found");

    // properties.description must be absent
    assert!(
        node.properties.get("description").is_none(),
        "description must not be written to properties"
    );
}

#[tokio::test]
async fn test_update_schema_replaces_description_subtree() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Contact",
            "description": "Original description.",
            "fields": []
        }),
    )
    .await
    .expect("create_schema should succeed");

    let initial_children = svc
        .get_children("contact")
        .await
        .expect("get_children failed");
    assert!(
        !initial_children.is_empty(),
        "Schema should have initial description children"
    );
    let initial_ids: std::collections::HashSet<String> =
        initial_children.iter().map(|c| c.id.clone()).collect();

    // Update description
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "contact",
            "description": "Updated description with new content."
        }),
    )
    .await
    .expect("update_schema should succeed");

    let updated_children = svc
        .get_children("contact")
        .await
        .expect("get_children failed");
    assert!(
        !updated_children.is_empty(),
        "Schema should still have description children after update"
    );

    // None of the original child IDs should survive — replace_description_subtree must
    // delete the full old subtree before creating a new one.
    for child in &updated_children {
        assert!(
            !initial_ids.contains(&child.id),
            "Stale child node {:?} should have been deleted by replace_description_subtree",
            child.id
        );
    }

    // The combined text should reflect the new description
    let combined: String = updated_children
        .iter()
        .map(|c| c.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        combined.contains("Updated description") || combined.contains("new content"),
        "Updated description should appear in children: {:?}",
        combined
    );
}

#[tokio::test]
async fn test_schema_behavior_can_have_children_is_true() {
    use crate::behaviors::{NodeBehavior, SchemaNodeBehavior};

    let behavior = SchemaNodeBehavior;
    assert!(
        behavior.can_have_children(),
        "SchemaNodeBehavior should allow children (description subtree)"
    );
}

#[tokio::test]
async fn test_schema_get_aggregated_content_returns_description_text() {
    use crate::behaviors::{NodeBehavior, SchemaNodeBehavior};

    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Billing",
            "description": "Tracks invoices and payment records for clients.",
            "fields": []
        }),
    )
    .await
    .expect("create_schema should succeed");

    let schema_node = svc
        .get_node("billing")
        .await
        .expect("get_node failed")
        .expect("schema node not found");

    let behavior = SchemaNodeBehavior;
    let aggregated = behavior.get_aggregated_content(&schema_node, &*svc).await;

    assert!(
        aggregated.is_some(),
        "get_aggregated_content should return Some when description subtree exists"
    );
    let text = aggregated.unwrap();
    assert!(
        text.contains("invoices") || text.contains("payment"),
        "Aggregated content should include description text for semantic search: {:?}",
        text
    );
}

// ============================================================================
// Malformed `fields` entries — error must LOCATE the bad entry
// ============================================================================
//
// An LLM is the primary caller of create_schema, and it repairs a rejected call
// from the error text alone. Serde's whole-payload error names only the absent
// key ("missing field `type`") with no position, so the model cannot tell which
// array element to fix; observed behaviour is that it mutates an element that
// was already correct and degrades the arguments on every retry. These tests
// pin the properties that make the error repairable: which entry, what is
// missing, and an instruction not to disturb the rest.

#[tokio::test]
async fn test_create_schema_missing_field_type_names_the_offending_entry() {
    let (svc, _tmp) = create_test_service().await;

    // Second entry lacks "type" — the exact shape observed in the agent logs.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Project",
            "fields": [
                { "name": "project_name", "type": "text", "required": true },
                { "name": "status", "required": true }
            ]
        }),
    )
    .await;

    let err = result.expect_err("field missing 'type' must be rejected");
    let msg = err.to_string();

    assert!(
        msg.contains("fields[1]"),
        "error must identify WHICH entry is malformed, not just the absent key: {msg}"
    );
    assert!(
        msg.contains("status"),
        "error should name the offending entry so the caller can match it: {msg}"
    );
    assert!(
        msg.contains("\"type\""),
        "error must say which key is missing: {msg}"
    );
    assert!(
        !msg.contains("fields[0]"),
        "the well-formed entry must not be implicated — blaming it is what drives \
         the caller to corrupt a correct field on retry: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_malformed_fields_error_preserves_correct_entries() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Project",
            "fields": [
                { "name": "project_name", "type": "text" },
                { "type": "text" }
            ]
        }),
    )
    .await;

    let msg = result
        .expect_err("field missing 'name' must be rejected")
        .to_string();

    // Without this instruction the observed failure mode is a retry that drops
    // `name` from the entry that already had one.
    assert!(
        msg.contains("leave every other field exactly as it was"),
        "error must tell the caller to correct only the listed entries: {msg}"
    );
    assert!(
        msg.contains("fields[1]"),
        "error must locate the entry missing 'name': {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_reports_every_malformed_entry_at_once() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Project",
            "fields": [
                { "name": "a" },
                { "name": "b", "type": "text" },
                { "type": "number" }
            ]
        }),
    )
    .await;

    let msg = result
        .expect_err("malformed entries must be rejected")
        .to_string();

    // Reporting one problem per round-trip would take as many retries as there
    // are bad entries, and each retry is an opportunity to corrupt a good one.
    assert!(
        msg.contains("fields[0]") && msg.contains("fields[2]"),
        "every malformed entry must be reported in a single error: {msg}"
    );
    assert!(
        !msg.contains("fields[1]"),
        "the valid entry must not be reported: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_rejects_blank_field_name_not_just_absent_key() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Project",
            "fields": [{ "name": "   ", "type": "text" }]
        }),
    )
    .await;

    let msg = result
        .expect_err("a whitespace-only field name is not a usable name")
        .to_string();
    assert!(
        msg.contains("fields[0]") && msg.contains("\"name\""),
        "blank name must be reported like an absent one: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_reports_non_object_field_entry() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Project",
            "fields": ["status"]
        }),
    )
    .await;

    let msg = result
        .expect_err("a bare string is not a field definition")
        .to_string();
    assert!(
        msg.contains("fields[0]") && msg.contains("not an object"),
        "error must explain that the entry is the wrong shape entirely: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_snake_case_core_values_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // The tool schema declares "coreValues" (camelCase). A caller that sends
    // the snake_case Rust field name instead must be rejected with an error
    // naming the unknown key, not silently produce an enum with no values.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                {
                    "name": "status",
                    "type": "enum",
                    "core_values": [
                        { "value": "pending", "label": "Pending" },
                        { "value": "paid", "label": "Paid" }
                    ]
                }
            ]
        }),
    )
    .await;

    let msg = result
        .expect_err("core_values is not a recognized key on a schema field")
        .to_string();
    assert!(
        msg.contains("core_values"),
        "error must name the unknown field so the caller can correct it: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_enum_field_with_core_values_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                {
                    "name": "status",
                    "type": "enum",
                    "coreValues": [
                        { "value": "pending", "label": "Pending" },
                        { "value": "paid", "label": "Paid" }
                    ]
                }
            ]
        }),
    )
    .await;

    let val = result.expect("enum field with coreValues should succeed end to end");
    assert_eq!(val["schemaId"], "invoice");
    let fields = val["fields"].as_array().expect("fields array");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0]["name"], "status");
    let core_values = fields[0]["coreValues"]
        .as_array()
        .expect("coreValues array");
    assert_eq!(core_values.len(), 2);
    assert_eq!(core_values[0]["value"], "pending");
}

#[tokio::test]
async fn test_create_schema_with_well_formed_fields_is_unaffected() {
    let (svc, _tmp) = create_test_service().await;

    // The locating pass must only ever turn an error into a better error — it
    // must not reject a payload that previously succeeded.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Equipment",
            "fields": [
                { "name": "replacement_cost", "type": "number" },
                { "name": "checked_out_on", "type": "date" }
            ]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "well-formed fields must still create the schema: {:?}",
        result
    );
}

/// A payload with no `fields` array at all must be rejected by the missing-fields
/// check with an actionable error — not intercepted by `describe_malformed_fields`'s
/// entry-locating pass (which only fires when `fields` is present and malformed),
/// and not silently accepted as an empty-fields schema (ADR-063: `description`
/// is no longer parsed into fields).
#[tokio::test]
async fn test_create_schema_without_fields_key_is_rejected() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "description": "contact email and capacity number"
        }),
    )
    .await;

    let msg = result
        .expect_err("create_schema with no 'fields' key must be rejected")
        .to_string();
    assert!(
        msg.contains("fields"),
        "error must name the missing 'fields' parameter: {msg}"
    );
}

#[tokio::test]
async fn test_update_schema_locates_malformed_add_fields_entry() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Album",
            "fields": [{ "name": "artist", "type": "text" }]
        }),
    )
    .await
    .expect("setup create_schema failed");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "album",
            "add_fields": [{ "name": "released" }]
        }),
    )
    .await;

    let msg = result
        .expect_err("add_fields entry missing 'type' must be rejected")
        .to_string();
    assert!(
        msg.contains("add_fields[0]"),
        "update_schema must locate the bad entry using its own key name: {msg}"
    );
    assert!(
        msg.contains("released"),
        "error should name the offending entry: {msg}"
    );
}

// ============================================================================
// Missing top-level `name` — the grammar cannot enforce it, so the error must
// ============================================================================
//
// Tool-call arguments are NOT constrained to the tool's JSON schema on this
// stack. llama.cpp emits `tool-create-schema ::= ("create_schema") gemma4-dict`
// where `gemma4-dict` is any well-formed JSON object, so `required: ["name",
// "fields"]` is never enforced during sampling. Upstream states this outright:
// "Gemma 4 only forces the structure, not the arguments", because
// `json-schema-to-grammar.cpp` "only produces rules for JSON and not Gemma's fc
// notation" (ggml-org/llama.cpp discussion 21839).
//
// Measured on the locked model: 17 of 17 failing create_schema calls in one run
// began `{"fields":[…]}` — complete, correct fields array, no top-level `name`.
// Serde's message for that is a bare "missing field `name`", and the model
// re-sent the identical payload until the duplicate-call guard broke the loop.

#[tokio::test]
async fn test_create_schema_missing_name_says_what_to_add_and_where() {
    let (svc, _tmp) = create_test_service().await;

    // The exact payload shape observed live, fields and all.
    let result = handle_create_schema(
        &svc,
        json!({
            "fields": [
                {
                    "coreValues": [
                        { "label": "Drafting", "value": "drafting" },
                        { "label": "Signed Off", "value": "signed_off" }
                    ],
                    "friendlyName": "Status",
                    "name": "status",
                    "type": "enum"
                },
                { "name": "estimated_days", "type": "number", "unique": false }
            ]
        }),
    )
    .await;

    let err = result.expect_err("a payload with no top-level name must be rejected");
    let msg = err.to_string();

    assert!(
        msg.contains("\"name\""),
        "error must name the missing key: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("top-level"),
        "error must say WHERE the key goes — the model's failure mode is putting it \
         in the fields array instead: {msg}"
    );
    assert!(
        msg.contains("Ticket"),
        "error should carry a worked example of the value's shape: {msg}"
    );
    // The whole point: the model must learn its fields survived, so it extends
    // the call rather than rewriting it.
    assert!(
        msg.contains("status") && msg.contains("estimated_days"),
        "error must reflect back the fields that were accepted, so the caller adds \
         one key instead of rebuilding the payload: {msg}"
    );
    assert!(
        !msg.contains("missing field `name`"),
        "the bare serde message is what the model demonstrably cannot act on; it \
         must be replaced, not appended to: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_empty_name_is_rejected_like_a_missing_one() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({ "name": "   ", "fields": [{ "name": "amount", "type": "number" }] }),
    )
    .await;

    let msg = result
        .expect_err("a blank name must be rejected")
        .to_string();
    assert!(
        msg.contains("\"name\""),
        "a whitespace-only name must get the same actionable message as an absent \
         one, not a different downstream error: {msg}"
    );
}

/// Malformed entries are reported BEFORE the missing top-level key: those are
/// what the caller must rebuild, whereas a missing `name` is a one-key addition
/// to an otherwise-correct call. Reporting them together would ask for both at
/// once, which is what drives a full rewrite.
#[tokio::test]
async fn test_create_schema_reports_field_problems_before_missing_name() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(&svc, json!({ "fields": [{ "name": "status" }] })).await;

    let msg = result.expect_err("both problems present").to_string();
    assert!(
        msg.contains("fields[0]"),
        "the field-level problem must be reported first: {msg}"
    );
}

/// A well-formed call is untouched — this check only ever converts one error
/// into a better one, never rejects something that would have succeeded.
#[tokio::test]
async fn test_create_schema_with_name_still_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Feature Write-up",
            "fields": [{ "name": "estimated_days", "type": "number" }]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a complete call must be unaffected: {result:?}"
    );
}

// ============================================================================
// An informationless field entry must not fail an otherwise-correct call
// ============================================================================
//
// Observed live: on the retry that had just CORRECTLY repaired a missing
// top-level `name`, the model appended `{"description":null,"name":null}` to a
// fields array whose other two entries were complete. The call was rejected for
// that entry; the next attempt added a stray `field_values` key; the one after
// abandoned create_schema and called create_node against a type that had never
// been created. One entry carrying no information cost the whole chain.

#[tokio::test]
async fn test_create_schema_ignores_an_all_null_field_entry() {
    let (svc, _tmp) = create_test_service().await;

    // The exact payload observed live, including the null entry.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Feature Write-up",
            "fields": [
                {
                    "coreValues": [
                        { "label": "Drafting", "value": "drafting" },
                        { "label": "Signed Off", "value": "signed_off" }
                    ],
                    "friendlyName": "Status",
                    "name": "status",
                    "type": "enum"
                },
                { "name": "estimated_days", "type": "number" },
                { "description": null, "name": null }
            ]
        }),
    )
    .await;

    let value = result.expect("an all-null entry must not fail the call");
    let fields = value["fields"]
        .as_array()
        .expect("fields array in the response");
    assert_eq!(
        fields.len(),
        2,
        "the informationless entry must be dropped, not stored: {fields:?}"
    );
}

#[tokio::test]
async fn test_create_schema_ignores_an_empty_field_entry() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [{ "name": "capacity", "type": "number" }, {}]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "an empty object declares nothing and must be dropped like an all-null \
         entry: {result:?}"
    );
}

/// The narrowness is the point: an entry with a real name but no type is a
/// genuine mistake the caller must see. Dropping it would silently discard a
/// field the user asked for.
#[tokio::test]
async fn test_create_schema_still_reports_a_named_entry_missing_its_type() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [{ "name": "capacity", "type": "number" }, { "name": "city" }]
        }),
    )
    .await;

    let msg = result
        .expect_err("a named entry with no type is a real error")
        .to_string();
    assert!(
        msg.contains("city"),
        "the offending entry must still be named: {msg}"
    );
}

/// Dropping an informationless entry must not renumber the entries that remain.
/// The whole value of the per-entry check is telling the caller WHICH element to
/// fix; a position that points at an element the caller got right sends it to
/// rewrite working input, which is the degrading-retry loop these errors exist
/// to break.
#[tokio::test]
async fn test_create_schema_reports_the_position_the_caller_actually_sent() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [
                { "description": null, "name": null },
                { "type": "number" }
            ]
        }),
    )
    .await;

    let msg = result
        .expect_err("the second entry has no name")
        .to_string();
    assert!(
        msg.contains("fields[1]"),
        "the malformed entry was sent at index 1 and must be reported there: {msg}"
    );
    assert!(
        !msg.contains("fields[0]"),
        "index 0 is the entry that gets dropped — reporting it points the caller \
         at something it did not get wrong: {msg}"
    );
}

/// An array of nothing but informationless entries is a caller that never
/// declared a single field. Dropping them all leaves `fields: []`, which
/// `handle_create_schema` reads as a deliberate fieldless (relationship-only)
/// schema and creates one — a silent wrong success where the caller gets no
/// signal to retry and the user gets a type that holds nothing.
#[tokio::test]
async fn test_create_schema_rejects_a_fields_array_of_only_null_entries() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Feature Write-up",
            "fields": [{ "description": null, "name": null }]
        }),
    )
    .await;

    let msg = result
        .expect_err("a fields array declaring nothing must be rejected, not emptied")
        .to_string();
    assert!(
        msg.contains("fields[0]"),
        "the caller must be told which entry declared nothing: {msg}"
    );
}

/// An explicitly empty `fields: []` stays valid — a relationship-only schema is
/// a deliberate choice, and it is only the *collapse* of a non-empty array to
/// empty that has to be caught.
#[tokio::test]
async fn test_create_schema_still_accepts_an_explicitly_empty_fields_array() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(&svc, json!({ "name": "Tag", "fields": [] })).await;

    assert!(
        result.is_ok(),
        "an explicitly empty fields array is a deliberate relationship-only \
         schema: {result:?}"
    );
}

/// A null `name` reaches serde as `invalid type: null, expected a string` —
/// which names neither the key nor what to do — unless it is treated as absent.
/// A model that writes `"name": null` inside a field entry writes it at the top
/// level too.
#[tokio::test]
async fn test_create_schema_null_name_gets_the_actionable_message() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({ "name": null, "fields": [{ "name": "amount", "type": "number" }] }),
    )
    .await;

    let msg = result
        .expect_err("a null name must be rejected")
        .to_string();
    assert!(
        msg.contains("top-level"),
        "a null name must get the same actionable guidance as an absent one: {msg}"
    );
    assert!(
        !msg.contains("invalid type: null"),
        "the bare serde type error is what this check exists to replace: {msg}"
    );
}

/// The same for a name of the wrong type entirely — serde's message for it does
/// not name the key either.
#[tokio::test]
async fn test_create_schema_non_string_name_gets_the_actionable_message() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({ "name": 42, "fields": [{ "name": "amount", "type": "number" }] }),
    )
    .await;

    let msg = result
        .expect_err("a numeric name must be rejected")
        .to_string();
    assert!(
        msg.contains("\"name\"") && msg.contains("top-level"),
        "a wrongly-typed name must be reported by key, with the shape it needs: {msg}"
    );
}

/// A null `name` one level down, inside a field entry that carries other real
/// values, is the same defect: it is not informationless (so it is not dropped)
/// and it is not a string (so it must be reported by position).
#[tokio::test]
async fn test_create_schema_reports_a_null_field_name() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [{ "name": null, "type": "number" }]
        }),
    )
    .await;

    let msg = result
        .expect_err("a null field name is a real error")
        .to_string();
    assert!(
        msg.contains("fields[0]") && msg.contains("\"name\""),
        "a null field name must be located like an absent one: {msg}"
    );
}

/// `add_fields` is `fields` under another name and takes the same treatment.
/// Leaving it unpatched reproduced, on `update_schema`, the exact failure
/// `create_schema` was fixed for.
#[tokio::test]
async fn test_update_schema_ignores_an_all_null_add_fields_entry() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [{ "name": "capacity", "type": "number" }]
        }),
    )
    .await
    .expect("seed schema");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "venue",
            "add_fields": [
                { "name": "city", "type": "text" },
                { "description": null, "name": null }
            ]
        }),
    )
    .await;

    let value = result.expect("an informationless entry must not fail the batch");
    assert_eq!(
        value["fieldsAdded"], 1,
        "only the real field is added: {value:?}"
    );
}

// ============================================================================
// title_template resolution is owned by validate_template_tokens
// ============================================================================
//
// There is deliberately no second, pre-deserialization check for this rule.
// A hand-rolled scanner disagreed with the authority on real inputs — it waved
// through "{a} {b" (unclosed) and prescribed a fix for "{} {b}" that still
// failed on the empty placeholder — handing the caller a confident but wrong
// repair instruction, which is worse than a terse correct one.

#[tokio::test]
async fn test_create_schema_title_template_all_placeholders_defined_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Equipment",
            "fields": [
                { "name": "equipment", "type": "text" },
                { "name": "status", "type": "text" }
            ],
            "title_template": "{equipment} ({status})"
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a template whose placeholders all resolve must create: {:?}",
        result
    );
}

#[tokio::test]
async fn test_create_schema_title_template_unclosed_placeholder_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // The case a hand-rolled pre-check silently passed: scanning for '{' and
    // taking the next '}' finds none, skips the token, and reports nothing.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Equipment",
            "fields": [{ "name": "equipment", "type": "text" }],
            "title_template": "{equipment} {status"
        }),
    )
    .await;

    let msg = result
        .expect_err("an unclosed placeholder must be rejected")
        .to_string();
    // Assert the AUTHORITY's exact phrasing, not just the word "unclosed".
    //
    // On this input a pre-check and the authority agree on the OUTCOME — both
    // reject — so an outcome assertion cannot tell which one spoke, and cannot
    // detect a second validator being reintroduced. Pinning the exact wording
    // at least catches a reintroduction that rewords the error, which is the
    // most this input can guarantee. The empty-placeholder test below is the
    // one that genuinely fails on a reintroduced pre-check.
    assert!(
        msg.contains("contains an unclosed '{' placeholder"),
        "the authority's own message must be what reaches the caller: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_title_template_empty_placeholder_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // This is the ratchet: it fails if a second, hand-rolled title_template
    // validator is ever reintroduced ahead of the authority.
    //
    // `fields` MUST stay empty. A pre-check that scans for placeholders and
    // prescribes the missing ones would report `status` here and return its own
    // message, which does not contain the authority's "empty" wording — so this
    // assertion fails and the reintroduction is caught. Define `status` in
    // `fields` and the placeholder resolves, the pre-check falls silent, the
    // authority runs anyway, and the test passes either way — blind to the
    // very thing it exists to catch.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Equipment",
            "fields": [],
            "title_template": "{} {status}"
        }),
    )
    .await;

    let msg = result
        .expect_err("an empty placeholder must be rejected")
        .to_string();
    assert!(
        msg.contains("contains an empty '{}' placeholder"),
        "the authority must reject on the empty placeholder, and its own message \
         must be what reaches the caller — a different message here means a \
         second validator spoke first: {msg}"
    );
}

#[tokio::test]
async fn test_update_schema_title_template_may_reference_preexisting_fields() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Gear",
            "fields": [{ "name": "label", "type": "text" }]
        }),
    )
    .await
    .expect("setup create_schema failed");

    // update_schema deliberately does NOT get the title_template pre-check: the
    // template may reference fields already on the stored schema, which the
    // payload alone cannot see. Pre-checking here would reject a valid call.
    let result = handle_update_schema(
        &svc,
        json!({ "schema_id": "gear", "title_template": "{label}" }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a template referencing an existing stored field must be accepted: {:?}",
        result
    );
}

// ============================================================================
// create_schema with an explicit empty `fields` array
// ============================================================================

/// An explicit empty array is a deliberate choice (e.g. a relationship-only
/// schema) and must be accepted — only a wholly absent `fields` key is
/// rejected. `description` is stored for semantic discovery only; it is not
/// parsed into fields (ADR-063).
#[tokio::test]
async fn test_create_schema_with_explicit_empty_fields_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "description": "contact email and capacity number",
            "fields": []
        }),
    )
    .await;

    let val = result.expect("explicit empty fields array should succeed");
    assert_eq!(val["schemaId"], "venue");
    assert_eq!(
        val["fields"]
            .as_array()
            .expect("fields should be an array")
            .len(),
        0,
        "description must not be parsed into fields"
    );

    // The schema must be readable back from storage, not merely returned.
    let stored = svc
        .get_schema_node("venue")
        .await
        .expect("get_schema_node should succeed");
    assert!(
        stored.is_some(),
        "Schema should be persisted and retrievable"
    );
}

/// The reserved-core-property shadowing warning must fire on explicit `fields`
/// (moved there from the deleted description-inference route, ADR-063) and
/// survive all the way into the response JSON.
#[tokio::test]
async fn test_create_schema_reserved_property_warning_reaches_response() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{ "name": "status", "type": "text" }]
        }),
    )
    .await
    .expect("create_schema should succeed");

    let warnings = result["warnings"]
        .as_array()
        .expect("warnings should be present in the response when a field collides");
    assert!(
        warnings.iter().any(|w| {
            let w = w.as_str().unwrap_or_default();
            w.contains("status") && w.contains("reserved core property")
        }),
        "Expected a reserved-core-property warning for 'status', got {warnings:?}"
    );

    // The field itself is stored verbatim — the warning does not rewrite it.
    let stored = svc
        .get_schema_node("ticket")
        .await
        .expect("get_schema_node should succeed")
        .expect("schema should be persisted");
    assert!(
        stored.fields.iter().any(|f| f.name == "status"),
        "Field name should be stored as given, not rewritten"
    );
}

/// Bare names are the convention for user-defined types, but a type NodeSpace
/// owns is different: a bare name added to a core schema can be claimed by a
/// core property in a future release, and the user's field would collide with
/// it. The prefix requirement is kept exactly where that hazard exists.
#[tokio::test]
async fn test_update_schema_core_type_requires_namespace_prefix() {
    let (svc, _tmp) = create_test_service().await;

    let unprefixed = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_fields": [{ "name": "effort", "type": "number" }]
        }),
    )
    .await;

    let err = unprefixed
        .expect_err("adding an unprefixed field to a core schema should be rejected")
        .to_string();
    assert!(
        err.contains("effort") && err.contains("namespace prefix"),
        "Error should name the field and the requirement, got: {err}"
    );

    // The same field is accepted once it carries a prefix.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_fields": [{ "name": "custom:effort", "type": "number" }]
        }),
    )
    .await
    .expect("adding a namespaced field to a core schema should succeed");

    let stored = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    assert!(
        stored.fields.iter().any(|f| f.name == "custom:effort"),
        "Namespaced field should be persisted on the core schema"
    );
}

/// Renaming is the other way a bare name can land on a core type, and it is the
/// more damaging one: `rename_schema_field` migrates node property data and
/// rewrites the schema per rename as it goes, so a check that ran afterwards
/// would reject the call with the offending key already written across every
/// node instance. The requirement is therefore enforced before any rename runs.
#[tokio::test]
async fn test_update_schema_core_type_rename_cannot_drop_namespace_prefix() {
    let (svc, _tmp) = create_test_service().await;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_fields": [{ "name": "custom:effort", "type": "number" }]
        }),
    )
    .await
    .expect("adding a namespaced field to a core schema should succeed");

    let renamed = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "custom:effort", "to": "effort" }]
        }),
    )
    .await;

    let err = renamed
        .expect_err("renaming a core-schema field to a bare name should be rejected")
        .to_string();
    assert!(
        err.contains("effort") && err.contains("namespace prefix"),
        "Error should name the field and the requirement, got: {err}"
    );

    // The rejected rename must not have been applied on the way out.
    let stored = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node should succeed")
        .expect("core task schema should exist");
    assert!(
        stored.fields.iter().any(|f| f.name == "custom:effort"),
        "Original namespaced field should survive a rejected rename: {:?}",
        stored.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    assert!(
        !stored.fields.iter().any(|f| f.name == "effort"),
        "Bare name must not reach a core schema via rename: {:?}",
        stored.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );

    // Renaming between two namespaced names stays allowed.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "custom:effort", "to": "custom:effort_points" }]
        }),
    )
    .await
    .expect("renaming to another namespaced name should succeed");
}

/// A user-defined type is unaffected by the core-type rule: bare names are the
/// convention there, so neither adding nor renaming to one is rejected.
#[tokio::test]
async fn test_update_schema_user_defined_type_allows_bare_names() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Venue",
            "fields": [{ "name": "capacity", "type": "number" }]
        }),
    )
    .await
    .expect("create_schema should succeed");

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "venue",
            "add_fields": [{ "name": "address", "type": "string" }],
            "rename_fields": [{ "from": "capacity", "to": "seats" }]
        }),
    )
    .await
    .expect("bare names on a user-defined type should be accepted");

    let stored = svc
        .get_schema_node("venue")
        .await
        .expect("get_schema_node should succeed")
        .expect("venue schema should exist");
    let names: Vec<&str> = stored.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"seats") && names.contains(&"address"),
        "Expected bare 'seats' and 'address', got {names:?}"
    );
}

// ============================================================================
// Unknown-field rejection (acceptance criterion, #1816)
// ============================================================================

/// `update_schema` is `create_schema`'s sibling — same silent-discard risk the
/// `coreValues` incident exposed on `create_schema` — so it must reject an
/// unknown top-level key through the real dispatch path rather than ignore it.
#[tokio::test]
async fn test_update_schema_rejects_unknown_field() {
    let (svc, _tmp) = create_test_service().await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "status", "type": "text" }]
        }),
    )
    .await
    .expect("create_schema should succeed");

    let err = handle_update_schema(&svc, json!({ "schema_id": "invoice", "addFields": [] }))
        .await
        .expect_err("update_schema with an unknown key must be rejected, not ignored");

    let msg = err.to_string();
    assert!(
        msg.contains("addFields"),
        "expected error naming unknown field `addFields`, got: {msg}"
    );
}

/// `additional_constraints` no longer exists as a param — the description-
/// inference route it configured was deleted (ADR-063). An unknown top-level
/// key by that name must be rejected the same as any other unknown key.
#[tokio::test]
async fn test_create_schema_rejects_additional_constraints_as_unknown_field() {
    let (svc, _tmp) = create_test_service().await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "description": "An invoice",
            "fields": [],
            "additional_constraints": {
                "requiredFields": ["status"]
            }
        }),
    )
    .await
    .expect_err("additional_constraints must be rejected as an unknown field, not accepted");

    let msg = err.to_string();
    assert!(
        msg.contains("additional_constraints"),
        "expected error naming unknown field `additional_constraints`, got: {msg}"
    );
}

// ============================================================================
// relationship targetType existence validation
// ============================================================================
//
// TARGET_TYPE_MUST_EXIST (skill_rules.rs) already tells the model this in
// prose; these tests cover the structural backstop added to
// validate_relationship_targets_exist so a relationship pointing at a type
// that doesn't exist fails loudly instead of persisting a dangling reference.

#[tokio::test]
async fn test_create_schema_rejects_relationship_to_nonexistent_target_type() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "amount", "type": "number", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await;

    let err = result.expect_err(
        "a relationship targeting a type that doesn't exist yet must be rejected, not persisted",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("customer"),
        "error should name the missing target type: {msg}"
    );
}

#[tokio::test]
async fn test_create_schema_with_relationship_to_existing_target_type_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    create_base_schema(&svc, "Customer", &["first_name"]).await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "amount", "type": "number", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a relationship targeting an existing schema should succeed: {:?}",
        result
    );
}

#[tokio::test]
async fn test_create_schema_relationship_with_no_target_type_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    // targetType is optional (Option<String>) — omitting it entirely is the
    // documented escape hatch for "the type doesn't exist yet" and must not
    // be treated as an invalid target.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "amount", "type": "number", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "billed_to", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a relationship with no targetType should not be treated as an invalid target: {:?}",
        result
    );
}

#[tokio::test]
async fn test_update_schema_add_relationships_rejects_nonexistent_target_type() {
    let (svc, _tmp) = create_test_service().await;

    let invoice_id = create_base_schema(&svc, "Invoice", &["amount"]).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": invoice_id,
            "add_relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await;

    let err = result.expect_err(
        "update_schema add_relationships targeting a nonexistent type must be rejected",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("customer"),
        "error should name the missing target type: {msg}"
    );
}

#[tokio::test]
async fn test_update_schema_add_relationships_to_existing_target_type_succeeds() {
    let (svc, _tmp) = create_test_service().await;

    let invoice_id = create_base_schema(&svc, "Invoice", &["amount"]).await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": invoice_id,
            "add_relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "add_relationships targeting an existing schema should succeed: {:?}",
        result
    );
}

// ============================================================================
// required reverse fields
// ============================================================================
//
// A relationship is stored once — one row between the two schema nodes — but
// read from both ends, so a declaration naming only the forward half leaves
// the stored edge half-declared: the target's side had to synthesize a label
// ("Invoice (Customer)" where the modeled answer is "Invoices") and carried no
// cardinality at all. Naming the inverse is a modeling decision only the
// author can make, so both halves are required rather than derived.

/// The corrected example matters as much as the rejection: an agent repairing
/// the call has only the error text to work from, so it must carry a payload
/// that would be accepted.
fn assert_reverse_field_error(msg: &str, expect_named: &[&str]) {
    for field in expect_named {
        assert!(
            msg.contains(field),
            "error must name the missing field '{field}': {msg}"
        );
    }
    assert!(
        msg.contains("Corrected:"),
        "error must show a corrected example: {msg}"
    );
    assert!(
        msg.contains("\"reverseName\":\"...\"") && msg.contains("\"reverseCardinality\":\"many\""),
        "the corrected example must include both reverse keys: {msg}"
    );
}

#[tokio::test]
async fn create_schema_rejects_a_relationship_without_reverse_name() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number" }],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseCardinality": "many" }
            ]
        }),
    )
    .await
    .expect_err("a relationship without reverseName must be rejected");

    let msg = err.to_string();
    assert_reverse_field_error(&msg, &["reverseName", "billed_to"]);
    assert!(
        !msg.contains("\"reverseCardinality\" and"),
        "only the genuinely missing field should be reported: {msg}"
    );
    // The rejection must not leave a half-created schema behind.
    assert!(
        svc.get_schema_node("invoice").await.unwrap().is_none(),
        "a rejected create must persist no schema node"
    );
}

#[tokio::test]
async fn create_schema_rejects_a_relationship_without_reverse_cardinality() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number" }],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices" }
            ]
        }),
    )
    .await
    .expect_err("a relationship without reverseCardinality must be rejected");

    assert_reverse_field_error(&err.to_string(), &["reverseCardinality", "billed_to"]);
    assert!(svc.get_schema_node("invoice").await.unwrap().is_none());
}

/// A null or blank value is the same defect as an absent key from the caller's
/// side, and must produce the same actionable error rather than being stored
/// as an empty reverse name.
#[tokio::test]
async fn create_schema_rejects_null_or_blank_reverse_fields() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    for reverse_name in [json!(null), json!("   ")] {
        let err = handle_create_schema(
            &svc,
            json!({
                "name": "Invoice",
                "fields": [{ "name": "amount", "type": "number" }],
                "relationships": [{
                    "name": "billed_to",
                    "targetType": "customer",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": reverse_name,
                    "reverseCardinality": "many"
                }]
            }),
        )
        .await
        .expect_err("a null or blank reverseName must be rejected");

        assert_reverse_field_error(&err.to_string(), &["reverseName"]);
    }
}

/// The error names the offending entry by position, so a caller repairing a
/// multi-relationship payload knows which one to fix.
#[tokio::test]
async fn create_schema_names_the_offending_relationship_entry() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;
    create_base_schema(&svc, "Vendor", &["name"]).await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number" }],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" },
                { "name": "supplied_by", "targetType": "vendor", "direction": "out", "cardinality": "one" }
            ]
        }),
    )
    .await
    .expect_err("the second relationship is missing both reverse fields");

    let msg = err.to_string();
    assert!(
        msg.contains("supplied_by") && msg.contains("entry 1"),
        "error must identify the offending entry by name and position: {msg}"
    );
    assert!(
        !msg.contains("billed_to"),
        "the well-formed relationship must not be implicated: {msg}"
    );
}

/// An untyped relationship (no `targetType`) is a documented shape, so the
/// message must read correctly without a target type name rather than printing
/// a placeholder where one belongs.
#[tokio::test]
async fn create_schema_reverse_error_reads_correctly_without_a_target_type() {
    let (svc, _tmp) = create_test_service().await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number" }],
            "relationships": [
                { "name": "related", "direction": "out", "cardinality": "many" }
            ]
        }),
    )
    .await
    .expect_err("a relationship without reverse fields must be rejected");

    let msg = err.to_string();
    assert_reverse_field_error(&msg, &["reverseName", "reverseCardinality"]);
    assert!(
        !msg.contains("<targetType>"),
        "the message must not print a placeholder where a type name belongs: {msg}"
    );
    assert!(
        !msg.contains("\"targetType\""),
        "the corrected example must omit targetType when the caller sent none: {msg}"
    );
}

#[tokio::test]
async fn update_schema_rejects_an_added_relationship_without_reverse_fields() {
    let (svc, _tmp) = create_test_service().await;
    let invoice_id = create_base_schema(&svc, "Invoice", &["amount"]).await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": invoice_id,
            "add_relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one" }
            ]
        }),
    )
    .await
    .expect_err("update_schema must apply the same reverse-field rule as create");

    assert_reverse_field_error(&err.to_string(), &["reverseName", "reverseCardinality"]);

    // The rejection leaves the schema untouched — no partial declaration.
    let schema = svc
        .get_schema_node("invoice")
        .await
        .unwrap()
        .expect("invoice schema");
    assert!(
        schema.relationships.is_empty(),
        "a rejected add_relationships must declare nothing"
    );
}

/// The end-to-end property the requirement exists for: a relationship declared
/// with both fields is readable from the TARGET's end by the declared
/// `reverseName`, with the declared `reverseCardinality` reported as the
/// inbound group's cardinality.
#[tokio::test]
async fn declared_reverse_fields_govern_the_inbound_group() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Customer", &["first_name"]).await;

    handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number" }],
            "relationships": [
                { "name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one", "reverseName": "invoices", "reverseCardinality": "many" }
            ]
        }),
    )
    .await
    .expect("a fully declared relationship must be accepted");

    for (id, node_type) in [("c1", "customer"), ("i1", "invoice")] {
        svc.store()
            .create_node(
                crate::models::Node::new_with_id(
                    id.to_string(),
                    node_type.to_string(),
                    id.to_string(),
                    json!({}),
                ),
                None,
                None,
            )
            .await
            .unwrap();
    }
    svc.create_relationship("i1", "billed_to", "c1", json!({}))
        .await
        .unwrap();

    let inbound = crate::ops::rel_ops::get_node_relationships(&svc, "c1")
        .await
        .unwrap();
    let group = inbound
        .groups
        .iter()
        .find(|g| g.relationship_name == "billed_to" && g.direction == "in")
        .expect("the customer must see the inbound group");

    assert_eq!(
        group.reverse_name, "invoices",
        "the inbound group is labeled by the declared reverseName, not a synthesized name"
    );
    assert_eq!(
        group.cardinality,
        crate::models::schema::RelationshipCardinality::Many,
        "the declared reverseCardinality governs the inbound side"
    );
    assert_eq!(group.count, 1);
    assert_eq!(group.related[0].id, "i1");
}

// ============================================================================
// self-referential relationships
// ============================================================================
//
// A relationship whose targetType names the schema being created is the one
// target that cannot dangle — it is guaranteed to resolve the moment the call
// commits. Rejecting it was a false positive, and it is a routine modelling
// shape (supersedes on an ADR, blocks on a task, parent on a category). These
// tests pin both halves: self-reference is accepted, and every other target
// keeps the existing dangling-reference guard unchanged.

#[tokio::test]
async fn test_create_schema_accepts_self_referential_relationship() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "ADR",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("a relationship targeting the schema being created must be accepted");

    assert_eq!(result["schemaId"], "adr");

    // The declaration actually landed — not merely accepted and dropped.
    let schema = svc
        .get_schema_node("adr")
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    let rel = schema
        .get_relationship("supersedes")
        .expect("self-referential declaration should be persisted");
    assert_eq!(rel.target_type.as_deref(), Some("adr"));
}

#[tokio::test]
async fn test_create_schema_accepts_self_reference_alongside_existing_target() {
    let (svc, _tmp) = create_test_service().await;

    create_base_schema(&svc, "Ticket", &["title"]).await;

    // Mixed batch: the self-reference is exempt from the lookup, the other
    // target still goes through it.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "ADR",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" },
                { "name": "decides", "targetType": "ticket", "direction": "out", "cardinality": "many", "reverseName": "decided_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("self-reference alongside an existing target should be accepted");

    assert_eq!(result["schemaId"], "adr");

    let schema = svc
        .get_schema_node("adr")
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    assert_eq!(schema.relationships.len(), 2);
}

#[tokio::test]
async fn test_create_schema_self_reference_reverse_edge_resolves() {
    let (svc, _tmp) = create_test_service().await;

    // The forward/reverse pair a self-referential type actually needs. Both
    // declarations point at the same schema and must both persist with their
    // own direction.
    handle_create_schema(
        &svc,
        json!({
            "name": "ADR",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" },
                { "name": "superseded_by", "targetType": "adr", "direction": "in", "cardinality": "one", "reverseName": "supersedes", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("a self-referential forward/reverse pair must be accepted");

    let schema = svc
        .get_schema_node("adr")
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");

    let forward = schema
        .get_relationship("supersedes")
        .expect("forward edge should be persisted");
    let reverse = schema
        .get_relationship("superseded_by")
        .expect("reverse edge should be persisted");

    assert_eq!(forward.target_type.as_deref(), Some("adr"));
    assert_eq!(reverse.target_type.as_deref(), Some("adr"));
    assert_ne!(
        forward.direction, reverse.direction,
        "the two edges of a self-referential pair must keep distinct directions"
    );
}

#[tokio::test]
async fn test_create_schema_self_reference_uses_normalized_schema_id() {
    let (svc, _tmp) = create_test_service().await;

    // The exemption keys off the schema ID (snake_case of the name), which is
    // also the only string a later lookup could resolve. A multi-word name
    // must therefore self-reference by its normalized ID.
    handle_create_schema(
        &svc,
        json!({
            "name": "Design Decision",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "design_decision", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("self-reference by normalized schema ID must be accepted");

    let schema = svc
        .get_schema_node("design_decision")
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    assert_eq!(
        schema
            .get_relationship("supersedes")
            .expect("declaration should be persisted")
            .target_type
            .as_deref(),
        Some("design_decision")
    );
}

#[tokio::test]
async fn test_create_schema_self_reference_by_display_name_is_rejected() {
    let (svc, _tmp) = create_test_service().await;

    // "Design Decision" is not a schema ID and never resolves through a
    // lookup, so it must still be rejected — the exemption is for the ID the
    // call is actually creating, not for anything resembling the name.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Design Decision",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "Design Decision", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect_err("a targetType that is not the schema ID must still be rejected");

    assert!(
        err.to_string().contains("Design Decision"),
        "error should name the unresolvable target: {err}"
    );
    assert!(
        svc.get_schema_node("design_decision")
            .await
            .expect("get_schema_node failed")
            .is_none(),
        "a rejected create must persist no schema node"
    );
}

#[tokio::test]
async fn test_punctuation_only_name_does_not_make_the_exemption_vacuous() {
    let (svc, _tmp) = create_test_service().await;

    // "!!!" survives the trim().is_empty() check but normalizes to "", so an
    // unguarded exemption would accept `targetType: ""` as self-reference.
    // Nothing can ever resolve that id, so the call must still be rejected —
    // and by the targetType error, not a later, vaguer one.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "!!!",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect_err("an unresolvable empty targetType must be rejected");

    assert!(
        err.to_string().contains("not an existing schema"),
        "the targetType guard should fire, not a later error: {err}"
    );
}

#[tokio::test]
async fn test_self_referential_declaration_writes_a_self_loop_row() {
    let (svc, _tmp) = create_test_service().await;

    // Every other test here reads declarations back through
    // `get_schema_declarations`, which selects only the `properties` column —
    // so `target_type` comes from the serialized declaration JSON and the
    // stored `out_node` is never observed. This queries the relationship row
    // directly, because `set_schema_declarations` writes
    // `out_node = target_type ?? schema_id`, and a self-referential
    // declaration is the one shape where that yields `in_node == out_node`.
    // Nothing above this layer can tell a correct self-loop from an `out_node`
    // written as something else entirely.
    handle_create_schema(
        &svc,
        json!({
            "name": "ADR",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" },
                { "name": "superseded_by", "targetType": "adr", "direction": "in", "cardinality": "one", "reverseName": "supersedes", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("self-referential create should succeed");

    let conn = svc
        .store()
        .read()
        .await
        .expect("failed to open a read connection");
    // Declaration rows only. The schema's description subtree also hangs off
    // 'adr' via a built-in `has_child` edge, which is structural hierarchy
    // rather than a declaration — the production reader excludes the built-in
    // names for the same reason.
    let mut rows = conn
        .query(
            "SELECT in_node, out_node, relationship_type FROM relationship \
             WHERE in_node = 'adr' AND relationship_type NOT IN \
             ('has_child', 'mentions', 'member_of', 'has_role') \
             ORDER BY relationship_type",
            (),
        )
        .await
        .expect("failed to query relationship rows");

    let mut stored: Vec<(String, String, String)> = Vec::new();
    while let Some(row) = rows.next().await.expect("row read failed") {
        stored.push((
            row.get(0).expect("in_node"),
            row.get(1).expect("out_node"),
            row.get(2).expect("relationship_type"),
        ));
    }

    // ORDER BY relationship_type — "superseded_by" sorts before "supersedes".
    let expected: Vec<(String, String, String)> = vec![
        ("adr".into(), "adr".into(), "superseded_by".into()),
        ("adr".into(), "adr".into(), "supersedes".into()),
    ];
    assert_eq!(
        stored, expected,
        "both edges must be stored as self-loops (in_node == out_node == 'adr'), \
         distinguished only by relationship_type"
    );
}

#[tokio::test]
async fn test_self_reference_does_not_bypass_the_already_exists_check() {
    let (svc, _tmp) = create_test_service().await;

    create_base_schema(&svc, "ADR", &["status"]).await;

    // The self-reference exemption skips only the target lookup. Re-creating a
    // schema that already exists must still be rejected by the duplicate check
    // that runs immediately after validation.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "ADR",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect_err("re-creating an existing schema must still be rejected");

    assert!(
        err.to_string().contains("already exists"),
        "the duplicate-schema rejection must still fire: {err}"
    );
}

#[tokio::test]
async fn test_update_schema_accepts_self_referential_relationship() {
    let (svc, _tmp) = create_test_service().await;

    let adr_id = create_base_schema(&svc, "ADR", &["status"]).await;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": adr_id,
            "add_relationships": [
                { "name": "supersedes", "targetType": adr_id, "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]
        }),
    )
    .await
    .expect("add_relationships targeting the schema being edited must be accepted");

    let schema = svc
        .get_schema_node(&adr_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    assert_eq!(
        schema
            .get_relationship("supersedes")
            .expect("self-referential declaration should be persisted")
            .target_type
            .as_deref(),
        Some(adr_id.as_str())
    );
}

// ============================================================================
// a rejected create persists nothing
// ============================================================================
//
// Exempting self-reference must not weaken the guarantee the pre-check stands
// in for: `handle_create_schema` is not atomic, so every rejection has to
// return before the first write. These pin that for each rejection reason.

#[tokio::test]
async fn test_rejected_create_persists_no_schema_node() {
    // (relationships, why, expected_in_error) — every declaration-level
    // rejection reason. `expected_in_error` pins WHICH rejection fired: without
    // it, a future change that made every case fail for one unrelated reason
    // (a new required parameter, say) would leave this test green while
    // covering none of the reasons it was written for.
    let cases: Vec<(serde_json::Value, &str, &str)> = vec![
        (
            json!([
                { "name": "has_child", "targetType": "adr", "direction": "out", "cardinality": "many", "reverseName": "parent", "reverseCardinality": "one" }
            ]),
            "reserved built-in relationship name",
            "reserved for a built-in structural relationship",
        ),
        (
            json!([
                { "name": "decides", "targetType": "ticket", "direction": "out", "cardinality": "one", "reverseName": "decided_by", "reverseCardinality": "one" }
            ]),
            "target type that does not exist",
            "targets 'ticket', which is not an existing schema",
        ),
        (
            json!([
                { "name": "supersedes", "targetType": "adrr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" }
            ]),
            "typo'd targetType that is neither the pending schema nor an existing one",
            "targets 'adrr', which is not an existing schema",
        ),
        (
            json!([
                { "name": "supersedes", "targetType": "adr", "direction": "out", "cardinality": "one", "reverseName": "superseded_by", "reverseCardinality": "one" },
                { "name": "decides", "targetType": "ticket", "direction": "out", "cardinality": "one", "reverseName": "decided_by", "reverseCardinality": "one" }
            ]),
            "a valid self-reference batched with an invalid target",
            "targets 'ticket', which is not an existing schema",
        ),
    ];

    for (relationships, why, expected_in_error) in cases {
        let (svc, _tmp) = create_test_service().await;

        let result = handle_create_schema(
            &svc,
            json!({
                "name": "ADR",
                "fields": [
                    { "name": "status", "type": "string", "protection": "user", "indexed": false }
                ],
                "relationships": relationships,
            }),
        )
        .await;

        let err = result
            .err()
            .unwrap_or_else(|| panic!("create should be rejected for {why}"));
        let msg = err.to_string();
        assert!(
            msg.contains(expected_in_error),
            "rejection for {why} should name its own reason \
             (expected {expected_in_error:?}), got: {msg}"
        );
        assert!(
            svc.get_schema_node("adr")
                .await
                .expect("get_schema_node failed")
                .is_none(),
            "no schema node may be written when a create is rejected for {why}"
        );
    }
}

// ============================================================================
// friendly_name write-boundary defaulting
// ============================================================================

#[tokio::test]
async fn test_create_schema_derives_friendly_name_when_omitted() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                { "name": "due_date", "type": "date", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("create_schema should succeed");

    assert_eq!(
        result["fields"][0]["friendlyName"], "Due date",
        "friendly_name omitted on input must be derived from `name` at the write boundary: {result:?}"
    );

    // The derived value is what actually landed in storage, not just what the
    // create_schema response echoes back.
    let schema = svc
        .get_schema_node("invoice")
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    assert_eq!(schema.fields[0].friendly_name, "Due date");
}

#[tokio::test]
async fn test_create_schema_respects_explicit_friendly_name() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [
                {
                    "name": "due_date",
                    "friendlyName": "Payment due",
                    "type": "date",
                    "protection": "user",
                    "indexed": false
                }
            ]
        }),
    )
    .await
    .expect("create_schema should succeed");

    assert_eq!(
        result["fields"][0]["friendlyName"], "Payment due",
        "an explicit friendlyName must not be overwritten by the derived default: {result:?}"
    );
}

#[tokio::test]
async fn test_update_schema_add_fields_derives_friendly_name_when_omitted() {
    let (svc, _tmp) = create_test_service().await;
    let schema_id = create_base_schema(&svc, "Invoice", &["amount"]).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "add_fields": [
                { "name": "due_date", "type": "date", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await;
    assert!(
        result.is_ok(),
        "update_schema add_fields should succeed: {result:?}"
    );

    let schema = svc
        .get_schema_node(&schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    let due_date = schema
        .get_field("due_date")
        .expect("due_date field should have been added");
    assert_eq!(
        due_date.friendly_name, "Due date",
        "add_fields must derive friendly_name the same way create_schema does"
    );
}

#[tokio::test]
async fn test_update_schema_add_fields_respects_explicit_friendly_name() {
    let (svc, _tmp) = create_test_service().await;
    let schema_id = create_base_schema(&svc, "Invoice", &["amount"]).await;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "add_fields": [
                {
                    "name": "due_date",
                    "friendlyName": "Payment due",
                    "type": "date",
                    "protection": "user",
                    "indexed": false
                }
            ]
        }),
    )
    .await
    .expect("update_schema add_fields should succeed");

    let schema = svc
        .get_schema_node(&schema_id)
        .await
        .expect("get_schema_node failed")
        .expect("schema should exist");
    assert_eq!(
        schema.get_field("due_date").unwrap().friendly_name,
        "Payment due"
    );
}

/// `custom:status` stripped and humanized for display derives the exact same
/// text ("Status") as the core `task` schema's existing `status` field —
/// different storage keys, same display label, reachable today via any
/// namespaced field whose base name happens to match an existing field on
/// the same schema. The write boundary must disambiguate the derived value
/// rather than let it collide.
#[tokio::test]
async fn test_update_schema_add_fields_disambiguates_friendly_name_colliding_with_existing_field() {
    let (svc, _tmp) = create_test_service().await;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_fields": [
                { "name": "custom:status", "type": "text", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("update_schema add_fields should succeed");

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node failed")
        .expect("task schema should exist");

    let core_status = schema.get_field("status").expect("core status field");
    let custom_status = schema
        .get_field("custom:status")
        .expect("custom:status field should have been added");

    assert_eq!(core_status.friendly_name, "Status");
    assert_ne!(
        custom_status.friendly_name, core_status.friendly_name,
        "a derived friendly_name that collides with an existing field's must be disambiguated, \
         not silently duplicated: got {:?}",
        custom_status.friendly_name
    );
    // The namespace is the disambiguator, so the collision is legible, not
    // just different.
    assert!(
        custom_status.friendly_name.contains("custom"),
        "expected the namespace to disambiguate the label, got {:?}",
        custom_status.friendly_name
    );
}

/// Self-collision within a single batch: two brand-new fields whose derived
/// labels would otherwise be identical must not collide with each other,
/// even though neither has an "existing" field to conflict with.
#[tokio::test]
async fn test_create_schema_disambiguates_friendly_name_colliding_within_same_batch() {
    let (svc, _tmp) = create_test_service().await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Employee",
            "fields": [
                { "name": "employee_name", "type": "string", "protection": "user", "indexed": false },
                { "name": "employeeName", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("create_schema should succeed");

    let fields = result["fields"].as_array().expect("fields array");
    let labels: Vec<&str> = fields
        .iter()
        .map(|f| f["friendlyName"].as_str().unwrap())
        .collect();
    assert_ne!(
        labels[0], labels[1],
        "two distinct fields deriving the same label within one batch must be disambiguated: {labels:?}"
    );
}

/// An explicit friendly_name is the caller's deliberate choice, even if it
/// collides with another field — collision disambiguation only ever touches
/// a value this function itself derived.
#[tokio::test]
async fn test_update_schema_add_fields_does_not_rewrite_an_explicit_colliding_friendly_name() {
    let (svc, _tmp) = create_test_service().await;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_fields": [
                {
                    "name": "custom:status",
                    "friendlyName": "Status",
                    "type": "text",
                    "protection": "user",
                    "indexed": false
                }
            ]
        }),
    )
    .await
    .expect("update_schema add_fields should succeed");

    let schema = svc
        .get_schema_node("task")
        .await
        .expect("get_schema_node failed")
        .expect("task schema should exist");
    assert_eq!(
        schema.get_field("custom:status").unwrap().friendly_name,
        "Status",
        "an explicitly-supplied friendly_name must never be silently rewritten"
    );
}

#[test]
fn field_rename_rejects_unknown_field() {
    let args = json!({ "from": "old_name", "to": "new_name", "toName": "new_name" });
    let err = serde_json::from_value::<FieldRename>(args).unwrap_err();
    assert!(
        err.to_string().contains("toName"),
        "expected error naming `toName`, got: {err}"
    );
}

// ============================================================================
// enum edge fields — end-to-end through the handlers
// ============================================================================
//
// `validate_edge_field_declarations` has its own unit tests; these cover the
// two CALL SITES that wire it in (`handle_create_schema` /
// `handle_update_schema`). Without them a refactor could drop either wiring and
// every other test would still pass.

/// A relationship declaring one enum edge field with the given coreValues.
fn rel_with_enum_edge_field(core_values: serde_json::Value) -> serde_json::Value {
    json!({
        "name": "granted_on",
        "targetType": "widget",
        "direction": "out",
        "cardinality": "many",
        "reverseName": "grants",
        "reverseCardinality": "many",
        "edgeFields": [{ "name": "access", "type": "enum", "coreValues": core_values }]
    })
}

#[tokio::test]
async fn create_schema_accepts_a_well_formed_enum_edge_field() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Widget", &["label"]).await;

    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Grant",
            "fields": [{ "name": "note", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [rel_with_enum_edge_field(json!([
                {"value": "owner", "label": "Owner"},
                {"value": "viewer", "label": "Viewer"}
            ]))]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a well-formed enum edge field should be accepted: {result:?}"
    );
}

#[tokio::test]
async fn create_schema_rejects_an_enum_edge_field_without_core_values() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Widget", &["label"]).await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Grant",
            "fields": [{ "name": "note", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "granted_on",
                "targetType": "widget",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "grants",
                "reverseCardinality": "many",
                "edgeFields": [{ "name": "access", "type": "enum" }]
            }]
        }),
    )
    .await
    .expect_err("an enum edge field with no coreValues must be rejected at create time");

    let msg = err.to_string();
    assert!(
        msg.contains("access") && msg.contains("coreValues"),
        "error should name the field and coreValues, got: {msg}"
    );
}

#[tokio::test]
async fn create_schema_rejects_an_enum_edge_field_default_outside_the_set() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Widget", &["label"]).await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Grant",
            "fields": [{ "name": "note", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "granted_on",
                "targetType": "widget",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "grants",
                "reverseCardinality": "many",
                "edgeFields": [{
                    "name": "access",
                    "type": "enum",
                    "default": "admin",
                    "coreValues": [{"value": "owner", "label": "Owner"}]
                }]
            }]
        }),
    )
    .await
    .expect_err("a default outside the declared set must be rejected at create time");

    assert!(err.to_string().contains("admin"), "got: {err}");
}

#[tokio::test]
async fn update_schema_rejects_an_enum_edge_field_without_core_values() {
    // The `handle_update_schema` wiring — a relationship ADDED by an edit must
    // be validated the same way one declared at create time is.
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Widget", &["label"]).await;
    let schema_id = create_base_schema(&svc, "Grant", &["note"]).await;

    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "add_relationships": [{
                "name": "granted_on",
                "targetType": "widget",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "grants",
                "reverseCardinality": "many",
                "edgeFields": [{ "name": "access", "type": "enum" }]
            }]
        }),
    )
    .await
    .expect_err("an enum edge field with no coreValues must be rejected at update time");

    let msg = err.to_string();
    assert!(
        msg.contains("access") && msg.contains("coreValues"),
        "error should name the field and coreValues, got: {msg}"
    );
}

#[tokio::test]
async fn update_schema_accepts_a_well_formed_enum_edge_field() {
    let (svc, _tmp) = create_test_service().await;
    create_base_schema(&svc, "Widget", &["label"]).await;
    let schema_id = create_base_schema(&svc, "Grant", &["note"]).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": schema_id,
            "add_relationships": [rel_with_enum_edge_field(json!([
                {"value": "owner", "label": "Owner"},
                {"value": "viewer", "label": "Viewer"}
            ]))]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a well-formed enum edge field should be accepted on update: {result:?}"
    );
}

#[tokio::test]
async fn schema_declaring_an_edge_field_on_a_builtin_relationship_is_rejected() {
    // Documents the limit the ENUM_EDGE_FIELDS guidance warns about: a builtin
    // structural name cannot be declared at all, so it can never carry an edge
    // field. Rejected by the reserved-name guard before edge-field validation
    // is reached — asserted here so the guidance and the behavior stay in step.
    let (svc, _tmp) = create_test_service().await;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Grant",
            "fields": [{ "name": "note", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "member_of",
                "targetType": "collection",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "members",
                "reverseCardinality": "many",
                "edgeFields": [{
                    "name": "access",
                    "type": "enum",
                    "coreValues": [{"value": "owner", "label": "Owner"}]
                }]
            }]
        }),
    )
    .await
    .expect_err("a builtin relationship name must be rejected as a declaration");

    assert!(
        err.to_string().contains("reserved"),
        "error should say the name is reserved, got: {err}"
    );
}
