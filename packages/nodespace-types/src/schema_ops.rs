//! The parameters and results of the schema operations (ADR-086 §4).
//!
//! A schema is written through operations, `create_schema` and
//! `update_schema`, rather than through a field patch, so their parameters are
//! wire types like any other: defined once here and decoded by every surface
//! with the same serde.
//!
//! Top-level keys are snake_case, because a person or a model types them
//! (`schema_id`, `add_fields`, `title_template`). The `SchemaField` and
//! `SchemaRelationship` entries nested inside keep their camelCase metadata
//! keys.

use serde::{Deserialize, Serialize};

use crate::schema::{
    EnumValue, SchemaChildrenRule, SchemaField, SchemaParentRule, SchemaRelationship,
};

/// Parameters of `create_schema`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct CreateSchemaParams {
    /// Schema name (e.g., "Invoice", "Customer"). The schema id is derived
    /// from it.
    pub name: String,
    /// Brief prose summary of what this entity type represents. Stored as the
    /// schema node's child subtree for semantic discovery; not parsed into
    /// fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Explicit field definitions. Required: `[]` declares a type with no
    /// fields, an absent key is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SchemaField>>,
    /// Schema id of a parent type this schema specializes (ADR-078).
    ///
    /// Structural vocabulary, on the same footing as `fields`, not a
    /// relationship the caller authors: `extends` and `extended_by` are
    /// refused in `relationships`. Declaring it composes this schema's
    /// effective field set as its own fields plus the parent's (additive
    /// only, single parent, no override), and instances created under this
    /// schema carry *this* schema's id as their `node_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Declare the type abstract (ADR-086 §6): it can be extended and
    /// queried, but no node is created with it as its `node_type` or retyped
    /// into it. Only its subtypes are instantiated.
    #[serde(default, rename = "abstract")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub is_abstract: bool,
    /// Which children this type's nodes may have (ADR-089): `{"rule": "any"}`
    /// (the default), `{"rule": "none"}`, or
    /// `{"rule": "any_except", "types": [...]}`. A named type covers its
    /// subtypes. A subtype inherits its base's rule and may only tighten it.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub children: SchemaChildrenRule,
    /// Where this type's nodes may sit in the tree (ADR-089):
    /// `{"rule": "any"}` (the default), `{"rule": "must_be_root"}`, or
    /// `{"rule": "must_have_parent_of", "types": [...]}`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent: SchemaParentRule,
    /// Relationship definitions to other schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationships: Option<Vec<SchemaRelationship>>,
    /// Template for computing a node's title from its field values, with
    /// `{field_name}` tokens that reference fields defined in `fields`.
    /// Example: `"{first_name} {last_name}"` for a customer schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_template: Option<String>,
    /// Template for the property summary shown under a node's title, in the
    /// same `{field_name}` syntax. Evaluated by the client.
    /// Example: `"{status} · {company}"` → `"Active · Acme Corp"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties_header_summary_template: Option<String>,
}

/// Result of `create_schema`: what was persisted, read back from the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct CreateSchemaOutput {
    /// Id of the created schema (the normalized name).
    pub schema_id: String,
    pub is_core: bool,
    /// Schema version.
    pub version: u32,
    /// The description text written to the schema's child subtree.
    pub description: String,
    /// The fields the schema declares.
    pub fields: Vec<SchemaField>,
    /// The schema id of the type the schema extends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// The relationships the schema declares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub relationships: Vec<SchemaRelationship>,
    /// E.g. a field name shadowing a reserved core property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

/// A single field rename within `update_schema`.
///
/// Two different renames share this shape:
/// - **identity rename** (`from` != `to`): rekeys `name`, migrates every
///   existing node's property data, and breaks `titleTemplate`, CEL and
///   query-filter references to the old name.
/// - **display rename** (`from` == `to`, `friendlyName` set): changes only the
///   display label and migrates nothing. Both may be combined in one entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldRename {
    /// Current field name.
    pub from: String,
    /// New field name (the same value as `from` for a display-only rename).
    pub to: String,
    /// New display label. Omit to leave `friendlyName` exactly as stored,
    /// including when it was derived from the old `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
}

/// One field's worth of `add_field_values`: the target field and the values
/// to append to its `userValues`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldValueAddition {
    /// Name of the existing field to extend (an `enum` field with
    /// `extensible: true`).
    pub field: String,
    /// Values to append. Each `value` must not already exist among the
    /// field's core and user values.
    pub values: Vec<EnumValue>,
}

/// Parameters of `update_schema`: a batch of changes to one schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct UpdateSchemaParams {
    /// Schema id to update.
    pub schema_id: String,
    /// Fields to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_fields: Option<Vec<SchemaField>>,
    /// Field names to remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_fields: Option<Vec<String>>,
    /// Append values to an existing field's `userValues` (ADR-076). Gated on
    /// that field being `extensible`. Append-only: never touches
    /// `coreValues`, never removes or renames existing user values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_field_values: Option<Vec<FieldValueAddition>>,
    /// Field renames. An identity rename rekeys the property data of every
    /// existing node of this type together with the schema definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_fields: Option<Vec<FieldRename>>,
    /// Set or change this schema's parent type (ADR-078). Absent leaves the
    /// current parent untouched; there is no way to clear one.
    ///
    /// Re-targeting is validated exactly as creation is: the new parent must
    /// exist, must not introduce a cycle, and must not collide with a field
    /// this schema (or a remaining ancestor) already declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Relationships to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_relationships: Option<Vec<SchemaRelationship>>,
    /// Relationship names to remove. `extends` and `extended_by` are refused
    /// here: the parent can only be re-targeted, through `extends` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_relationships: Option<Vec<String>>,
    /// New description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Make the type abstract (`true`) or concrete (`false`); absent leaves it
    /// unchanged. A type that already has nodes of its own cannot become
    /// abstract: no node may have an abstract type (ADR-086 §6).
    #[serde(default, rename = "abstract", skip_serializing_if = "Option::is_none")]
    pub is_abstract: Option<bool>,
    /// Replace the type's `children` rule (ADR-089); absent leaves it
    /// unchanged. Refused when it relaxes the base type's rule, or when a
    /// node of the type already breaks the new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<SchemaChildrenRule>,
    /// Replace the type's `parent` rule (ADR-089); absent leaves it
    /// unchanged. Refused like `children`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SchemaParentRule>,
    /// Set the title template; absent leaves it unchanged. `{field_name}`
    /// tokens reference fields defined in the schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_template: Option<String>,
    /// Set the properties header summary template; absent leaves it
    /// unchanged. Same `{field_name}` syntax, evaluated by the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties_header_summary_template: Option<String>,
    /// Proceed even if active plays would be affected. When false (the
    /// default), such an update is refused with the affected plays listed.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub force: bool,
}

/// Result of `update_schema`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchemaUpdateOutput {
    pub schema_id: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields_added: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields_removed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields_renamed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_values_added: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationships_added: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationships_removed: Option<usize>,
    /// Plays affected by the change (present when `force` let it through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_plays: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Top-level keys are snake_case; the nested field and relationship
    /// entries keep their camelCase metadata keys.
    #[test]
    fn create_params_decode_from_the_authored_shape() {
        let params: CreateSchemaParams = serde_json::from_value(json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number", "friendlyName": "Amount" }],
            "extends": "document",
            "abstract": true,
            "children": { "rule": "none" },
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }],
            "title_template": "{amount}"
        }))
        .unwrap();

        assert_eq!(params.name, "Invoice");
        assert_eq!(params.fields.as_ref().unwrap()[0].friendly_name, "Amount");
        assert_eq!(params.extends.as_deref(), Some("document"));
        assert!(params.is_abstract);
        assert_eq!(params.children, SchemaChildrenRule::None);
        assert!(params.parent.is_any());
        assert_eq!(params.relationships.unwrap()[0].reverse_name, "invoices");
        assert_eq!(params.title_template.as_deref(), Some("{amount}"));
    }

    #[test]
    fn params_refuse_an_unknown_key() {
        let create = serde_json::from_value::<CreateSchemaParams>(
            json!({ "name": "X", "titleTemplate": "" }),
        )
        .unwrap_err()
        .to_string();
        assert!(create.contains("titleTemplate"), "{create}");

        let update = serde_json::from_value::<UpdateSchemaParams>(
            json!({ "schema_id": "x", "addFields": [] }),
        )
        .unwrap_err()
        .to_string();
        assert!(update.contains("addFields"), "{update}");
    }

    /// An absent key leaves that part of the schema unchanged, so it must
    /// stay absent through a round trip.
    #[test]
    fn update_params_round_trip_without_inventing_changes() {
        let params: UpdateSchemaParams = serde_json::from_value(json!({
            "schema_id": "invoice",
            "rename_fields": [{ "from": "amt", "to": "amount", "friendlyName": "Amount" }],
            "add_field_values": [{ "field": "status", "values": [{ "value": "void", "label": "Void" }] }],
            "title_template": "{amount}"
        }))
        .unwrap();
        assert_eq!(
            params.rename_fields.as_ref().unwrap()[0]
                .friendly_name
                .as_deref(),
            Some("Amount")
        );
        assert!(params.is_abstract.is_none() && params.children.is_none());
        assert!(!params.force);

        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(wire["schema_id"], "invoice");
        assert_eq!(wire["title_template"], "{amount}");
        assert!(wire.get("abstract").is_none());
        assert!(wire.get("children").is_none());
        assert!(wire.get("extends").is_none());
        assert!(wire.get("add_fields").is_none());
    }

    #[test]
    fn outputs_are_camel_case() {
        let created = serde_json::to_value(CreateSchemaOutput {
            schema_id: "invoice".to_string(),
            is_core: false,
            version: 1,
            description: "An invoice".to_string(),
            fields: vec![],
            extends: None,
            relationships: vec![],
            warnings: None,
        })
        .unwrap();
        assert_eq!(created["schemaId"], "invoice");
        assert_eq!(created["isCore"], false);
        assert!(created.get("extends").is_none());
        assert!(created.get("relationships").is_none());
        assert!(created.get("warnings").is_none());

        let updated = serde_json::to_value(SchemaUpdateOutput {
            schema_id: "invoice".to_string(),
            success: true,
            fields_added: Some(2),
            fields_removed: None,
            fields_renamed: None,
            field_values_added: None,
            relationships_added: None,
            relationships_removed: None,
            affected_plays: None,
        })
        .unwrap();
        assert_eq!(updated["fieldsAdded"], 2);
        assert!(updated.get("fieldsRemoved").is_none());
    }
}
