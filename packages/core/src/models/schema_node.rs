//! Storage mapping of the [`SchemaNode`] wire type (ADR-086 §1).
//!
//! `SchemaNode` is defined once, in `nodespace-types`. This module is how one
//! is laid out in storage, as functions rather than a second type:
//!
//! - the **node row** holds the name (`content`) and, in `properties`, the
//!   field definitions, the `abstract` flag, the structural rules and the
//!   templates;
//! - the **declaration edges** in the `relationship` table hold the declared
//!   relationships and the `extends` parent (ADR-070, ADR-078). Neither is a
//!   `properties` key.
//!
//! A `SchemaNode` is assembled from both by [`from_storage`], which the
//! store's schema reads call, and taken apart by [`to_node`] and
//! [`to_declarations`], which the schema writes call. A schema's description
//! is neither: it is the schema node's child subtree.

use crate::models::schema::{
    is_type_system_relationship, SchemaField, SchemaRelationship, EXTENDS_RELATIONSHIP,
};
use crate::models::{CoreNodeType, Node, NodeEnvelope, SchemaNode, ValidationError};
use nodespace_types::RelationshipPath;

/// The key a schema node's row stores its context paths under.
pub const CONTEXT_PATHS_KEY: &str = "contextPaths";

/// Assemble a schema from its node row and its declaration edges.
///
/// `declarations` are the rows of the `relationship` table the schema
/// declares, the `extends` edge among them: the edge becomes
/// [`SchemaNode::extends`], and every other row an entry in
/// [`SchemaNode::relationships`].
///
/// # Errors
///
/// `ValidationError::InvalidNodeType` when `node` is not a schema node.
pub fn from_storage(
    node: Node,
    declarations: Vec<SchemaRelationship>,
) -> Result<SchemaNode, ValidationError> {
    if !CoreNodeType::Schema.is_exactly(&node.node_type) {
        return Err(ValidationError::InvalidNodeType(format!(
            "Expected 'schema', got '{}'",
            node.node_type
        )));
    }

    let flag = |key: &str| {
        node.properties
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let text = |key: &str| {
        node.properties
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    let is_core = is_core_schema(&node);
    let is_abstract = flag("abstract");
    let children = structural_rule(&node, "children");
    let parent = structural_rule(&node, "parent");
    let schema_version = node
        .properties
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(1);

    // An unreadable field list reads back as empty rather than failing the
    // schema, so it is logged: otherwise the only symptom is a schema with
    // no fields.
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

    let title_template = text("titleTemplate");
    let properties_header_summary_template = text("propertiesHeaderSummaryTemplate");

    // Unreadable paths read back as none, logged for the same reason an
    // unreadable field list is.
    let context_paths: Vec<RelationshipPath> = node
        .properties
        .get(CONTEXT_PATHS_KEY)
        .and_then(|v| match serde_json::from_value(v.clone()) {
            Ok(paths) => Some(paths),
            Err(e) => {
                tracing::warn!(
                    schema_id = %node.id,
                    error = %e,
                    "Failed to parse a schema's context paths; reading them as none"
                );
                None
            }
        })
        .unwrap_or_default();

    let (type_system, relationships): (Vec<_>, Vec<_>) = declarations
        .into_iter()
        .partition(|rel| is_type_system_relationship(&rel.name));
    let extends = type_system
        .into_iter()
        .find(|rel| rel.name == EXTENDS_RELATIONSHIP)
        .and_then(|rel| rel.target_type);

    Ok(SchemaNode {
        envelope: NodeEnvelope {
            properties: serde_json::json!({}),
            ..node
        },
        is_core,
        is_abstract,
        extends,
        children,
        parent,
        schema_version,
        fields,
        relationships,
        title_template,
        properties_header_summary_template,
        context_paths,
    })
}

/// One structural rule out of a schema node's stored properties. An absent
/// key is `any`. An unreadable one is logged and read as `any`: the
/// database's own copy of the rule, not this parse, is what refuses a write.
fn structural_rule<R: serde::de::DeserializeOwned + Default>(node: &Node, key: &str) -> R {
    match node.properties.get(key) {
        None => R::default(),
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            tracing::warn!(
                schema_id = %node.id,
                rule = key,
                error = %e,
                "Failed to parse a schema's structural rule; reading it as `any`"
            );
            R::default()
        }),
    }
}

/// Whether `node` is a built-in schema: a schema node whose row carries the
/// `isCore` flag that [`from_storage`] reads into [`SchemaNode::is_core`].
/// `false` for a user-defined schema and for any node that is not a schema.
///
/// The one Rust definition of the test: [`from_storage`] (and so
/// [`SchemaNode::is_core`]), the store's delete refusal, the service's guards
/// on minting or changing a core schema and the default search scope all use
/// it. `is_core_schema_sql` in `db/schema.rs` is its SQL mirror and must
/// stay in step.
pub fn is_core_schema(node: &Node) -> bool {
    CoreNodeType::Schema.is_exactly(&node.node_type)
        && node
            .properties
            .get("isCore")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// The `properties` of a schema's node row. Relationships and the `extends`
/// parent are not among them: they are declaration edges
/// ([`to_declarations`]).
pub fn to_properties(schema: &SchemaNode) -> serde_json::Value {
    let mut properties = serde_json::json!({
        "isCore": schema.is_core,
        "schemaVersion": schema.schema_version,
        "fields": schema.fields,
    });

    if schema.is_abstract {
        properties["abstract"] = serde_json::Value::Bool(true);
    }

    // `any` declares nothing, so it is not stored.
    if !schema.children.is_any() {
        properties["children"] = serde_json::json!(schema.children);
    }
    if !schema.parent.is_any() {
        properties["parent"] = serde_json::json!(schema.parent);
    }

    if let Some(template) = &schema.title_template {
        properties["titleTemplate"] = serde_json::Value::String(template.clone());
    }
    if let Some(template) = &schema.properties_header_summary_template {
        properties["propertiesHeaderSummaryTemplate"] = serde_json::Value::String(template.clone());
    }

    // No paths declares nothing, so nothing is stored.
    if !schema.context_paths.is_empty() {
        properties[CONTEXT_PATHS_KEY] = serde_json::json!(schema.context_paths);
    }

    properties
}

/// A schema's node row.
pub fn to_node(schema: &SchemaNode) -> Node {
    Node {
        properties: to_properties(schema),
        // A schema is a root, so it carries its name as its title like any
        // other root — the same value `NodeService::compute_title` derives.
        title: Some(crate::utils::strip_markdown(&schema.envelope.content)),
        ..schema.envelope.clone()
    }
}

/// A schema's declaration edges: its relationships, and its `extends` edge
/// when it has a parent. This is the full set the store replaces a schema's
/// declarations with, so the parent edge has to ride in it.
pub fn to_declarations(schema: &SchemaNode) -> Vec<SchemaRelationship> {
    let mut declarations = schema.relationships.clone();
    if let Some(parent) = &schema.extends {
        declarations.push(crate::schema::extends_chain::extends_declaration(parent));
    }
    declarations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SchemaChildrenRule, SchemaParentRule};
    use crate::schema::extends_chain::extends_declaration;
    use serde_json::json;

    fn schema_row() -> Node {
        Node::new_with_id(
            "issue".to_string(),
            "schema".to_string(),
            "Issue".to_string(),
            json!({
                "isCore": true,
                "schemaVersion": 2,
                "abstract": true,
                "children": { "rule": "none" },
                "parent": { "rule": "must_have_parent_of", "types": ["project"] },
                "titleTemplate": "{status}",
                "propertiesHeaderSummaryTemplate": "{status}",
                "contextPaths": [["project"], [{ "name": "child_of", "open_ended": true }]],
                "fields": [{
                    "name": "status",
                    "friendlyName": "Status",
                    "type": "enum",
                    "protection": "core",
                    "coreValues": [{ "value": "open", "label": "Open" }],
                    "indexed": true
                }]
            }),
        )
    }

    fn widgets() -> SchemaRelationship {
        serde_json::from_value(json!({
            "name": "widgets",
            "targetType": "widget",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "gadget",
            "reverseCardinality": "one"
        }))
        .unwrap()
    }

    #[test]
    fn from_storage_rejects_a_node_that_is_not_a_schema() {
        let task = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let err = from_storage(task, vec![]).unwrap_err();
        assert!(err.to_string().contains("Expected 'schema'"));
    }

    #[test]
    fn from_storage_reads_the_row_and_keeps_the_envelope() {
        let row = schema_row();
        let schema = from_storage(row.clone(), vec![]).unwrap();

        assert_eq!(schema.envelope.id, "issue");
        assert_eq!(schema.envelope.content, "Issue");
        assert_eq!(schema.envelope.node_type, "schema");
        assert_eq!(schema.envelope.version, row.version);
        assert_eq!(schema.envelope.properties, json!({}));
        assert!(schema.is_core && schema.is_abstract);
        assert_eq!(schema.schema_version, 2);
        assert_eq!(schema.children, SchemaChildrenRule::None);
        assert_eq!(
            schema.parent,
            SchemaParentRule::MustHaveParentOf {
                types: vec!["project".to_string()]
            }
        );
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.title_template.as_deref(), Some("{status}"));
        assert_eq!(
            schema
                .context_paths
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["project", "child_of*"]
        );
        assert!(schema.relationships.is_empty());
        assert!(schema.extends.is_none());
    }

    /// The `extends` edge becomes the parent, never a relationship entry.
    #[test]
    fn from_storage_splits_the_extends_edge_from_the_relationships() {
        let schema =
            from_storage(schema_row(), vec![widgets(), extends_declaration("task")]).unwrap();

        assert_eq!(schema.extends.as_deref(), Some("task"));
        assert_eq!(schema.relationships, vec![widgets()]);
    }

    /// A `relationships` or `extends` key in a schema row is not storage: the
    /// declaration edges are the one source.
    #[test]
    fn from_storage_ignores_relationship_and_extends_keys_in_the_row() {
        let row = Node::new(
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "fields": [],
                "extends": "stale",
                "relationships": [{
                    "name": "stale",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "stale_of",
                    "reverseCardinality": "many"
                }]
            }),
        );
        let schema = from_storage(row, vec![]).unwrap();
        assert!(schema.relationships.is_empty());
        assert!(schema.extends.is_none());
    }

    #[test]
    fn from_storage_reads_an_unreadable_field_list_as_empty() {
        let row = Node::new(
            "schema".to_string(),
            "Broken".to_string(),
            json!({ "fields": [{ "name": "status", "type": 42 }] }),
        );
        assert!(from_storage(row, vec![]).unwrap().fields.is_empty());
    }

    /// Nothing declared by an edge is written to the row, and everything the
    /// row holds survives the trip.
    #[test]
    fn the_row_holds_no_relationships_and_no_parent() {
        let schema =
            from_storage(schema_row(), vec![widgets(), extends_declaration("task")]).unwrap();

        let row = to_node(&schema);
        assert!(row.properties.get("relationships").is_none());
        assert!(row.properties.get("extends").is_none());
        assert_eq!(row.id, "issue");
        assert_eq!(row.node_type, "schema");
        assert_eq!(row.title.as_deref(), Some("Issue"));
        assert_eq!(row.properties, schema_row().properties);

        let declarations = to_declarations(&schema);
        assert_eq!(declarations, vec![widgets(), extends_declaration("task")]);

        let read = from_storage(row, declarations).unwrap();
        assert_eq!(read.extends, schema.extends);
        assert_eq!(read.relationships, schema.relationships);
        assert_eq!(read.children, schema.children);
        assert_eq!(read.parent, schema.parent);
    }

    /// `any` rules, a concrete type and absent templates store no key.
    #[test]
    fn defaults_are_not_stored() {
        let properties = to_properties(&SchemaNode::new("invoice", "Invoice"));
        assert_eq!(
            properties,
            json!({ "isCore": false, "schemaVersion": 1, "fields": [] })
        );
        assert!(to_declarations(&SchemaNode::new("invoice", "Invoice")).is_empty());
    }
}
