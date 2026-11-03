//! The shape snapshot: what this build ships, as canonical JSON.
//!
//! Generated from the table DDL, the seeded core schemas and the shipped
//! seeds; never edited by hand. The same inputs give the same bytes: object
//! keys are written in sorted order and nothing time- or run-dependent is
//! read (a schema's timestamps and version counter are left out).
//!
//! ```text
//! {
//!   "formatVersion": 1,
//!   "hash":    canonical hash of "tables" and "schemas" (see below),
//!   "tables":  { name: { "columns": [...], "virtual": bool } },
//!   "schemas": { type id: { name, fields{}, relationships{}, extends,
//!                           children, parent, contextPaths, ... } },
//!   "registry": { type id: { parent, abstract, category, contentRole,
//!                            participation, wire, derived, alwaysTitled,
//!                            titleTemplate, structure, ancestry[] } },
//!   "seeded":  { seed id: { key, nodeType, tier, configVersion,
//!                           guidanceVersion } }
//! }
//! ```
//!
//! # The hash
//!
//! The hash identifies a shape from a database's contents alone. It leaves
//! out what a user may legitimately change on a core schema, the same things
//! `db::core_type_shape` leaves out of its comparison: namespaced fields
//! (`custom:`), values added to an extensible enum (`userValues`, and an
//! inherited enum a type redeclares to hold only added values), fields whose
//! protection is `user` (a user may remove or rename them), and context
//! paths. Seeded content is not in the hash, because users edit it, and the
//! registry is not either: it is code, not something a database stores, so it
//! cannot identify what a database holds. The snapshot document still lists
//! all of it: a change to any of it is a change to record.
//!
//! # Known gaps
//!
//! - The seeded `Links` aspect (a skill's `attached_to` links) is not
//!   recorded. Links are seeded by the agent crate, as are skills, tool
//!   descriptions and agent guidance; none of those seeds is listed.
//! - Table coverage is names and columns only. Indexes, views, triggers,
//!   column types and constraints are not listed, so a change confined to one
//!   of them leaves the snapshot as it was. When the DDL gains views (the
//!   resolved structural-rule views, for one), each view's definition needs a
//!   line here.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::markdown::{prepare_nodes_from_template, NodeTemplate};
use crate::models::SchemaNode;
use nodespace_types::{ChildrenRule, CoreNodeType, ParentRule, WireShape};

/// The layout of the snapshot document.
pub const FORMAT_VERSION: u32 = 1;

/// A tables entry as [`crate::db::schema::built_in_tables`] reports it:
/// name, columns, whether the table is virtual.
pub type TableRow = (String, Vec<String>, bool);

/// Fields of a serialized schema the snapshot keeps, beside the field and
/// relationship lists.
const KEPT_SCHEMA_KEYS: [&str; 9] = [
    "isCore",
    "abstract",
    "extends",
    "children",
    "parent",
    "schemaVersion",
    "titleTemplate",
    "propertiesHeaderSummaryTemplate",
    "contextPaths",
];

/// Keys of a serialized schema that are not part of its shape: the node
/// envelope's bookkeeping.
pub const IGNORED_SCHEMA_KEYS: [&str; 8] = [
    "nodeType",
    "version",
    "createdAt",
    "modifiedAt",
    "properties",
    "mentions",
    "mentionedIn",
    "title",
];

/// Every key a serialized schema can carry that this module accounts for:
/// kept, ignored, or turned into the `name` / `fields` / `relationships`
/// entries. A new key on the wire type fails a test until it is placed in one
/// of the lists.
pub fn accounted_schema_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = KEPT_SCHEMA_KEYS.to_vec();
    keys.extend(IGNORED_SCHEMA_KEYS);
    keys.extend([
        "id",
        "content",
        "fields",
        "relationships",
        "lifecycleStatus",
    ]);
    keys
}

/// Writes `value` as canonical JSON: object keys sorted, two-space
/// indentation, a trailing newline. Independent of how the `Value` orders its
/// keys, so the bytes do not depend on a crate feature.
pub fn canonical_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, Some(0), &mut out);
    out.push('\n');
    out
}

/// The same, on one line, for hashing.
pub fn canonical_compact(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, None, &mut out);
    out
}

fn write_value(value: &Value, indent: Option<usize>, out: &mut String) {
    let pad = |level: usize, out: &mut String| {
        out.push('\n');
        out.push_str(&"  ".repeat(level));
    };
    match value {
        Value::Object(map) if !map.is_empty() => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (i, (key, item)) in sorted.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(level) = indent {
                    pad(level + 1, out);
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(item, indent.map(|l| l + 1), out);
            }
            if let Some(level) = indent {
                pad(level, out);
            }
            out.push('}');
        }
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(level) = indent {
                    pad(level + 1, out);
                }
                write_value(item, indent.map(|l| l + 1), out);
            }
            if let Some(level) = indent {
                pad(level, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The tables as the snapshot lists them.
pub fn tables_view(tables: &[TableRow]) -> Value {
    let mut map = Map::new();
    for (name, columns, is_virtual) in tables {
        map.insert(
            name.clone(),
            json!({ "columns": columns, "virtual": is_virtual }),
        );
    }
    Value::Object(map)
}

/// One schema as the snapshot lists it: its own declarations, fields and
/// relationships keyed by name so a difference names what changed.
pub fn schema_view(schema: &SchemaNode) -> Value {
    let serialized = serde_json::to_value(schema).expect("a schema serializes");
    let source = serialized.as_object().expect("a schema is an object");
    let mut view = Map::new();
    view.insert(
        "name".to_string(),
        source.get("content").cloned().unwrap_or(Value::Null),
    );
    for key in KEPT_SCHEMA_KEYS {
        if let Some(value) = source.get(key) {
            view.insert(key.to_string(), value.clone());
        }
    }
    view.entry("abstract").or_insert(json!(false));
    view.entry("contextPaths").or_insert(json!([]));

    let mut fields = Map::new();
    for field in source
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = field.get("name").and_then(Value::as_str).unwrap_or("");
        fields.insert(name.to_string(), field.clone());
    }
    view.insert("fields".to_string(), Value::Object(fields));

    let mut relationships = Map::new();
    for rel in source
        .get("relationships")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        relationships.insert(relationship_key(rel), rel.clone());
    }
    view.insert("relationships".to_string(), Value::Object(relationships));
    Value::Object(view)
}

/// A relationship declaration's identity: its name, target and direction.
fn relationship_key(rel: &Value) -> String {
    let text = |key: &str| {
        rel.get(key)
            .map(|v| v.as_str().map_or(v.to_string(), str::to_string))
    };
    format!(
        "{}->{}/{}",
        text("name").unwrap_or_default(),
        text("targetType").unwrap_or_default(),
        text("direction").unwrap_or_default()
    )
}

/// The schemas as the snapshot lists them, by type id.
pub fn schemas_view(schemas: &[SchemaNode]) -> Value {
    Value::Object(
        schemas
            .iter()
            .map(|s| (s.envelope.id.clone(), schema_view(s)))
            .collect(),
    )
}

fn type_ids(types: &[CoreNodeType]) -> Value {
    json!(types.iter().map(|t| t.as_str()).collect::<Vec<_>>())
}

/// The type registry as the snapshot lists it, by type id: what the registry
/// declares for each core type (the `schema` meta-type included), and the
/// `extends` chain the ancestry and structural-rule rows are derived from.
pub fn registry_view() -> Value {
    let mut types = Map::new();
    for core in CoreNodeType::ALL {
        let info = core.info();
        let children = match info.structure.children {
            ChildrenRule::Any => json!("any"),
            ChildrenRule::None => json!("none"),
            ChildrenRule::AnyExcept(except) => json!({ "anyExcept": type_ids(except) }),
        };
        let parent = match info.structure.parent {
            ParentRule::Any => json!("any"),
            ParentRule::MustBeRoot => json!("must_be_root"),
            ParentRule::MustHaveParentOf(of) => json!({ "mustHaveParentOf": type_ids(of) }),
        };
        let wire = match info.wire {
            WireShape::Envelope => json!("envelope"),
            WireShape::Generic => json!("generic"),
            WireShape::Typed { update } => json!({ "typed": { "update": update } }),
        };
        types.insert(
            info.id.to_string(),
            json!({
                "parent": info.parent.map(|p| p.as_str()),
                "abstract": info.is_abstract,
                "category": serde_json::to_value(info.category).expect("a category serializes"),
                "contentRole": serde_json::to_value(info.content_role).expect("a role serializes"),
                "participation": {
                    "embedded": info.participation.embedded,
                    "mentionable": info.participation.mentionable,
                    "excludedFromDefaultQueries": info.participation.excluded_from_default_queries,
                },
                "wire": wire,
                "derived": info.derived.iter().map(|d| d.name()).collect::<Vec<_>>(),
                "alwaysTitled": info.always_titled,
                "titleTemplate": info.title_template,
                "structure": { "children": children, "parent": parent },
                "ancestry": core.chain().into_iter().map(|t| t.as_str()).collect::<Vec<_>>(),
            }),
        );
    }
    Value::Object(types)
}

/// The shipped seeds as the snapshot lists them, by seed id: what identifies
/// each and the two versions reconciliation compares.
pub fn seeded_view(templates: &[NodeTemplate]) -> Result<Value> {
    let mut seeded = Map::new();
    for template in templates {
        let nodes = prepare_nodes_from_template(template)
            .with_context(|| format!("seed '{}' does not expand", template.title))?;
        let seed = nodes[0]
            .properties
            .get("_seed")
            .context("an expanded seed carries its reconciliation block")?;
        let version = |key: &str| seed.get(key).cloned().unwrap_or(Value::Null);
        seeded.insert(
            template.id.clone(),
            json!({
                "key": template.title,
                "nodeType": template.root_node_type,
                "tier": template.tier.as_str(),
                "configVersion": version("config_version"),
                "guidanceVersion": version("guidance_version"),
            }),
        );
    }
    Ok(Value::Object(seeded))
}

/// Whether `field` is one a user may have produced or removed on this type:
/// left out of the hash.
///
/// The last case is what the core-type shape check reads as a user's
/// extension of an inherited enum: the field is declared again on the type
/// with the inherited field's type and no core values, and the type inherits
/// an extensible enum of that name.
fn is_user_territory(field: &Value, inherited: &[&Value]) -> bool {
    let name = field.get("name").and_then(Value::as_str).unwrap_or("");
    if name.contains(':') {
        return true;
    }
    if field.get("protection").and_then(Value::as_str) == Some("user") {
        return true;
    }
    let holds_core_values = field
        .get("coreValues")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty());
    !holds_core_values
        && field.get("type").and_then(Value::as_str) == Some("enum")
        && inherited.iter().any(|from| {
            from.get("name").and_then(Value::as_str) == Some(name)
                && from.get("type").and_then(Value::as_str) == Some("enum")
                && from.get("extensible").and_then(Value::as_bool) == Some(true)
                && from.get("itemType") == field.get("itemType")
        })
}

fn strip_user_values(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("userValues");
            map.values_mut().for_each(strip_user_values);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_user_values),
        _ => {}
    }
}

/// A schema view with what a user may change taken out. `inherited` is the
/// fields the schema inherits through `extends`.
fn hashed_schema_view(view: &Value, inherited: &[&Value]) -> Value {
    let mut view = view.clone();
    let Some(object) = view.as_object_mut() else {
        return view;
    };
    object.remove("contextPaths");
    if let Some(fields) = object.get_mut("fields").and_then(Value::as_object_mut) {
        fields.retain(|_, field| !is_user_territory(field, inherited));
        fields.values_mut().for_each(strip_user_values);
    }
    if let Some(rels) = object
        .get_mut("relationships")
        .and_then(Value::as_object_mut)
    {
        rels.retain(|_, rel| {
            !rel.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains(':'))
        });
    }
    view
}

/// The fields `view` inherits through its `extends` chain, as `schemas` lists
/// its ancestors.
fn inherited_fields<'a>(schemas: &'a Value, view: &Value) -> Vec<&'a Value> {
    let mut fields = Vec::new();
    let mut parent = view.get("extends").and_then(Value::as_str);
    // A chain is short; the bound only guards a cycle in a document.
    for _ in 0..64 {
        let Some(ancestor) = parent.and_then(|id| schemas.get(id)) else {
            break;
        };
        fields.extend(
            ancestor
                .get("fields")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|f| f.values()),
        );
        parent = ancestor.get("extends").and_then(Value::as_str);
    }
    fields
}

/// The canonical hash of a shape: the tables and the schemas, with what a user
/// may change taken out. `schemas` is a [`schemas_view`].
pub fn shape_hash(tables: &Value, schemas: &Value) -> String {
    let hashed_schemas: Map<String, Value> = schemas
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, view)| {
            (
                id.clone(),
                hashed_schema_view(view, &inherited_fields(schemas, view)),
            )
        })
        .collect();
    let whole = json!({ "tables": tables, "schemas": Value::Object(hashed_schemas) });
    format!("{:x}", Sha256::digest(canonical_compact(&whole).as_bytes()))
}

/// Puts the pieces together: the snapshot document, hash included.
pub fn assemble(
    tables: &[TableRow],
    schemas: &[SchemaNode],
    registry: Value,
    seeded: Value,
) -> Value {
    let tables = tables_view(tables);
    let schemas = schemas_view(schemas);
    json!({
        "formatVersion": FORMAT_VERSION,
        "hash": shape_hash(&tables, &schemas),
        "tables": tables,
        "schemas": schemas,
        "registry": registry,
        "seeded": seeded,
    })
}

/// The seeds this crate ships: the core Plays and the core saved queries. The
/// skills, tool descriptions and agent guidance are seeded by the agent crate
/// and are not listed here.
pub fn core_seed_templates() -> Vec<NodeTemplate> {
    crate::playbook::core_plays::core_play_templates()
        .into_iter()
        .chain(crate::services::query_service::core_queries::core_query_templates())
        .collect()
}

/// The snapshot of this build, as the canonical text committed in
/// `shape-record/current.json`.
pub async fn generate() -> Result<String> {
    let tables = crate::db::schema::built_in_tables().await?;
    let seeded = seeded_view(&core_seed_templates())?;
    let snapshot = assemble(
        &tables,
        &crate::models::core_schemas::get_core_schemas(),
        registry_view(),
        seeded,
    );
    Ok(canonical_pretty(&snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::core_schemas::get_core_schemas;
    use crate::models::schema::{EnumValue, SchemaField, SchemaFieldType};

    #[test]
    fn canonical_text_sorts_keys_and_is_stable() {
        let a = json!({ "b": [1, { "z": 1, "a": 2 }], "a": {} });
        let b = json!({ "a": {}, "b": [1, { "a": 2, "z": 1 }] });
        assert_eq!(canonical_pretty(&a), canonical_pretty(&b));
        assert_eq!(canonical_compact(&a), r#"{"a":{},"b":[1,{"a":2,"z":1}]}"#);
        assert!(canonical_pretty(&a).ends_with("}\n"));
    }

    /// Two runs give the same bytes, and the bytes do not depend on when the
    /// schemas were built (their timestamps differ between runs).
    #[tokio::test]
    async fn generating_twice_gives_the_same_bytes() {
        let first = generate().await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = generate().await.unwrap();
        assert_eq!(first, second);
        let parsed: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(canonical_pretty(&parsed), first, "the output is canonical");
    }

    /// Every key a core schema serializes with is accounted for, so a field
    /// added to the wire type cannot be silently left out of the snapshot.
    #[test]
    fn every_serialized_schema_key_is_accounted_for() {
        let known = accounted_schema_keys();
        for schema in get_core_schemas() {
            let value = serde_json::to_value(&schema).unwrap();
            for key in value.as_object().unwrap().keys() {
                assert!(
                    known.contains(&key.as_str()),
                    "schema '{}' serializes a key '{key}' the snapshot does not place",
                    schema.envelope.id
                );
            }
        }
    }

    fn hash_of(schemas: &[SchemaNode]) -> String {
        shape_hash(&json!({}), &schemas_view(schemas))
    }

    fn task() -> SchemaNode {
        get_core_schemas()
            .into_iter()
            .find(|s| s.envelope.id == "task")
            .unwrap()
    }

    /// What a user may do to a core schema does not change the hash: a
    /// namespaced field, a value added to an extensible enum, a removed
    /// `user` field, a changed context path.
    #[test]
    fn the_hash_ignores_what_a_user_may_change() {
        let base = vec![task()];
        let before = hash_of(&base);

        let mut edited = task();
        let mut namespaced: SchemaField = edited.fields[0].clone();
        namespaced.name = "custom:estimate".to_string();
        namespaced.field_type = SchemaFieldType::Text;
        namespaced.core_values = None;
        edited.fields.push(namespaced);
        for field in &mut edited.fields {
            if field.name == "status" {
                assert!(field.extensible == Some(true), "task status is extensible");
                field
                    .user_values
                    .get_or_insert_with(Vec::new)
                    .push(EnumValue::new("parked".to_string(), "Parked".to_string()));
            }
        }
        edited.fields.retain(|f| f.name != "priority");
        edited.context_paths.clear();
        assert_eq!(hash_of(&[edited]), before);
    }

    /// What no user can do does change it.
    #[test]
    fn the_hash_sees_a_change_a_user_cannot_make() {
        let before = hash_of(&[task()]);

        let mut core_value_added = task();
        let status = core_value_added
            .fields
            .iter_mut()
            .find(|f| f.name == "status")
            .unwrap();
        status
            .core_values
            .as_mut()
            .unwrap()
            .push(EnumValue::new("parked".to_string(), "Parked".to_string()));
        assert_ne!(hash_of(&[core_value_added]), before);

        let mut bare_field = task();
        let mut extra = bare_field.fields[0].clone();
        extra.name = "estimate".to_string();
        bare_field.fields.push(extra);
        assert_ne!(hash_of(&[bare_field]), before);

        let mut removed_core_field = task();
        removed_core_field.fields.retain(|f| f.name != "status");
        assert_ne!(hash_of(&[removed_core_field]), before);

        assert_ne!(
            shape_hash(&json!({ "t": 1 }), &json!({})),
            shape_hash(&json!({}), &json!({}))
        );
    }

    /// An issue holds the inherited `priority` only as an enum a user
    /// extended: redeclared with no core values.
    #[test]
    fn an_inherited_enum_a_user_extended_is_left_out_of_the_hash() {
        let issue = get_core_schemas()
            .into_iter()
            .find(|s| s.envelope.id == "issue")
            .unwrap();
        let before = hash_of(&[task(), issue.clone()]);
        let mut extended = issue.clone();
        let mut priority = task()
            .fields
            .into_iter()
            .find(|f| f.name == "priority")
            .unwrap();
        priority.core_values = None;
        priority.protection = crate::models::SchemaProtectionLevel::Core;
        priority.extensible = Some(true);
        priority.user_values = Some(vec![EnumValue::new(
            "urgent".to_string(),
            "Urgent".to_string(),
        )]);
        extended.fields.retain(|f| f.name != "priority");
        extended.fields.push(priority);
        assert_eq!(hash_of(&[task(), extended]), before);
    }

    #[test]
    fn the_hash_recorded_in_a_snapshot_matches_its_content() {
        let tables = tables_view(&[("node".to_string(), vec!["id".to_string()], false)]);
        let snapshot = assemble(
            &[("node".to_string(), vec!["id".to_string()], false)],
            &[task()],
            json!({}),
            json!({}),
        );
        assert_eq!(
            snapshot["hash"].as_str().unwrap(),
            shape_hash(&tables, &snapshot["schemas"])
        );
    }

    #[test]
    fn seeded_view_lists_each_seed_with_its_versions() {
        let seeded = seeded_view(&core_seed_templates()).unwrap();
        let entries = seeded.as_object().unwrap();
        assert!(!entries.is_empty());
        for (id, entry) in entries {
            assert!(uuid::Uuid::parse_str(id).is_ok());
            assert!(entry["configVersion"].is_string(), "{id}");
            assert!(entry["guidanceVersion"].is_string(), "{id}");
        }
    }

    /// A field declared as an extensible enum with no core values is left out
    /// of the hash only where it extends an enum the type inherits, as the
    /// core-type shape check reads it. A type that inherits no such field
    /// does not get to shed one.
    #[test]
    fn an_extensible_enum_without_core_values_is_left_out_only_when_inherited() {
        let decision = get_core_schemas()
            .into_iter()
            .find(|s| s.envelope.id == "decision")
            .unwrap();
        let before = hash_of(std::slice::from_ref(&decision));
        let mut stray = decision.clone();
        let mut field = task()
            .fields
            .into_iter()
            .find(|f| f.name == "priority")
            .unwrap();
        field.name = "urgency".to_string();
        field.core_values = None;
        field.protection = crate::models::SchemaProtectionLevel::Core;
        field.extensible = Some(true);
        stray.fields.push(field);
        assert_ne!(hash_of(&[stray]), before);
    }

    /// The registry section lists every core type, the `schema` meta-type
    /// among them, with what the registry records for it.
    #[test]
    fn the_registry_view_lists_every_core_type() {
        let registry = registry_view();
        let types = registry.as_object().unwrap();
        for core in nodespace_types::CoreNodeType::ALL {
            assert!(types.contains_key(core.as_str()), "{}", core.as_str());
        }
        assert_eq!(types.len(), nodespace_types::CoreNodeType::ALL.len());
        assert!(types.contains_key("schema"));
        let checkbox = &registry["checkbox"];
        assert_eq!(checkbox["derived"], json!(["checked"]));
        assert_eq!(checkbox["contentRole"], "body");
        assert_eq!(checkbox["participation"]["embedded"], true);
        assert_eq!(
            registry["ai-chat-message"]["participation"]["embedded"],
            false
        );
        assert_eq!(registry["issue"]["parent"], "task");
        assert_eq!(registry["issue"]["ancestry"], json!(["issue", "task"]));
        assert_eq!(registry["tool"]["abstract"], true);
        assert!(registry["text"]["wire"].is_string() || registry["text"]["wire"].is_object());
        assert!(
            registry["text"]["structure"]["children"].is_string()
                || registry["text"]["structure"]["children"].is_object()
        );
    }
}
