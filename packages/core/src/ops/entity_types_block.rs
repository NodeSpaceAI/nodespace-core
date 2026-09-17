//! The single renderer for the `EXISTING SCHEMAS` prompt block.
//!
//! The block is emitted into the agent's system prompt from two places — the
//! workspace-context path ([`super::context_ops`]) and the skill-routing path
//! (`nodespace-agent`'s `local_agent::routing`) — which historically held two
//! independent renderers reading two different sources. They drifted: the
//! node-creation guidance conditioned `properties` population on field
//! metadata that only one of them emitted, so the model saw an instruction
//! with no referent and silently dropped every user-supplied value.
//!
//! Both paths now convert into [`EntityTypeDescriptor`] and render through
//! [`render_entity_types`]. The descriptor is the choke point: a field added
//! to `SchemaField` reaches the prompt only by passing through it, so it
//! cannot land in one path and not the other.
//!
//! The routing path additionally needs the descriptor to survive a JSON
//! round-trip, because it arrives as the `schema_metadata` body of the
//! model-facing `search_skills` tool response rather than as a typed value.
//! [`EntityTypeDescriptor::to_json`] and [`EntityTypeDescriptor::from_json`]
//! own that encoding so it is one reversible mapping rather than a
//! hand-written projection on each side.

use crate::models::schema::{RelationshipCardinality, RelationshipDirection, SchemaRelationship};
use crate::models::SchemaNode;
use serde_json::{json, Value};

/// One entity type as the prompt block renders it.
///
/// Deliberately smaller than [`SchemaNode`]: it carries exactly what the block
/// shows, so "what the model is told about a type" is a single reviewable
/// struct rather than a property of whichever renderer ran.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityTypeDescriptor {
    /// The type id the model must pass as `node_type`.
    pub type_id: String,
    /// Human-readable name. Rendered only when it adds information beyond the
    /// id — the routing path frequently has nothing else.
    pub name: Option<String>,
    pub fields: Vec<EntityFieldDescriptor>,
    /// This type's declared relationships to other types, carried so
    /// `schema_metadata` can tell the model which relationship to use (e.g.
    /// `epic.stories` vs. the structural `has_child`) and why, via each
    /// relationship's own `description`. Not rendered by [`Self::render_line`]
    /// — see [`EntityRelationshipDescriptor`]'s doc comment for why this stays
    /// out of the compact `EXISTING SCHEMAS` line.
    pub relationships: Vec<EntityRelationshipDescriptor>,
    /// Rendered because the `create_node` tool description promises the
    /// template is "shown in EXISTING SCHEMAS"; that promise needs a referent.
    pub title_template: Option<String>,
}

/// One field of an entity type as the prompt block renders it.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityFieldDescriptor {
    pub name: String,
    pub field_type: String,
    /// Legal values for an enum field. A bare `status: enum` tells the model a
    /// value is wanted but not which ones the schema accepts, so it invents one
    /// and the write is rejected.
    pub enum_values: Vec<String>,
    /// The node-creation guidance treats a required field as mandatory in the
    /// `properties` map, so the flag must reach the prompt to be actionable.
    pub required: bool,
    /// What the field is for — meaning, purpose, usage, an example where
    /// helpful. Mirrors [`crate::models::schema::SchemaField::description`],
    /// whose own doc comment already promises it is "consumed by the model
    /// for schema comprehension" — a promise [`Self::from_schema_field`]
    /// previously did not keep, since this struct dropped the value before it
    /// ever reached [`EntityTypeDescriptor::to_json`].
    ///
    /// Deliberately NOT rendered by [`Self::render`]/[`Self::render_shape`]:
    /// those notations feed the compact `EXISTING SCHEMAS` prompt line (both
    /// the workspace-context and skill-routing paths), which stays exactly as
    /// it read before this field existed. Prose this long belongs in
    /// `schema_metadata`'s JSON (via `to_json`/`from_json`), delivered
    /// alongside a matched skill result — not inlined into a block whose
    /// whole design goal is staying short enough to always be in context.
    pub description: Option<String>,
}

impl EntityFieldDescriptor {
    /// Render as `name: type`, with enum values and required-ness appended.
    ///
    /// This is the notation both call sites emit under a shared heading, so
    /// two spellings of one concept would read to the model as two different
    /// things.
    ///
    /// Only for prompts about *writing* a node. `required` is defined to the
    /// model as "MUST be included in the properties map" (the node-creation
    /// guidance), which is an obligation on a write and meaningless anywhere
    /// else — see [`Self::render_shape`] for read/filter contexts.
    pub fn render(&self) -> String {
        let mut descriptor = self.render_shape();
        if self.required {
            descriptor.push_str(", required");
        }
        descriptor
    }

    /// Render the field's shape alone — `name: type`, plus enum values — with
    /// no write-time obligations attached.
    ///
    /// For prompts that ask the model to *read* or filter rather than create.
    /// Carrying ", required" into such a prompt is not merely noise: its only
    /// definition tells the model the field must appear, so a model acting on
    /// it emits a filter for a field the request never mentioned, which matches
    /// nothing — indistinguishable from a genuinely empty result, which is the
    /// failure this whole area keeps reproducing.
    ///
    /// Enum values are wrapped in `{}`, not `()` — `render_line` wraps a
    /// type's whole field list in `()`, and a `()`-wrapped enum inside that
    /// list would nest identical delimiters with no visual way to tell which
    /// `)` closes which `(`.
    pub fn render_shape(&self) -> String {
        if self.enum_values.is_empty() {
            format!("{}: {}", self.name, self.field_type)
        } else {
            format!(
                "{}: {} {{{}}}",
                self.name,
                self.field_type,
                self.enum_values.join(", ")
            )
        }
    }

    /// Build from a schema field. Public so callers that render a field list
    /// into some *other* prompt — `resolve_query`'s sub-prompt, for one — can
    /// reuse this notation rather than hand-rolling a fourth renderer that
    /// drops whatever it forgets to spell out.
    pub fn from_schema_field(f: &crate::models::schema::SchemaField) -> Self {
        // Core and user values both carry legal enum members; the model needs
        // the union, since either kind is accepted on a write.
        let enum_values: Vec<String> = f
            .core_values
            .iter()
            .chain(f.user_values.iter())
            .flatten()
            .map(|v| v.value.clone())
            .collect();
        Self {
            name: f.name.clone(),
            field_type: f.field_type.clone(),
            enum_values,
            required: f.required.unwrap_or(false),
            description: f.description.clone(),
        }
    }
}

/// One relationship of an entity type as `schema_metadata` renders it.
///
/// `EntityTypeDescriptor` previously carried nothing about a type's
/// relationships at all — a caller reading `schema_metadata` could see a
/// type's fields but had no way to learn it also declares, say, `stories`
/// (out, many, to `story`) versus the structural `has_child`. This struct is
/// the relationship counterpart to [`EntityFieldDescriptor`]: deliberately
/// smaller than [`SchemaRelationship`] (no `edge_fields` — out of this
/// struct's scope; add it if a real caller needs it), carrying exactly enough
/// to name the relationship, tell one direction/cardinality from the other,
/// and — the reason this struct exists — surface its `description`.
///
/// Not rendered by [`EntityTypeDescriptor::render_line`]: relationships were
/// never part of the compact `EXISTING SCHEMAS` block before this struct
/// existed, and this issue's scope is getting `description` prose into
/// `schema_metadata` (the `find_skills` sidecar), not changing what the
/// deterministic prompt-injection block shows.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRelationshipDescriptor {
    /// The name this edge is declared under, from the declaring type's side.
    pub name: String,
    /// The related type, when the relationship is typed. `None` for an
    /// untyped relationship that accepts any target.
    pub target_type: Option<String>,
    pub direction: RelationshipDirection,
    pub cardinality: RelationshipCardinality,
    /// The name this edge reads by from the target's end (e.g. `epic.stories`
    /// read from `story`'s side might be `epic`).
    pub reverse_name: String,
    pub reverse_cardinality: RelationshipCardinality,
    /// Why this relationship exists / when to use it over another one.
    /// Mirrors [`SchemaRelationship::description`] — see
    /// [`EntityFieldDescriptor::description`]'s doc comment for why this is
    /// carried here rather than rendered into the compact prompt line.
    pub description: Option<String>,
}

impl EntityRelationshipDescriptor {
    /// Build from a schema relationship declaration.
    pub fn from_schema_relationship(r: &SchemaRelationship) -> Self {
        Self {
            name: r.name.clone(),
            target_type: r.target_type.clone(),
            direction: r.direction.clone(),
            cardinality: r.cardinality.clone(),
            reverse_name: r.reverse_name.clone(),
            reverse_cardinality: r.reverse_cardinality.clone(),
            description: r.description.clone(),
        }
    }

    /// Encode for the `schema_metadata` wire form. Snake_case keys, matching
    /// [`EntityTypeDescriptor::to_json`]'s convention for this response (it is
    /// not the camelCase wire contract the rest of the app uses).
    pub fn to_json(&self) -> Value {
        let mut rel = json!({
            "name": self.name,
            "direction": self.direction,
            "cardinality": self.cardinality,
            "reverse_name": self.reverse_name,
            "reverse_cardinality": self.reverse_cardinality,
        });
        if let Some(target_type) = &self.target_type {
            rel["target_type"] = json!(target_type);
        }
        if let Some(description) = &self.description {
            rel["description"] = json!(description);
        }
        rel
    }

    /// Decode one entry of a `schema_metadata` relationship listing.
    ///
    /// Returns `None` when `name`, `direction`, `cardinality`, `reverse_name`,
    /// or `reverse_cardinality` is missing or fails to parse — mirroring
    /// [`SchemaRelationship`]'s own requirement that both halves of a
    /// relationship are always named (see that type's doc comments). A
    /// relationship this incomplete has nothing a caller could act on, so the
    /// entry is skipped rather than decoded with a fabricated direction or
    /// cardinality.
    pub fn from_json(entry: &Value) -> Option<Self> {
        let name = entry.get("name").and_then(|v| v.as_str())?.to_string();
        let direction: RelationshipDirection = entry
            .get("direction")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())?;
        let cardinality: RelationshipCardinality = entry
            .get("cardinality")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())?;
        let reverse_name = entry
            .get("reverse_name")
            .and_then(|v| v.as_str())?
            .to_string();
        let reverse_cardinality: RelationshipCardinality = entry
            .get("reverse_cardinality")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())?;

        Some(Self {
            name,
            target_type: entry
                .get("target_type")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            direction,
            cardinality,
            reverse_name,
            reverse_cardinality,
            description: entry
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }
}

impl EntityTypeDescriptor {
    /// Build from a [`SchemaNode`] — the lossless conversion, used wherever the
    /// typed value is in hand.
    pub fn from_schema(schema: &SchemaNode) -> Self {
        Self {
            type_id: schema.id.clone(),
            name: Some(schema.content.clone()),
            fields: schema
                .fields
                .iter()
                .map(EntityFieldDescriptor::from_schema_field)
                .collect(),
            relationships: schema
                .relationships
                .iter()
                .map(EntityRelationshipDescriptor::from_schema_relationship)
                .collect(),
            title_template: schema.title_template.clone(),
        }
    }

    /// Encode for the `search_skills` tool response.
    ///
    /// This shape is model-facing and pinned by the routing latency suites, so
    /// it is a contract rather than an internal detail. Keeping the encode and
    /// decode adjacent is what stops it from becoming a second renderer.
    ///
    /// One intentional difference from the projection this replaced: that code
    /// emitted `enum_values` only when `field_type == "enum"`, whereas this
    /// emits them whenever a field carries values. The two agree on every
    /// seeded core schema (none has a non-enum field with values), but they can
    /// differ at runtime: validation only enforces enum ⇒ has-values, never the
    /// converse, so a `create_schema` call may legitimately land `coreValues` on
    /// a `text` field. Emitting the values there is the better behaviour — the
    /// model can only use legal values it is shown — but it is a widening of
    /// this response, not a no-op.
    pub fn to_json(&self) -> Value {
        let fields: Vec<Value> = self
            .fields
            .iter()
            .map(|f| {
                let mut field = json!({
                    "name": f.name,
                    "type": f.field_type,
                });
                if f.required {
                    field["required"] = json!(true);
                }
                if !f.enum_values.is_empty() {
                    field["enum_values"] = json!(f.enum_values);
                }
                if let Some(description) = &f.description {
                    field["description"] = json!(description);
                }
                field
            })
            .collect();

        let mut entry = json!({
            "type_id": self.type_id,
            "fields": fields,
        });
        if let Some(name) = &self.name {
            entry["name"] = json!(name);
        }
        if !self.relationships.is_empty() {
            let relationships: Vec<Value> =
                self.relationships.iter().map(|r| r.to_json()).collect();
            entry["relationships"] = json!(relationships);
        }
        if let Some(tmpl) = &self.title_template {
            entry["title_template"] = json!(tmpl);
        }
        entry
    }

    /// Decode one entry of a `schema_metadata` array.
    ///
    /// Returns `None` for an entry with no `type_id`: without it the model has
    /// nothing to pass as `node_type`, so the line would describe a type it
    /// cannot name.
    pub fn from_json(entry: &Value) -> Option<Self> {
        let type_id = entry.get("type_id").and_then(|v| v.as_str())?;
        let fields = entry
            .get("fields")
            .and_then(|f| f.as_array())
            .map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        let name = f.get("name").and_then(|v| v.as_str())?;
                        // A field whose type was dropped in transit is still
                        // worth naming; `text` is the schema system's own
                        // default for an unspecified type.
                        let field_type = f
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("text")
                            .to_string();
                        let enum_values = f
                            .get("enum_values")
                            .and_then(|v| v.as_array())
                            .map(|vals| {
                                vals.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(EntityFieldDescriptor {
                            name: name.to_string(),
                            field_type,
                            enum_values,
                            required: f.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                            description: f
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Malformed entries (missing/unparseable direction or cardinality)
        // are skipped rather than failing the whole type, same reasoning as
        // the `fields` decode above.
        let relationships = entry
            .get("relationships")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(EntityRelationshipDescriptor::from_json)
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            type_id: type_id.to_string(),
            name: entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            fields,
            relationships,
            title_template: entry
                .get("title_template")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }

    /// Render this type as one line of the block.
    ///
    /// `:` carries exactly one meaning in this line — field name to field
    /// type, inside the field list. Nothing else in the line uses it, so a
    /// reader (model or human) never has to decide which relationship a given
    /// `:` expresses:
    ///
    /// - the display name is quoted, not colon-prefixed, so `- invoice
    ///   "Invoice"` cannot be misread as a field declaration `invoice:
    ///   Invoice`
    /// - the field list is introduced by `->`, a token used nowhere else in
    ///   the line
    /// - enum values are wrapped in `{}` ([`EntityFieldDescriptor::render_shape`]),
    ///   distinct from the `->`-introduced field list, so a line never nests
    ///   the same delimiter inside itself
    ///
    /// Both call sites — workspace-context (`name: Some`) and per-candidate
    /// routing (`name: None`) — share this one shape; only whether the quoted
    /// name segment appears differs, per the doc comment above.
    ///
    /// A type with no fields renders with no `->`: a trailing separator would
    /// read as a promise of a field list that never arrives.
    pub fn render_line(&self) -> String {
        let mut line = format!("- {}", self.type_id);
        if let Some(name) = &self.name {
            // Escaped: an unescaped `"` in a user-authored display name would
            // close the quote early and reintroduce the ambiguity this format
            // exists to remove.
            line.push_str(&format!(" \"{}\"", name.replace('"', "\\\"")));
        }

        if !self.fields.is_empty() {
            let rendered: Vec<String> = self.fields.iter().map(|f| f.render()).collect();
            line.push_str(&format!(" -> {}", rendered.join("; ")));
        }

        if let Some(tmpl) = &self.title_template {
            line.push_str(&format!(" [title_template: {tmpl}]"));
        }
        line
    }
}

/// Render a set of entity types as the body of the block, one line per type.
///
/// Returns `None` when there is nothing to show, so callers can omit the
/// heading rather than emit one over an empty list.
pub fn render_entity_types(types: &[EntityTypeDescriptor]) -> Option<String> {
    if types.is_empty() {
        return None;
    }
    let lines: Vec<String> = types.iter().map(|t| t.render_line()).collect();
    Some(lines.join("\n"))
}

/// Decode a `schema_metadata` array into descriptors, skipping malformed
/// entries.
///
/// Skipping rather than failing is deliberate: propagating out of the loop
/// would discard every remaining type because one lacked an id.
pub fn descriptors_from_json(meta: &Value) -> Vec<EntityTypeDescriptor> {
    meta.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(EntityTypeDescriptor::from_json)
                .collect()
        })
        .unwrap_or_default()
}

/// Build the `available_properties` list for a node of a given type: every
/// field its schema defines, each marked with whether the node currently has a
/// value for it.
///
/// This exists because a node's `properties` map carries only *populated*
/// fields, so a defined-but-unset field (`due_date` on a fresh task) is, from
/// the model's side, indistinguishable from a field that does not exist. The
/// model then either invents a key or — correctly, under the "use the node's
/// own existing property keys" rule — declines and asks the user to name a
/// field the system already knows.
///
/// Delivered on a *tool result* rather than the routing prompt, per ADR-064
/// rule 4 (tool results own facts). That choice is what keeps this independent
/// of the `is_core` exclusion in [`super::context_ops`] and the routing block:
/// `EXISTING SCHEMAS` answers "which user-defined types are relevant to
/// this message", a retrieval question over an unbounded set. Core types are
/// the opposite — a small fixed set that needs to be *always known* when the
/// model is looking at a node of that type, not *found*. Routing that through
/// a relevance mechanism would make every core type a retrieval candidate on
/// every turn, which is what the `is_core` filter exists to prevent.
///
/// ADR-063 is unaffected by this channel: the list names only field names the
/// schema *already defines*, so it opens no path to writing a new bare key
/// onto a core type. The prefix rule lives in `create_node`'s guidance and
/// keys off `EXISTING SCHEMAS`, which this deliberately does not touch.
///
/// `set` is the load-bearing flag — without it this would merely be a longer
/// list of names, and "defined but unset" would still be unreadable.
///
/// Field names are rendered *flat* (`due_date`, not `task.due_date`). That is
/// the shape on both sides: `node_to_typed_value` flattens the stored
/// `{"task":{...}}` namespace for reads, and `NodeService` re-namespaces flat
/// keys on writes, so a name taken from this list can be passed straight back
/// to `update_node`.
pub fn build_available_properties(schema: &SchemaNode, properties: &Value) -> Vec<Value> {
    schema
        .fields
        .iter()
        .map(|f| {
            let descriptor = EntityFieldDescriptor::from_schema_field(f);
            // A JSON null is "no value", the same as an absent key — otherwise
            // a cleared field would report itself as set and the model would
            // have no reason to write to it.
            let is_set = properties
                .get(&descriptor.name)
                .map(|v| !v.is_null())
                .unwrap_or(false);
            let mut entry = json!({
                "name": descriptor.name,
                "type": descriptor.field_type,
                "set": is_set,
            });
            if !descriptor.enum_values.is_empty() {
                entry["allowed_values"] = json!(descriptor.enum_values);
            }
            entry
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel};

    fn field(name: &str, ty: &str) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            friendly_name: name.to_string(),
            field_type: ty.to_string(),
            protection: SchemaProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
            local_only: false,
        }
    }

    fn enum_value(value: &str) -> EnumValue {
        EnumValue::new(value.to_string(), value.to_string())
    }

    fn sample_schema() -> SchemaNode {
        let mut amount = field("amount", "number");
        amount.required = Some(true);
        amount.description =
            Some("Total invoiced amount, in the customer's billing currency".to_string());

        let mut status = field("status", "enum");
        status.core_values = Some(vec![enum_value("draft")]);
        status.user_values = Some(vec![enum_value("sent")]);

        SchemaNode {
            id: "invoice".to_string(),
            content: "Invoice".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            is_core: false,
            schema_version: 1,
            fields: vec![field("reference", "string"), amount, status],
            relationships: vec![SchemaRelationship {
                name: "billed_to".to_string(),
                target_type: Some("customer".to_string()),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::One,
                required: None,
                reverse_name: "invoices".to_string(),
                reverse_cardinality: RelationshipCardinality::Many,
                edge_fields: None,
                description: Some("The customer this invoice is billed to".to_string()),
            }],
            title_template: Some("{reference}".to_string()),
            properties_header_summary_template: None,
        }
    }

    /// Golden test: the exact rendered line for a type with a display name,
    /// an enum field (core and user values unioned), a required field, and a
    /// `title_template`. Pins the grammar this module exists to keep
    /// unambiguous — `:` appears only inside the `->`-introduced field list,
    /// the display name is quoted rather than colon-prefixed, and enum values
    /// use `{}` rather than the field list's own delimiter.
    #[test]
    fn renders_type_field_enum_required_and_template() {
        let d = EntityTypeDescriptor::from_schema(&sample_schema());

        assert_eq!(
            d.render_line(),
            "- invoice \"Invoice\" -> reference: string; amount: number, required; \
             status: enum {draft, sent} [title_template: {reference}]"
        );
    }

    /// The property this whole module exists to enforce: the typed path and the
    /// JSON path must describe a schema identically. A field added to
    /// `SchemaField` that reaches only one of them fails here.
    #[test]
    fn json_round_trip_preserves_every_rendered_field() {
        let typed = EntityTypeDescriptor::from_schema(&sample_schema());
        let decoded = EntityTypeDescriptor::from_json(&typed.to_json())
            .expect("descriptor with a type_id must decode");

        assert_eq!(typed, decoded, "JSON round-trip must not lose information");
        assert_eq!(typed.render_line(), decoded.render_line());
    }

    #[test]
    fn fieldless_type_renders_without_a_dangling_separator() {
        let d = EntityTypeDescriptor {
            type_id: "venue".to_string(),
            name: None,
            fields: vec![],
            relationships: vec![],
            title_template: None,
        };
        assert_eq!(d.render_line(), "- venue");
    }

    /// A display name containing `"` must not close the quote early — that
    /// would let the rest of the name spill out unquoted and reintroduce the
    /// ambiguity this format exists to remove.
    #[test]
    fn a_quote_in_the_display_name_is_escaped_not_left_to_close_early() {
        let d = EntityTypeDescriptor {
            type_id: "invoice".to_string(),
            name: Some(r#"The "Big" Invoice"#.to_string()),
            fields: vec![],
            relationships: vec![],
            title_template: None,
        };
        assert_eq!(d.render_line(), r#"- invoice "The \"Big\" Invoice""#);
    }

    #[test]
    fn malformed_entry_is_skipped_without_dropping_the_rest() {
        let meta = json!([
            { "fields": [] },
            { "type_id": "venue", "fields": [{ "name": "capacity", "type": "number" }] },
        ]);
        let descriptors = descriptors_from_json(&meta);

        assert_eq!(descriptors.len(), 1, "the entry lacking type_id is skipped");
        assert_eq!(descriptors[0].type_id, "venue");
    }

    #[test]
    fn empty_type_list_renders_nothing() {
        assert!(render_entity_types(&[]).is_none());
    }

    /// The block that reaches the model must be the same whether it came from
    /// the workspace-context path (typed `SchemaNode`) or the skill-routing
    /// path (`schema_metadata` JSON). Both are concatenated into one system
    /// prompt under one heading; describing a type two ways there is what
    /// caused the model to follow guidance with no referent.
    ///
    /// The JSON side is written out by hand rather than produced by
    /// [`EntityTypeDescriptor::to_json`]. Deriving it from `to_json` would make
    /// both sides originate from the same `from_schema` call, which asserts
    /// round-trip fidelity (already covered by
    /// `json_round_trip_preserves_every_rendered_field`) rather than agreement
    /// between the paths. Spelling the wire form out independently is what lets
    /// this fail when an encoder change stops matching what the decoder reads.
    ///
    /// Note the structural guarantee is the stronger one and does not depend on
    /// this test: a single `render_line` means a new `SchemaField` reaches both
    /// paths or neither. This pins the conversion seams on either side of it.
    #[test]
    fn both_prompt_paths_render_a_schema_identically() {
        let via_schema = EntityTypeDescriptor::from_schema(&sample_schema());

        // The `schema_metadata` wire form for the same schema, spelled out as
        // `skill_ops` emits it.
        let via_json = descriptors_from_json(&json!([{
            "type_id": "invoice",
            "name": "Invoice",
            "fields": [
                {"name": "reference", "type": "string"},
                {"name": "amount", "type": "number", "required": true},
                {"name": "status", "type": "enum", "enum_values": ["draft", "sent"]}
            ],
            "title_template": "{reference}"
        }]));

        assert_eq!(
            render_entity_types(std::slice::from_ref(&via_schema)),
            render_entity_types(&via_json),
            "the two prompt paths must describe a schema identically"
        );
    }

    /// Guards the specific regression that motivated the shared renderer:
    /// required-ness and enum values must reach the prompt on *both* paths,
    /// since the node-creation guidance keys `properties` population on them.
    #[test]
    fn required_and_enum_values_survive_the_routing_path() {
        let via_json =
            descriptors_from_json(&json!([
                EntityTypeDescriptor::from_schema(&sample_schema()).to_json()
            ]));
        let rendered = render_entity_types(&via_json).expect("one type renders");

        assert!(rendered.contains("amount: number, required"));
        assert!(rendered.contains("status: enum {draft, sent}"));
        assert!(rendered.contains("[title_template: {reference}]"));
    }

    /// This issue's core fix: a field's `description` and a relationship's
    /// `description` (previously dropped entirely — `SchemaRelationship`
    /// wasn't represented in `EntityTypeDescriptor` at all) must survive
    /// `from_schema` -> `to_json` -> `from_json`, so `schema_metadata`
    /// carries them to the model instead of silently discarding them.
    #[test]
    fn field_and_relationship_descriptions_survive_the_json_round_trip() {
        let typed = EntityTypeDescriptor::from_schema(&sample_schema());

        let amount = typed
            .fields
            .iter()
            .find(|f| f.name == "amount")
            .expect("amount field present");
        assert_eq!(
            amount.description.as_deref(),
            Some("Total invoiced amount, in the customer's billing currency")
        );

        assert_eq!(typed.relationships.len(), 1);
        let billed_to = &typed.relationships[0];
        assert_eq!(billed_to.name, "billed_to");
        assert_eq!(billed_to.target_type.as_deref(), Some("customer"));
        assert_eq!(billed_to.direction, RelationshipDirection::Out);
        assert_eq!(billed_to.cardinality, RelationshipCardinality::One);
        assert_eq!(billed_to.reverse_name, "invoices");
        assert_eq!(billed_to.reverse_cardinality, RelationshipCardinality::Many);
        assert_eq!(
            billed_to.description.as_deref(),
            Some("The customer this invoice is billed to")
        );

        // Round trip through the wire encoding find_skills actually emits.
        let json = typed.to_json();
        assert_eq!(
            json["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == "amount")
                .unwrap()["description"],
            json!("Total invoiced amount, in the customer's billing currency")
        );
        assert_eq!(
            json["relationships"][0]["description"],
            json!("The customer this invoice is billed to")
        );

        let decoded = EntityTypeDescriptor::from_json(&json).expect("decodes");
        assert_eq!(typed, decoded, "JSON round-trip must not lose information");
    }

    /// The compact `EXISTING SCHEMAS` prompt line must not grow field or
    /// relationship prose — that block's whole design goal is staying short
    /// enough to always be in context. Description content reaches the model
    /// only via `schema_metadata`'s JSON (`to_json`), never via `render_line`.
    #[test]
    fn descriptions_do_not_leak_into_the_compact_render_line() {
        let d = EntityTypeDescriptor::from_schema(&sample_schema());
        let line = d.render_line();

        assert!(!line.contains("Total invoiced amount"));
        assert!(!line.contains("billed to"));
        // The compact line predates relationships entirely — still true.
        assert!(!line.contains("billed_to"));
    }

    /// The core defect: an unset field must still be listed, and must be
    /// distinguishable from a set one. Without `set`, "defined but unset" and
    /// "does not exist" read identically and the model declines the write.
    #[test]
    fn available_properties_lists_unset_fields_marked_not_set() {
        let schema = sample_schema();
        let props = json!({ "reference": "INV-1" });

        let available = build_available_properties(&schema, &props);
        let by_name = |n: &str| {
            available
                .iter()
                .find(|f| f["name"] == n)
                .unwrap_or_else(|| panic!("{n} missing from available_properties"))
                .clone()
        };

        // Every defined field appears, set or not.
        assert_eq!(available.len(), 3);
        assert_eq!(by_name("reference")["set"], json!(true));
        assert_eq!(by_name("amount")["set"], json!(false));
        assert_eq!(by_name("status")["set"], json!(false));
        assert_eq!(by_name("amount")["type"], json!("number"));
    }

    /// A field explicitly cleared to null has no value, so reporting it as set
    /// would leave the model with no reason to write to it.
    #[test]
    fn available_properties_treats_null_as_unset() {
        let available = build_available_properties(&sample_schema(), &json!({ "amount": null }));
        let amount = available.iter().find(|f| f["name"] == "amount").unwrap();

        assert_eq!(amount["set"], json!(false));
    }

    /// Enum values must ride along, or `open`/`draft` still has to be guessed —
    /// which is the second half of the reported failure (the model asking the
    /// user to confirm the enum value, not just the field name).
    #[test]
    fn available_properties_exposes_allowed_values() {
        let available = build_available_properties(&sample_schema(), &json!({}));
        let status = available.iter().find(|f| f["name"] == "status").unwrap();

        assert_eq!(status["allowed_values"], json!(["draft", "sent"]));
        // Non-enum fields carry no empty key that would read as "no legal value".
        let reference = available.iter().find(|f| f["name"] == "reference").unwrap();
        assert!(reference.get("allowed_values").is_none());
    }

    /// ADR-063 guard, asserted rather than assumed (the issue requires this
    /// re-verification for any new delivery path): this list can only ever name
    /// keys the schema already defines, so it adds no route by which a model
    /// writes a new bare property key onto a core type. The prefix rule keys off
    /// `EXISTING SCHEMAS`, which this channel does not touch.
    #[test]
    fn available_properties_names_only_schema_defined_fields() {
        let schema = sample_schema();
        // Properties carrying keys the schema does not define — including a
        // bare key of the kind ADR-063 prohibits on a core type.
        let props = json!({ "reference": "INV-1", "weight": "40kg", "custom:color": "red" });

        let available = build_available_properties(&schema, &props);
        let names: Vec<&str> = available
            .iter()
            .filter_map(|f| f["name"].as_str())
            .collect();

        assert_eq!(names, vec!["reference", "amount", "status"]);
        assert!(!names.contains(&"weight"));
        assert!(!names.contains(&"custom:color"));
    }

    /// A core schema is handled identically — the whole point of the fix is
    /// that core types are no longer the blind spot.
    #[test]
    fn available_properties_covers_core_schemas() {
        let mut schema = sample_schema();
        schema.id = "task".to_string();
        schema.is_core = true;
        let mut due = field("due_date", "date");
        due.required = Some(false);
        schema.fields = vec![due];

        let available = build_available_properties(&schema, &json!({ "status": "in_progress" }));

        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["name"], json!("due_date"));
        assert_eq!(available[0]["set"], json!(false));
    }
}
