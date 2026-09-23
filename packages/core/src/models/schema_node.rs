//! Strongly-Typed SchemaNode
//!
//! Provides compile-time type safety for schema nodes using Universal Graph Architecture.
//! Field definitions are stored in `node.properties.fields`; relationship
//! declarations are stored as `relationship` table rows between schema nodes
//! (see `crate::models::schema` module docs) and hydrated into
//! [`SchemaNode::relationships`] by `SqliteStore::get_schema_node` /
//! `get_all_schemas` — callers always receive both in one fetch.
//!
//! ## Description Storage
//!
//! Schema descriptions are stored as a **child node subtree** (markdown parsed into text/header
//! nodes), not as `properties.description`. This enables:
//! - Semantic search: the subtree is aggregated into the schema's embedding via `get_aggregated_content`
//! - Rich content: descriptions can contain headers, lists, code blocks, etc.
//!
//! The `description` field on `SchemaNode` is a **transient/computed field** — it is NOT stored
//! in `properties` and NOT populated by `from_node()`. Callers that need the description text
//! must fetch it from the child subtree separately.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::SchemaNode;
//!
//! // Direct field access (no JSON parsing)
//! // let schema = service.get_schema_node("task").await?.unwrap();
//! // println!("{} fields, {} relationships", schema.fields.len(), schema.relationships.len());
//! ```

use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel, SchemaRelationship};
use crate::models::{Node, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Strongly-typed schema node with direct field access
///
/// Combines node metadata (id, content, timestamps) with schema-specific
/// fields (is_core, fields, relationships). Field definitions live in
/// `node.properties`; relationship declarations live in the `relationship`
/// table and are hydrated by the store's schema fetch methods.
///
/// Persist schema changes via `update_schema`/`create_schema` handlers (fields)
/// and `NodeService::set_schema_relationships` (declarations).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaNode {
    // ========================================================================
    // Node fields (from node table)
    // ========================================================================
    /// Unique identifier (e.g., "task", "date")
    pub id: String,

    /// Display name of the schema (e.g., "Task", "Date")
    pub content: String,

    /// Optimistic concurrency control version
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    // ========================================================================
    // Schema-specific fields (from node.properties)
    // ========================================================================
    /// Whether this is a core schema (shipped with NodeSpace)
    #[serde(default)]
    pub is_core: bool,

    /// Schema version number (increments on schema changes)
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// List of fields in this schema
    #[serde(default)]
    pub fields: Vec<SchemaField>,

    /// List of relationships to other node types
    ///
    /// Declarations live in the `relationship` table (edges between schema
    /// nodes), NOT in `node.properties` — this field is hydrated by the store's
    /// schema fetch methods and is empty on a bare
    /// [`from_node`](Self::from_node) conversion. See [`SchemaRelationship`].
    #[serde(default)]
    pub relationships: Vec<SchemaRelationship>,

    /// Optional template for computing the node's indexed title from its properties.
    ///
    /// Uses `{field_name}` syntax, e.g. `"{first_name} {last_name} ({email})"`.
    /// When set, title is interpolated from node properties instead of content.
    /// Missing or null fields are replaced with empty strings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_template: Option<String>,

    /// Optional template for rendering a compact property summary inline below the node title.
    ///
    /// Uses the same `{field_name}` syntax as `title_template`. The template string itself
    /// is persisted in the schema's properties JSON, but the **evaluation result** is
    /// computed client-side only and never written back to any node.
    /// Enum values resolve to labels; dates are human-formatted.
    /// Example: `"{status} · {company}"` → `"Active · Acme Corp"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties_header_summary_template: Option<String>,
}

fn default_version() -> i64 {
    1
}

fn default_schema_version() -> u32 {
    1
}

impl SchemaNode {
    /// Create a SchemaNode from an existing Node (for backward compatibility)
    ///
    /// This converts the JSON properties pattern to strongly-typed fields.
    /// Prefer using `get_schema_node()` from NodeService for direct deserialization.
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "schema".
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "schema" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'schema', got '{}'",
                node.node_type
            )));
        }

        // Extract fields from properties JSON
        let is_core = node
            .properties
            .get("isCore")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let schema_version = node
            .properties
            .get("schemaVersion")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(1);

        // A parse failure here (e.g. a pre-reset database whose stored field
        // JSON predates a required-field addition like `friendlyName`) is
        // swallowed to an empty Vec rather than surfaced as an error, so a
        // schema with fields silently reads back as a schema with none. Log
        // it — otherwise the only symptom is "this schema has no fields",
        // with no indication why.
        let fields: Vec<SchemaField> = node
            .properties
            .get("fields")
            .and_then(|v| match serde_json::from_value(v.clone()) {
                Ok(fields) => Some(fields),
                Err(e) => {
                    tracing::warn!(
                        schema_id = %node.id,
                        error = %e,
                        "Failed to parse schema fields; reading back as empty. \
                         Likely a stale storage format — reset the database."
                    );
                    None
                }
            })
            .unwrap_or_default();

        let title_template = node
            .properties
            .get("titleTemplate")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let properties_header_summary_template = node
            .properties
            .get("propertiesHeaderSummaryTemplate")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Self {
            id: node.id,
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            is_core,
            schema_version,
            fields,
            // Declarations are relationship-table rows; the store's schema
            // fetch methods hydrate this after conversion.
            relationships: Vec::new(),
            title_template,
            properties_header_summary_template,
        })
    }

    /// Convert to the universal Node STORAGE shape.
    ///
    /// `relationships` is deliberately NOT written into `properties` — the
    /// relationship table is the single source of truth for declarations, and
    /// a `relationships` key here would resurrect the parallel JSON copy this
    /// storage model removed. Callers that persist a schema with declarations
    /// write them separately via `NodeService::set_schema_relationships` (or
    /// `SqliteStore::set_schema_declarations`).
    pub fn into_node(self) -> Node {
        let mut properties = serde_json::json!({
            "isCore": self.is_core,
            "schemaVersion": self.schema_version,
            "fields": self.fields,
        });

        if let Some(template) = self.title_template {
            properties["titleTemplate"] = serde_json::Value::String(template);
        }

        if let Some(template) = self.properties_header_summary_template {
            properties["propertiesHeaderSummaryTemplate"] = serde_json::Value::String(template);
        }

        // A schema is a root, so it carries its name as its title like any
        // other root — the same value `NodeService::compute_title` derives.
        let title = Some(crate::utils::strip_markdown(&self.content));

        Node {
            id: self.id,
            node_type: "schema".to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title,
            lifecycle_status: "active".to_string(),
        }
    }

    /// Convert to a Node for the WIRE (gRPC/Tauri) boundary, with the hydrated
    /// `relationships` embedded in `properties` alongside `fields`.
    ///
    /// The desktop app parses the daemon's schema responses back through
    /// `nodespace_types::SchemaNode::from_node`, which reads
    /// `properties.relationships` — the wire contract intentionally carries the
    /// fully-assembled view so no client needs to know how declarations are
    /// stored. Never persist the result: that would write the JSON copy back
    /// into storage.
    pub fn into_wire_node(self) -> Node {
        let relationships =
            serde_json::to_value(&self.relationships).unwrap_or_else(|_| serde_json::json!([]));
        let mut node = self.into_node();
        node.properties["relationships"] = relationships;
        node
    }

    /// Get a field by name
    pub fn get_field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Get a mutable field by name
    pub fn get_field_mut(&mut self, name: &str) -> Option<&mut SchemaField> {
        self.fields.iter_mut().find(|f| f.name == name)
    }

    /// Get all valid values for an enum field (core + user values combined)
    ///
    /// Returns `None` if the field doesn't exist or isn't an enum.
    /// Returns `EnumValue` structs with both value and label.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = store.get_schema_node("task").await?.unwrap();
    /// let status_values = schema.get_enum_values("status");
    /// // Returns: Some([EnumValue::new("open", "Open"), ...])
    /// ```
    pub fn get_enum_values(&self, field_name: &str) -> Option<Vec<EnumValue>> {
        let field = self.get_field(field_name)?;

        // Only return values for enum fields
        if field.field_type != "enum" {
            return None;
        }

        let mut values = Vec::new();
        if let Some(core_vals) = &field.core_values {
            values.extend(core_vals.clone());
        }
        if let Some(user_vals) = &field.user_values {
            values.extend(user_vals.clone());
        }

        Some(values)
    }

    /// Get all valid value strings for an enum field (for validation)
    ///
    /// Returns only the value strings, not the labels. Use this for validation
    /// when checking if a value is valid for an enum field. For UI display where
    /// you need both values and labels, use [`get_enum_values`] instead.
    ///
    /// # Example
    /// ```ignore
    /// let valid_values = schema.get_enum_value_strings("status");
    /// // Returns: Some(["open", "in_progress", "done", "blocked"])
    /// ```
    pub fn get_enum_value_strings(&self, field_name: &str) -> Option<Vec<String>> {
        self.get_enum_values(field_name)
            .map(|values| values.into_iter().map(|v| v.value).collect())
    }

    /// Check if a field can be deleted based on its protection level
    ///
    /// Only `User` protected fields can be deleted.
    pub fn can_delete_field(&self, field_name: &str) -> bool {
        self.get_field(field_name)
            .map(|f| f.protection == SchemaProtectionLevel::User)
            .unwrap_or(false)
    }

    /// Check if a field can be modified based on its protection level
    ///
    /// Only `User` protected fields can be modified (type changes, etc.).
    /// Core/System fields are immutable.
    pub fn can_modify_field(&self, field_name: &str) -> bool {
        self.get_field(field_name)
            .map(|f| f.protection == SchemaProtectionLevel::User)
            .unwrap_or(false)
    }

    // ========================================================================
    // Relationship helpers
    // ========================================================================

    /// Get a relationship by name
    pub fn get_relationship(&self, name: &str) -> Option<&SchemaRelationship> {
        self.relationships.iter().find(|r| r.name == name)
    }

    /// Get a mutable relationship by name
    pub fn get_relationship_mut(&mut self, name: &str) -> Option<&mut SchemaRelationship> {
        self.relationships.iter_mut().find(|r| r.name == name)
    }

    /// Check if this schema has any relationships defined
    pub fn has_relationships(&self) -> bool {
        !self.relationships.is_empty()
    }

    /// Get all relationships targeting a specific node type
    ///
    /// Includes untyped relationships (target_type: None) since they accept any target.
    pub fn get_relationships_to(&self, target_type: &str) -> Vec<&SchemaRelationship> {
        self.relationships
            .iter()
            .filter(|r| {
                r.target_type
                    .as_deref()
                    .map(|t| t == target_type)
                    .unwrap_or(true) // None = untyped, matches all types
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::SchemaProtectionLevel;
    use serde_json::json;

    fn create_test_schema_node() -> Node {
        Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({
                "isCore": true,
                "schemaVersion": 2,
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "core",
                        "coreValues": [
                            { "value": "open", "label": "Open" },
                            { "value": "done", "label": "Done" }
                        ],
                        "indexed": true
                    }
                ]
            }),
        )
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = create_test_schema_node();
        assert!(SchemaNode::from_node(node).is_ok());

        let wrong_type = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let result = SchemaNode::from_node(wrong_type);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected 'schema'"));
    }

    #[test]
    fn test_from_node_extracts_fields() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        assert!(schema.is_core);
        assert_eq!(schema.schema_version, 2);
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "status");
    }

    #[test]
    fn test_into_node_excludes_relationships_and_wire_node_includes_them() {
        let node = create_test_schema_node();
        let mut schema = SchemaNode::from_node(node).unwrap();
        schema.relationships = vec![serde_json::from_value(json!({
            "name": "widgets",
            "targetType": "widget",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "gadget",
            "reverseCardinality": "one"
        }))
        .unwrap()];

        // Storage shape: no relationships key — the relationship table is the
        // single source of truth for declarations.
        let storage = schema.clone().into_node();
        assert!(storage.properties.get("relationships").is_none());

        // Wire shape: relationships embedded for clients that parse
        // properties.relationships (desktop app via the daemon).
        let wire = schema.into_wire_node();
        assert_eq!(wire.properties["relationships"][0]["name"], "widgets");
        assert_eq!(wire.properties["relationships"][0]["targetType"], "widget");
    }

    #[test]
    fn test_from_node_ignores_legacy_relationships_key() {
        // A legacy/foreign node carrying a relationships key must not have it
        // read back as declarations — storage reads come from the relationship
        // table only.
        let node = Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({
                "isCore": true,
                "fields": [],
                "relationships": [{
                    "name": "stale",
                    "direction": "out",
                    "cardinality": "many"
                }]
            }),
        );
        let schema = SchemaNode::from_node(node).unwrap();
        assert!(schema.relationships.is_empty());
    }

    #[test]
    fn test_into_node_preserves_data() {
        let original = create_test_schema_node();
        let original_id = original.id.clone();

        let schema = SchemaNode::from_node(original).unwrap();
        let converted = schema.into_node();

        assert_eq!(converted.id, original_id);
        assert_eq!(converted.node_type, "schema");
        assert_eq!(converted.content, "task");
    }

    #[test]
    fn test_get_field() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        let status_field = schema.get_field("status");
        assert!(status_field.is_some());
        assert_eq!(status_field.unwrap().field_type, "enum");

        let missing_field = schema.get_field("nonexistent");
        assert!(missing_field.is_none());
    }

    #[test]
    fn test_direct_field_mutation() {
        let node = create_test_schema_node();
        let mut schema = SchemaNode::from_node(node).unwrap();

        // Direct field mutation
        schema.schema_version += 1;

        assert_eq!(schema.schema_version, 3);
    }

    #[test]
    fn test_add_field_via_push() {
        let node = create_test_schema_node();
        let mut schema = SchemaNode::from_node(node).unwrap();

        let new_field = SchemaField {
            name: "priority".to_string(),
            friendly_name: "Priority".to_string(),
            field_type: "number".to_string(),
            local_only: false,
            protection: SchemaProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: Some(json!(0)),
            description: Some("Priority level".to_string()),
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
        };

        schema.fields.push(new_field);
        assert_eq!(schema.fields.len(), 2);
        assert!(schema.get_field("priority").is_some());
    }

    #[test]
    fn test_get_enum_values() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        let values = schema.get_enum_values("status").unwrap();
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|v| v.value == "open"));
        assert!(values.iter().any(|v| v.value == "done"));
        // Verify labels are present
        assert!(values.iter().any(|v| v.label == "Open"));
        assert!(values.iter().any(|v| v.label == "Done"));

        // Test the string-only helper
        let value_strings = schema.get_enum_value_strings("status").unwrap();
        assert_eq!(value_strings.len(), 2);
        assert!(value_strings.contains(&"open".to_string()));
        assert!(value_strings.contains(&"done".to_string()));

        // Non-enum field should return None
        assert!(schema.get_enum_values("nonexistent").is_none());
    }

    #[test]
    fn test_can_delete_field() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Core field cannot be deleted
        assert!(!schema.can_delete_field("status"));
        // Non-existent field returns false
        assert!(!schema.can_delete_field("nonexistent"));
    }

    #[test]
    fn test_serde_serialization() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Uses camelCase for JSON
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["isCore"], true);
        assert_eq!(json["schemaVersion"], 2);
    }

    #[test]
    fn test_serde_deserialization() {
        // Direct deserialization (simulates node table query result)
        let json = json!({
            "id": "test-schema",
            "content": "Test Schema",
            "version": 1,
            "createdAt": "2025-01-01T00:00:00Z",
            "modifiedAt": "2025-01-01T00:00:00Z",
            "isCore": false,
            "schemaVersion": 1,
            "fields": []
        });

        let schema: SchemaNode = serde_json::from_value(json).unwrap();
        assert_eq!(schema.id, "test-schema");
        assert_eq!(schema.content, "Test Schema");
        assert!(!schema.is_core);
        assert_eq!(schema.schema_version, 1);
        assert!(schema.fields.is_empty());
    }
}
