use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::helpers::default_version;
use crate::node::Node;

fn default_schema_version() -> u32 {
    1
}

/// Derive a display label for a field whose `friendlyName` was omitted at
/// `create_schema`/`update_schema` time, e.g. `due_date` -> `Due date`,
/// `custom:capacity` -> `Capacity`, `restrictedToMembers` -> `Restricted to
/// members`, `employeeIDNumber` -> `Employee id number`.
///
/// Namespace prefixes (`custom:`, `org:`, `plugin:`, ...) are stripped before
/// humanizing — a display-only operation with no effect on the stored name.
/// This is reachable *today*, not just as a future hazard: adding
/// `custom:status` next to an existing core `status` field derives "Status"
/// for both, since the two have different storage keys but the same
/// stripped/humanized text. This function does not resolve that on its own —
/// callers (`apply_friendly_name_defaults` in `packages/core/src/schema/mod.rs`)
/// are responsible for disambiguating a derived value that collides with
/// another field already in the schema.
pub fn derive_friendly_name(name: &str) -> String {
    let base = name.rsplit(':').next().unwrap_or(name);
    let chars: Vec<char> = base.chars().collect();

    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }

        // A word boundary precedes `ch` when either:
        // - the previous char is lowercase/digit and this one is uppercase
        //   (`dueX` -> `due|X`), or
        // - this is the last uppercase letter of an acronym run immediately
        //   followed by a lowercase letter (`IDNumber` -> `ID|Number`, not
        //   `IDN|umber`) — without this second rule, an acronym directly
        //   adjacent to the next word (`employeeIDNumber`) merges into one
        //   unsplit blob instead of three words.
        let boundary = match chars.get(i.wrapping_sub(1)) {
            Some(&prev) if i > 0 => {
                let prev_lower = prev.is_lowercase() || prev.is_ascii_digit();
                let cur_upper = ch.is_uppercase();
                let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
                (prev_lower && cur_upper) || (prev.is_uppercase() && cur_upper && next_lower)
            }
            _ => false,
        };

        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }

    if words.is_empty() {
        return name.to_string();
    }

    words
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let lower = w.to_lowercase();
            if i == 0 {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => lower,
                }
            } else {
                lower
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnumValue {
    pub value: String,
    pub label: String,
    /// For a value added to a field this schema *inherited* via `extends`:
    /// which pre-existing value it collapses to at an ancestor's scope
    /// (ADR-078).
    ///
    /// An `issue` extending `task` may add `backlog` to the inherited
    /// `status`, mapping to `todo`. A consumer reading at `task`'s scope — a
    /// Play condition, a query filter, any CEL expression written against the
    /// base type — then sees `todo`, the value it was written to understand,
    /// rather than a `backlog` it has never heard of. Jira's status-category
    /// model is the direct precedent.
    ///
    /// Required on every value appended to an inherited field, and neither
    /// required nor meaningful on a field the schema declares itself: an own
    /// field has no ancestor scope whose meaning needs preserving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maps_to: Option<String>,
}

impl EnumValue {
    /// An enum value with no `maps_to` — the shape every value on a schema's
    /// own field takes, and what the core schema definitions construct.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            maps_to: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SchemaProtectionLevel {
    Core,
    #[default]
    User,
    System,
}

impl std::fmt::Display for SchemaProtectionLevel {
    /// Mirrors the wire form (`#[serde(rename_all = "lowercase")]`) so error
    /// messages naming a protection level match what the same value
    /// serializes to over the API.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SchemaProtectionLevel::Core => "core",
            SchemaProtectionLevel::User => "user",
            SchemaProtectionLevel::System => "system",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaField {
    /// Unique key within the schema: storage/query key, CEL selector,
    /// titleTemplate token. Changing it is a breaking change to every call
    /// site that references the field.
    pub name: String,
    /// Display label shown in every UI surface (table/kanban headers, query
    /// editor, property forms). Always populated in storage — every reader
    /// uses it unconditionally, with no fallback to `description` and no
    /// null-branching. Not required on input to `create_schema`/
    /// `update_schema`: when omitted (empty string), the write boundary
    /// derives it from `name` via [`derive_friendly_name`] before the field
    /// is persisted.
    #[serde(default)]
    pub friendly_name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub protection: SchemaProtectionLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<EnumValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_values: Option<Vec<EnumValue>>,
    #[serde(default)]
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// What the field is for: meaning, purpose, usage, an example where
    /// helpful. Consumed by the model for schema comprehension (schema
    /// retrieval embeds this text) — NOT rendered as a UI label. Prefer more
    /// detail over less; there is no UI-brevity cost to a longer description
    /// now that [`SchemaField::friendly_name`] carries the display label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SchemaField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_fields: Option<Vec<SchemaField>>,
    /// Marks this field as a uniqueness hint: values are expected to be unique
    /// among active nodes of the same type. This is a suggest-don't-block rule,
    /// not an enforced constraint — writes are never rejected on a collision
    /// (two offline devices can each validly create the same value). Uniqueness
    /// is scoped per-database (ADR-053) and surfaced via a read-only lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique: Option<bool>,
    /// When paired with `unique`, compares values case-insensitively (e.g. an
    /// email is a claim, not an identity key, and casing should not distinguish
    /// two otherwise-identical claims).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_case_insensitive: Option<bool>,
    /// Marks this property as machine-bound. A `localOnly` property is persisted
    /// and read locally like any other — normal for local reads, writes, and the
    /// UI — but is never included in a sync push, and is ignored if it arrives in
    /// a pull. It survives its own device's restarts and is simply absent on other
    /// devices (never a stale value from elsewhere). Use it when a value denotes
    /// state on a particular machine, such that transporting it means nothing or
    /// something false elsewhere (a resume handle, an absolute path, a device id,
    /// a local port), or when the content is not safe to transport as-is. Enforced
    /// by the sync engine, which consults this classification when building the
    /// push payload and when applying a pull.
    #[serde(default, skip_serializing_if = "is_false")]
    pub local_only: bool,
}

/// Serde `skip_serializing_if` helper: omit a `bool` field when it is `false`,
/// so the flag only appears in serialized schemas where it is actually set.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EdgeField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    /// The closed set of values an `enum` edge field admits, each with a display
    /// label. Required for `field_type == "enum"` and rejected on any other type.
    ///
    /// Deliberately narrower than [`SchemaField`], which also carries
    /// `user_values` and `extensible`: an edge enum is a fixed vocabulary. The
    /// motivating case is an access-control role on an edge (owner/editor/viewer),
    /// where a user-extensible value set would mean a permission level nothing
    /// downstream knows how to check. Add the extensible half only if a real
    /// use case for it appears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<EnumValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RelationshipDirection {
    Out,
    In,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipCardinality {
    One,
    Many,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaRelationship {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_type: Option<String>,
    pub direction: RelationshipDirection,
    pub cardinality: RelationshipCardinality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// The name this edge reads by from the target's end. REQUIRED.
    ///
    /// A relationship is declared once — the storage model keeps a single
    /// `relationship` row between the two schema nodes — but it is read from
    /// both ends. Leaving this unset left that one stored edge only
    /// half-declared: the target's side fell back to a synthesized
    /// `"{SourceType} ({Relationship Name})"` label, so an invoice declaring
    /// `billed_to → customer` surfaced on the customer as
    /// "Invoice (Customer)" rather than "Invoices".
    ///
    /// Naming the inverse is a modeling decision only the author can make, so
    /// it is required rather than derived. See the type-level note on
    /// [`SchemaRelationship::reverse_cardinality`] for why both live in the
    /// type rather than in a validator alone.
    pub reverse_name: String,
    /// The cardinality governing the target's end — how many sources may point
    /// at one target. REQUIRED, and the counterpart to
    /// [`SchemaRelationship::reverse_name`].
    ///
    /// Unset, the inbound group carried no cardinality at all, so nothing
    /// downstream could reason about how many sources may point at a node.
    ///
    /// Carrying both halves in the type — not merely in a validator — is what
    /// makes "every stored edge is named from both ends" an invariant every
    /// reader can rely on instead of a convention that holds only on the paths
    /// which happen to route through validation. `handle_create_schema` and
    /// `handle_update_schema` reject a payload missing either field before
    /// serde sees it, so a caller gets an actionable error naming the field
    /// rather than a bare `missing field` message.
    pub reverse_cardinality: RelationshipCardinality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_fields: Option<Vec<EdgeField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaNode {
    pub id: String,
    pub content: String,
    #[serde(default = "default_version")]
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    #[serde(default)]
    pub is_core: bool,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<SchemaField>,
    #[serde(default)]
    pub relationships: Vec<SchemaRelationship>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties_header_summary_template: Option<String>,
}

/// Parses the `fields` array out of a schema node's stored properties.
///
/// A parse failure (malformed/legacy field JSON) is intentionally NOT
/// surfaced by widening `from_node`'s `Result`. `from_node` has three
/// production callers: `node_to_typed_value` — reachable from every entry
/// point (Tauri commands, MCP, HTTP) via `nodes_to_typed_values`, which
/// `.collect()`s a `Vec<Result<_, _>>` into a single `Result<Vec<_>, _>` —
/// and two direct call sites in
/// `desktop-app/src-tauri/src/commands/schemas.rs`
/// (`get_all_schemas`/`get_schema_definition`), which do not go through
/// `nodes_to_typed_values` at all. Only the `node_to_typed_value` path risks
/// a blast-radius problem: turning this into an `Err` there would fail an
/// entire unrelated batch read over one bad schema node. Fixing the
/// diagnostic here, inside `from_node` itself, covers all three callers
/// uniformly without touching any of their signatures or `nodes_to_typed_values`'s
/// batch-collect behavior.
///
/// `nodespace-types` deliberately carries no logging dependency (see the
/// crate-level doc comment), so the diagnostic is plain text for the caller
/// to print via `eprintln!` — the closest thing to "a log line" available
/// without pulling in `tracing`/`log`, and one that fires the same way no
/// matter which binary (Tauri app, daemon, CLI) embeds this crate. Note this
/// is not airtight on every platform: on Windows, the daemon process is
/// spawned with its stderr piped to `/dev/null`-equivalent, and a release
/// desktop-app build has no console at all, so this specific diagnostic is
/// currently invisible there (both platforms' stdout/stderr routing is a
/// pre-existing daemon/app-launch concern, out of scope for this fix — see
/// the follow-up issue tracking it).
fn parse_fields(
    properties: &serde_json::Value,
    node_id: &str,
) -> (Vec<SchemaField>, Option<String>) {
    match properties.get("fields") {
        None => (Vec::new(), None),
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(fields) => (fields, None),
            Err(e) => (
                Vec::new(),
                Some(format!(
                    "nodespace-types: SchemaNode::from_node: failed to parse `fields` for \
                     schema node `{node_id}`: {e} — reading back as an empty field list. \
                     Likely a stale/corrupted storage format."
                )),
            ),
        },
    }
}

/// Parses the `relationships` array out of a schema node's stored properties.
///
/// Mirrors `parse_fields` above — same silent-swallow shape, same fix, same
/// reasoning for keeping `from_node`'s `Result` untouched (see `parse_fields`'s
/// doc comment for the full blast-radius analysis of `nodes_to_typed_values`'s
/// batch-collect semantics, which applies identically here).
fn parse_relationships(
    properties: &serde_json::Value,
    node_id: &str,
) -> (Vec<SchemaRelationship>, Option<String>) {
    match properties.get("relationships") {
        None => (Vec::new(), None),
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(relationships) => (relationships, None),
            Err(e) => (
                Vec::new(),
                Some(format!(
                    "nodespace-types: SchemaNode::from_node: failed to parse `relationships` for \
                     schema node `{node_id}`: {e} — reading back as an empty relationship list. \
                     Likely a stale/corrupted storage format."
                )),
            ),
        },
    }
}

impl SchemaNode {
    pub fn from_node(node: Node) -> Result<Self, String> {
        if node.node_type != "schema" {
            return Err(format!("Expected 'schema', got '{}'", node.node_type));
        }

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

        let description = node
            .properties
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let (fields, fields_diagnostic) = parse_fields(&node.properties, &node.id);
        if let Some(msg) = &fields_diagnostic {
            eprintln!("{msg}");
        }

        let (relationships, relationships_diagnostic) =
            parse_relationships(&node.properties, &node.id);
        if let Some(msg) = &relationships_diagnostic {
            eprintln!("{msg}");
        }

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
            description,
            fields,
            relationships,
            title_template,
            properties_header_summary_template,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_field() -> SchemaField {
        SchemaField {
            name: "status".to_string(),
            friendly_name: "Status".to_string(),
            field_type: "enum".to_string(),
            protection: SchemaProtectionLevel::Core,
            local_only: false,
            core_values: Some(vec![
                EnumValue::new("open".to_string(), "Open".to_string()),
                EnumValue::new("done".to_string(), "Done".to_string()),
            ]),
            user_values: Some(vec![EnumValue::new(
                "blocked".to_string(),
                "Blocked".to_string(),
            )]),
            indexed: true,
            required: Some(true),
            extensible: Some(true),
            default: Some(json!("open")),
            description: Some("Task status".to_string()),
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
        }
    }

    #[test]
    fn test_schema_field_serialization() {
        let field = create_test_field();
        let json = serde_json::to_value(&field).unwrap();

        assert_eq!(json["name"], "status");
        assert_eq!(json["friendlyName"], "Status");
        assert_eq!(json["protection"], "core");
        // field_type serializes to "type" due to #[serde(rename = "type")]
        assert_eq!(json["type"], "enum");
        // core_values serializes to coreValues
        assert!(json["coreValues"].is_array());
        assert_eq!(json["indexed"], true);
    }

    #[test]
    fn test_schema_field_friendly_name_defaults_to_empty_when_omitted() {
        // The write boundary (create_schema/update_schema) is the only place
        // friendly_name gets derived from `name` — the bare wire type accepts
        // an absent friendlyName as "" so a caller (including the agent) is
        // never forced to supply it, and never rejected for omitting it.
        let json = json!({
            "name": "due_date",
            "type": "date",
        });
        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.friendly_name, "");
    }

    #[test]
    fn test_schema_field_friendly_name_round_trips() {
        let json = json!({
            "name": "due_date",
            "friendlyName": "Due date",
            "type": "date",
        });
        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.friendly_name, "Due date");

        let out = serde_json::to_value(&field).unwrap();
        assert_eq!(out["friendlyName"], "Due date");
    }

    #[test]
    fn test_derive_friendly_name_snake_case() {
        assert_eq!(derive_friendly_name("due_date"), "Due date");
        assert_eq!(derive_friendly_name("started_at"), "Started at");
        assert_eq!(derive_friendly_name("status"), "Status");
    }

    #[test]
    fn test_derive_friendly_name_strips_namespace_prefix() {
        // ADR-063: a prefix only ever appears on a field added to a core
        // type; stripping it for display cannot collide with the storage key
        // of a bare core field (see derive_friendly_name's doc comment).
        assert_eq!(derive_friendly_name("custom:capacity"), "Capacity");
        assert_eq!(derive_friendly_name("org:cost_center"), "Cost center");
    }

    #[test]
    fn test_derive_friendly_name_splits_camel_case() {
        assert_eq!(
            derive_friendly_name("restrictedToMembers"),
            "Restricted to members"
        );
    }

    #[test]
    fn test_derive_friendly_name_hyphenated() {
        assert_eq!(
            derive_friendly_name("capture-session-id"),
            "Capture session id"
        );
    }

    #[test]
    fn test_derive_friendly_name_empty_falls_back_to_raw_name() {
        assert_eq!(derive_friendly_name(""), "");
        assert_eq!(derive_friendly_name(":"), ":");
    }

    #[test]
    fn test_derive_friendly_name_prefix_with_empty_base_falls_back_to_raw_name() {
        // Unreachable in practice (the field-name validator rejects an empty
        // bare segment before this ever runs), but pinned explicitly so the
        // fallback behavior is documented rather than incidental.
        assert_eq!(derive_friendly_name("custom:"), "custom:");
    }

    #[test]
    fn test_derive_friendly_name_splits_acronym_adjacent_to_next_word() {
        // The classic "XMLHttpRequest" splitting case: an acronym run
        // (`ID`) directly followed by another capitalized word (`Number`)
        // must not merge into one unsplit blob ("Idnumber").
        assert_eq!(
            derive_friendly_name("employeeIDNumber"),
            "Employee id number"
        );
        assert_eq!(derive_friendly_name("userIDStatus"), "User id status");
    }

    #[test]
    fn test_derive_friendly_name_all_uppercase_is_treated_as_one_word() {
        // No lowercase run anywhere to anchor a boundary against, so this is
        // one word, sentence-cased like every other single-word input.
        assert_eq!(derive_friendly_name("URL"), "Url");
    }

    #[test]
    fn test_schema_field_deserialization() {
        let json = json!({
            "name": "status",
            "type": "enum",
            "protection": "core",
            "coreValues": [
                { "value": "open", "label": "Open" },
                { "value": "done", "label": "Done" }
            ],
            "indexed": true
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "status");
        assert_eq!(field.field_type, "enum");
        assert_eq!(field.protection, SchemaProtectionLevel::Core);
        assert!(field.indexed);

        let core_values = field.core_values.unwrap();
        assert_eq!(core_values.len(), 2);
        assert_eq!(core_values[0].value, "open");
        assert_eq!(core_values[0].label, "Open");
    }

    #[test]
    fn test_schema_field_rejects_snake_case_core_values() {
        // core_values is the Rust field name; the wire key is coreValues
        // (rename_all = "camelCase"). A payload using the snake_case name must
        // be rejected outright, not silently dropped as an unknown field.
        let json = json!({
            "name": "status",
            "type": "enum",
            "core_values": [
                { "value": "open", "label": "Open" },
                { "value": "done", "label": "Done" }
            ]
        });

        let err = serde_json::from_value::<SchemaField>(json).unwrap_err();
        assert!(
            err.to_string().contains("core_values"),
            "expected error naming the unknown field `core_values`, got: {}",
            err
        );
    }

    #[test]
    fn test_protection_level_serialization() {
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::Core).unwrap(),
            "core"
        );
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::User).unwrap(),
            "user"
        );
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::System).unwrap(),
            "system"
        );
    }

    #[test]
    fn test_nested_field_serialization() {
        let address_field = SchemaField {
            name: "address".to_string(),
            friendly_name: "Address".to_string(),
            field_type: "object".to_string(),
            protection: SchemaProtectionLevel::User,
            local_only: false,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Address information".to_string()),
            item_type: None,
            fields: Some(vec![
                SchemaField {
                    name: "street".to_string(),
                    friendly_name: "Street".to_string(),
                    field_type: "string".to_string(),
                    protection: SchemaProtectionLevel::User,
                    local_only: false,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Street address".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "city".to_string(),
                    friendly_name: "City".to_string(),
                    field_type: "string".to_string(),
                    protection: SchemaProtectionLevel::User,
                    local_only: false,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("City".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ]),
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
        };

        let json = serde_json::to_value(&address_field).unwrap();
        assert_eq!(json["name"], "address");
        assert_eq!(json["type"], "object");
        assert_eq!(json["fields"][0]["name"], "street");
        assert_eq!(json["fields"][1]["name"], "city");
        assert_eq!(json["fields"][1]["indexed"], true);
    }

    #[test]
    fn test_nested_field_deserialization() {
        let json = json!({
            "name": "address",
            "type": "object",
            "protection": "user",
            "indexed": false,
            "fields": [
                {
                    "name": "city",
                    "type": "string",
                    "protection": "user",
                    "indexed": true
                }
            ]
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "address");
        assert_eq!(field.field_type, "object");

        let nested_fields = field.fields.as_ref().unwrap();
        assert_eq!(nested_fields.len(), 1);
        assert_eq!(nested_fields[0].name, "city");
        assert!(nested_fields[0].indexed);
    }

    #[test]
    fn test_array_of_objects_serialization() {
        let contacts_field = SchemaField {
            name: "contacts".to_string(),
            friendly_name: "Contacts".to_string(),
            field_type: "array".to_string(),
            protection: SchemaProtectionLevel::User,
            local_only: false,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Contact list".to_string()),
            item_type: Some("object".to_string()),
            fields: None,
            item_fields: Some(vec![SchemaField {
                name: "email".to_string(),
                friendly_name: "Email".to_string(),
                field_type: "string".to_string(),
                protection: SchemaProtectionLevel::User,
                local_only: false,
                core_values: None,
                user_values: None,
                indexed: true,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Email address".to_string()),
                item_type: None,
                fields: None,
                item_fields: None,
                unique: None,
                unique_case_insensitive: None,
            }]),
            unique: None,
            unique_case_insensitive: None,
        };

        let json = serde_json::to_value(&contacts_field).unwrap();
        assert_eq!(json["name"], "contacts");
        assert_eq!(json["type"], "array");
        // item_type serializes to itemType with camelCase
        assert_eq!(json["itemType"], "object");
        // item_fields serializes to itemFields with camelCase
        assert_eq!(json["itemFields"][0]["name"], "email");
        assert_eq!(json["itemFields"][0]["indexed"], true);
    }

    #[test]
    fn test_edge_field_serialization() {
        let field = EdgeField {
            name: "role".to_string(),
            field_type: "string".to_string(),
            core_values: None,
            indexed: Some(true),
            required: Some(false),
            default: Some(json!("member")),
            target_type: None,
            description: Some("Assignment role".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["name"], "role");
        assert_eq!(json["type"], "string");
        assert_eq!(json["indexed"], true);
        assert_eq!(json["required"], false);
        assert_eq!(json["default"], "member");
        assert_eq!(json["description"], "Assignment role");
        // target_type should be absent (skip_serializing_if = None)
        assert!(json.get("targetType").is_none());
    }

    #[test]
    fn test_edge_field_deserialization() {
        let json = json!({
            "name": "billing_date",
            "type": "date",
            "required": true,
            "indexed": true
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "billing_date");
        assert_eq!(field.field_type, "date");
        assert_eq!(field.required, Some(true));
        assert_eq!(field.indexed, Some(true));
        assert!(field.default.is_none());
        assert!(field.target_type.is_none());
        assert!(field.description.is_none());
    }

    #[test]
    fn test_edge_field_with_record_type() {
        let field = EdgeField {
            name: "approved_by".to_string(),
            field_type: "record".to_string(),
            core_values: None,
            indexed: Some(true),
            required: None,
            default: None,
            target_type: Some("person".to_string()),
            description: Some("Who approved this".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["type"], "record");
        assert_eq!(json["targetType"], "person");
    }

    #[test]
    fn test_edge_field_minimal() {
        // Test minimal edge field (only required fields)
        let json = json!({
            "name": "simple",
            "type": "string"
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "simple");
        assert_eq!(field.field_type, "string");
        assert!(field.indexed.is_none());
        assert!(field.required.is_none());
        assert!(field.default.is_none());
        assert!(field.core_values.is_none());
    }

    #[test]
    fn test_edge_field_enum_round_trip() {
        // The RBAC-style case: a closed role vocabulary declared on the edge.
        // Serializing must emit `coreValues` (camelCase) with value+label, and
        // deserializing the same JSON must reproduce the field exactly — this is
        // the property that fails if `core_values` is ever dropped at the
        // conversion boundary.
        let field = EdgeField {
            name: "role".to_string(),
            field_type: "enum".to_string(),
            core_values: Some(vec![
                EnumValue::new("owner".to_string(), "Owner".to_string()),
                EnumValue::new("editor".to_string(), "Editor".to_string()),
                EnumValue::new("viewer".to_string(), "Viewer".to_string()),
            ]),
            indexed: Some(true),
            required: Some(true),
            default: Some(json!("viewer")),
            target_type: None,
            description: Some("Access level on this membership".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["type"], "enum");
        // snake_case `core_values` must serialize to the camelCase wire key
        assert!(json.get("core_values").is_none());
        assert_eq!(json["coreValues"].as_array().unwrap().len(), 3);
        assert_eq!(json["coreValues"][0]["value"], "owner");
        assert_eq!(json["coreValues"][0]["label"], "Owner");
        assert_eq!(json["default"], "viewer");

        let round_tripped: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, field);
    }

    #[test]
    fn test_edge_field_omits_core_values_when_absent() {
        // A non-enum edge field must not gain an empty `coreValues` key.
        let field = EdgeField {
            name: "billing_date".to_string(),
            field_type: "date".to_string(),
            core_values: None,
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        };

        let json = serde_json::to_value(&field).unwrap();
        assert!(json.get("coreValues").is_none());
    }

    #[test]
    fn test_relationship_direction_serialization() {
        assert_eq!(
            serde_json::to_value(RelationshipDirection::Out).unwrap(),
            "out"
        );
        assert_eq!(
            serde_json::to_value(RelationshipDirection::In).unwrap(),
            "in"
        );
    }

    #[test]
    fn test_relationship_direction_deserialization() {
        let out: RelationshipDirection = serde_json::from_value(json!("out")).unwrap();
        assert_eq!(out, RelationshipDirection::Out);

        let r#in: RelationshipDirection = serde_json::from_value(json!("in")).unwrap();
        assert_eq!(r#in, RelationshipDirection::In);
    }

    #[test]
    fn test_relationship_cardinality_serialization() {
        assert_eq!(
            serde_json::to_value(RelationshipCardinality::One).unwrap(),
            "one"
        );
        assert_eq!(
            serde_json::to_value(RelationshipCardinality::Many).unwrap(),
            "many"
        );
    }

    #[test]
    fn test_relationship_cardinality_deserialization() {
        let one: RelationshipCardinality = serde_json::from_value(json!("one")).unwrap();
        assert_eq!(one, RelationshipCardinality::One);

        let many: RelationshipCardinality = serde_json::from_value(json!("many")).unwrap();
        assert_eq!(many, RelationshipCardinality::Many);
    }

    #[test]
    fn test_schema_relationship_serialization() {
        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: Some(true),
            reverse_name: "invoices".to_string(),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: Some(vec![
                EdgeField {
                    name: "billing_date".to_string(),
                    field_type: "date".to_string(),
                    core_values: None,
                    indexed: Some(true),
                    required: Some(true),
                    default: None,
                    target_type: None,
                    description: None,
                },
                EdgeField {
                    name: "payment_terms".to_string(),
                    field_type: "string".to_string(),
                    core_values: None,
                    indexed: None,
                    required: None,
                    default: Some(json!("net-30")),
                    target_type: None,
                    description: None,
                },
            ]),
            description: Some("Customer this invoice is billed to".to_string()),
        };

        let json = serde_json::to_value(&relationship).unwrap();

        assert_eq!(json["name"], "billed_to");
        assert_eq!(json["targetType"], "customer");
        assert_eq!(json["direction"], "out");
        assert_eq!(json["cardinality"], "one");
        assert_eq!(json["required"], true);
        assert_eq!(json["reverseName"], "invoices");
        assert_eq!(json["reverseCardinality"], "many");
        assert_eq!(json["edgeFields"].as_array().unwrap().len(), 2);
        assert_eq!(json["edgeFields"][0]["name"], "billing_date");
        assert_eq!(json["edgeFields"][1]["default"], "net-30");
    }

    #[test]
    fn test_schema_relationship_deserialization() {
        let json = json!({
            "name": "assigned_to",
            "targetType": "person",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "tasks",
            "reverseCardinality": "many",
            "edgeFields": [
                {
                    "name": "role",
                    "type": "string",
                    "indexed": true
                },
                {
                    "name": "assigned_at",
                    "type": "date",
                    "required": true
                }
            ]
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "assigned_to");
        assert_eq!(relationship.target_type, Some("person".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert_eq!(relationship.reverse_name, "tasks");
        assert_eq!(
            relationship.reverse_cardinality,
            RelationshipCardinality::Many
        );
        assert!(relationship.required.is_none());

        let edge_fields = relationship.edge_fields.unwrap();
        assert_eq!(edge_fields.len(), 2);
        assert_eq!(edge_fields[0].name, "role");
        assert_eq!(edge_fields[1].name, "assigned_at");
    }

    #[test]
    fn test_schema_relationship_minimal() {
        // The smallest legal declaration: the reverse half is part of it, so a
        // "minimal" relationship still names the edge from both ends.
        let json = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "children_of",
            "reverseCardinality": "one"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "parent_of");
        assert_eq!(relationship.target_type, Some("document".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert_eq!(relationship.reverse_name, "children_of");
        assert_eq!(
            relationship.reverse_cardinality,
            RelationshipCardinality::One
        );
        assert!(relationship.required.is_none());
        assert!(relationship.edge_fields.is_none());
        assert!(relationship.description.is_none());
    }

    /// The reverse half is carried by the type, not merely by a validator.
    ///
    /// The schema-op validators produce the actionable, example-bearing error a
    /// caller acts on; this asserts the floor beneath them — a payload missing
    /// either field cannot become a `SchemaRelationship` at all, so no other
    /// deserialization path can smuggle in a half-named edge.
    #[test]
    fn test_schema_relationship_requires_both_reverse_fields() {
        let missing_name = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseCardinality": "one"
        });
        let err = serde_json::from_value::<SchemaRelationship>(missing_name)
            .expect_err("reverseName must be required");
        assert!(
            err.to_string().contains("reverseName"),
            "error should name the missing field, got: {err}"
        );

        let missing_cardinality = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "children_of"
        });
        let err = serde_json::from_value::<SchemaRelationship>(missing_cardinality)
            .expect_err("reverseCardinality must be required");
        assert!(
            err.to_string().contains("reverseCardinality"),
            "error should name the missing field, got: {err}"
        );
    }

    #[test]
    fn test_schema_relationship_incoming_direction() {
        // Test "in" direction (less common but valid)
        let json = json!({
            "name": "owned_by",
            "targetType": "organization",
            "direction": "in",
            "cardinality": "one",
            "reverseName": "owns",
            "reverseCardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.direction, RelationshipDirection::In);
    }

    #[test]
    fn test_schema_relationship_untyped_deserialization() {
        // target_type absent → None (untyped/generic relationship)
        let json = json!({
            "name": "related",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "related_from",
            "reverseCardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.name, "related");
        assert!(relationship.target_type.is_none());
    }

    #[test]
    fn test_schema_relationship_untyped_serialization() {
        let relationship = SchemaRelationship {
            name: "related".to_string(),
            target_type: None,
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: "related_from".to_string(),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: None,
            description: None,
        };

        let json = serde_json::to_value(&relationship).unwrap();
        assert_eq!(json["name"], "related");
        // targetType absent when None
        assert!(json.get("targetType").is_none());
    }

    // Regression coverage for the silent-swallow bug: `parse_fields` (the
    // helper `SchemaNode::from_node` delegates to for its `fields` array) must
    // surface a diagnostic on a genuine parse failure instead of defaulting to
    // an empty `Vec` with zero signal. `nodespace-types` has no logging
    // dependency, so the diagnostic is a plain `String` message the caller
    // (`from_node`) prints via `eprintln!` — asserting on the message content
    // here is the stable, testable half of that; `eprintln!`'s actual stderr
    // write is exercised (not asserted) by
    // `test_from_node_malformed_fields_still_succeeds_with_empty_fields`
    // below and is visible under `cargo test -- --nocapture`.

    #[test]
    fn test_parse_fields_malformed_json_surfaces_diagnostic() {
        // `type` must be a string (`field_type`); a number is a genuine parse
        // failure, not merely an absent-and-defaulted key.
        let malformed = json!({ "fields": [{ "name": "status", "type": 42 }] });
        let (fields, diagnostic) = parse_fields(&malformed, "test-schema-id");

        assert!(
            fields.is_empty(),
            "malformed fields still default to empty — behavior is unchanged"
        );
        let msg = diagnostic.expect(
            "a fields parse failure must surface a diagnostic, not silently default to empty",
        );
        assert!(
            msg.contains("test-schema-id"),
            "diagnostic must name the affected schema node: {msg}"
        );
        assert!(
            msg.contains("fields"),
            "diagnostic must name the affected property: {msg}"
        );
    }

    #[test]
    fn test_parse_fields_valid_json_no_diagnostic() {
        let json = json!({ "fields": [{ "name": "status", "type": "enum" }] });
        let (fields, diagnostic) = parse_fields(&json, "test-schema-id");

        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "status");
        assert!(
            diagnostic.is_none(),
            "a successful parse must not produce a diagnostic"
        );
    }

    #[test]
    fn test_parse_fields_absent_key_no_diagnostic() {
        // A schema node with no `fields` key at all (e.g. a freshly created
        // schema) is not a parse failure — must stay silent, same as before.
        let json = json!({});
        let (fields, diagnostic) = parse_fields(&json, "test-schema-id");

        assert!(fields.is_empty());
        assert!(diagnostic.is_none());
    }

    #[test]
    fn test_from_node_malformed_fields_still_succeeds_with_empty_fields() {
        // End-to-end through `from_node`: a malformed `fields` value must not
        // fail the whole node conversion (that would propagate up through
        // `node_to_typed_value` into `nodes_to_typed_values`'s batch
        // `.collect()` and fail an entire unrelated batch read) — it must
        // still resolve to `Ok` with an empty `fields` Vec, now with a
        // diagnostic printed to stderr along the way (see
        // `test_parse_fields_malformed_json_surfaces_diagnostic` for the
        // assertable half of that diagnostic).
        let node = Node::new(
            "schema".to_string(),
            "Malformed schema".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "fields": [{ "name": "status", "type": 42 }],
            }),
        );

        let schema = SchemaNode::from_node(node).expect("must not fail the whole conversion");
        assert!(schema.fields.is_empty());
    }

    // Regression coverage for the silent-swallow bug: `parse_relationships`
    // (the helper `SchemaNode::from_node` delegates to for its
    // `relationships` array) must surface a diagnostic on a genuine parse
    // failure instead of defaulting to an empty `Vec` with zero signal.
    // Mirrors the `parse_fields` coverage above — see that block's comment
    // for the full rationale.

    #[test]
    fn test_parse_relationships_malformed_json_surfaces_diagnostic() {
        // `direction` must be "out" or "in" (`RelationshipDirection`); a
        // number is a genuine parse failure, not merely an absent-and-defaulted
        // key.
        let malformed = json!({
            "relationships": [{ "name": "assigned_to", "direction": 42, "cardinality": "one" }]
        });
        let (relationships, diagnostic) = parse_relationships(&malformed, "test-schema-id");

        assert!(
            relationships.is_empty(),
            "malformed relationships still default to empty — behavior is unchanged"
        );
        let msg = diagnostic.expect(
            "a relationships parse failure must surface a diagnostic, not silently default to empty",
        );
        assert!(
            msg.contains("test-schema-id"),
            "diagnostic must name the affected schema node: {msg}"
        );
        assert!(
            msg.contains("relationships"),
            "diagnostic must name the affected property: {msg}"
        );
    }

    #[test]
    fn test_parse_relationships_valid_json_no_diagnostic() {
        let json = json!({
            "relationships": [{
                "name": "assigned_to",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "tasks",
                "reverseCardinality": "many"
            }]
        });
        let (relationships, diagnostic) = parse_relationships(&json, "test-schema-id");

        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships[0].name, "assigned_to");
        assert!(
            diagnostic.is_none(),
            "a successful parse must not produce a diagnostic"
        );
    }

    #[test]
    fn test_parse_relationships_absent_key_no_diagnostic() {
        // A schema node with no `relationships` key at all (e.g. a freshly
        // created schema) is not a parse failure — must stay silent, same as
        // before.
        let json = json!({});
        let (relationships, diagnostic) = parse_relationships(&json, "test-schema-id");

        assert!(relationships.is_empty());
        assert!(diagnostic.is_none());
    }

    #[test]
    fn test_from_node_malformed_relationships_still_succeeds_with_empty_relationships() {
        // End-to-end through `from_node`: a malformed `relationships` value
        // must not fail the whole node conversion (same batch-collect
        // reasoning as the `fields` case) — it must still resolve to `Ok`
        // with an empty `relationships` Vec, now with a diagnostic printed to
        // stderr along the way (see
        // `test_parse_relationships_malformed_json_surfaces_diagnostic` for
        // the assertable half of that diagnostic).
        let node = Node::new(
            "schema".to_string(),
            "Malformed schema".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "relationships": [{ "name": "assigned_to", "direction": 42, "cardinality": "one" }],
            }),
        );

        let schema = SchemaNode::from_node(node).expect("must not fail the whole conversion");
        assert!(schema.relationships.is_empty());
    }
}
