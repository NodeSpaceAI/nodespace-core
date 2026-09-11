//! Schema creation and updates.
//!
//! Provides the `create_schema` tool for creating custom schemas with explicit
//! field and relationship definitions.

use crate::behaviors::SchemaNodeBehavior;
use crate::markdown::MarkdownError;
use crate::models::schema::SchemaField;
use crate::models::{Node, NodeUpdate, SchemaNode};
use crate::services::error::NodeServiceError;
use crate::services::{CreateNodeParams, NodeService};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Reserved core property names that conflict with system properties
const RESERVED_CORE_PROPERTIES: &[&str] = &[
    "id",
    "node_type",
    "content",
    "parent_id",
    "root_id",
    "created_at",
    "modified_at",
    "status",
    "priority",
    "due_date",
    "due",
];

/// Report malformed entries in a `create_schema`/`update_schema` `fields` array
/// with enough context for the caller to repair the exact element at fault.
///
/// Serde's own error for this payload names only the absent key — for a field
/// array it cannot say which element lacked it. That is actionable for a human
/// reading a stack trace and useless to an LLM constructing the call: with no
/// position, the model edits an element that was already correct and each retry
/// loses more information than the last.
///
/// Returns `Ok(())` when `fields` is absent or every entry has both `name` and
/// `type` — the payload then proceeds to normal deserialization, so this
/// function only ever converts an error into a better-located error.
fn describe_malformed_fields(params: &Value, key: &str) -> Result<(), MarkdownError> {
    let Some(fields) = params.get(key).and_then(Value::as_array) else {
        return Ok(());
    };

    // An entry that declares nothing at all is dropped rather than reported —
    // see `drop_empty_field_entries`, which runs after this check for exactly
    // that reason. Skipping it HERE rather than removing it first is what keeps
    // the reported positions honest: dropping first renumbers the array, so a
    // problem in the caller's `fields[1]` would come back named `fields[0]` and
    // send the caller to edit an entry it never got wrong.
    //
    // Unless nothing else declares anything either. An array of only-null
    // entries is a caller that never expressed a single field, and reporting
    // that is the whole point: dropping them all would leave `fields: []`,
    // which `handle_create_schema` reads as a deliberate fieldless schema and
    // creates one — a silent wrong success in place of a correctable error.
    let any_informative = fields.iter().any(|f| !is_informationless_field_entry(f));

    let mut problems: Vec<String> = Vec::new();
    for (idx, field) in fields.iter().enumerate() {
        if any_informative && is_informationless_field_entry(field) {
            continue;
        }
        let Some(obj) = field.as_object() else {
            problems.push(format!(
                "{key}[{idx}] is {}, not an object",
                json_type_name(field)
            ));
            continue;
        };
        // Identify the element by name where one exists — a name is far easier
        // for the caller to match against what it sent than a bare index.
        let label = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty())
            .map(|n| format!("{key}[{idx}] (\"{n}\")"))
            .unwrap_or_else(|| format!("{key}[{idx}]"));

        // Not `key` — that names the array being checked, and shadowing it here
        // with a per-entry property name gives one identifier two meanings in a
        // function whose whole job is naming things precisely.
        for required_key in ["name", "type"] {
            // `null` counts as missing, not as present-with-a-value. Serde's
            // own message for a null here is `invalid type: null, expected a
            // string`, which names neither the key nor the entry — the bare,
            // unlocatable error this function exists to replace.
            let missing = match obj.get(required_key) {
                None | Some(Value::Null) => true,
                Some(Value::String(s)) => s.trim().is_empty(),
                Some(_) => false,
            };
            if missing {
                problems.push(format!("{label} is missing \"{required_key}\""));
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }

    Err(MarkdownError::invalid_params(format!(
        // No "Invalid parameters:" prefix — Display for InvalidParams already
        // writes "invalid params: ", and the model reads the doubled preamble first.
        "{}. Every entry in \"{key}\" needs both \"name\" and \"type\", \
         e.g. {{\"name\":\"amount\",\"type\":\"number\"}}. Re-send the call with only the \
         listed entries corrected — leave every other field exactly as it was.",
        problems.join("; ")
    )))
}

/// Drop entries from a `fields`-shaped array whose every value is null.
///
/// An entry like `{"description":null,"name":null}` declares nothing: there is
/// no field name to store under and no type to store. Rejecting it is
/// technically correct and practically harmful — it fails a call whose other
/// entries were complete, and the caller's next attempt mutates the parts that
/// were already right.
///
/// Observed live on the locked model, on the retry that had just correctly
/// repaired a missing top-level `name`:
///
/// ```json
/// {"fields":[ …status…, …estimated_days…, {"description":null,"name":null}],
///  "name":"Feature Write-up"}
/// ```
///
/// Both real fields were well-formed and the schema name was right. The call
/// was rejected for the null entry, and the following attempt added a stray
/// `field_values` key, then the one after abandoned `create_schema` entirely
/// and called `create_node` against a type that had never been created. One
/// informationless entry cost the whole chain.
///
/// Deliberately narrow, and NOT a general "ignore bad fields" rule. An entry
/// with a real `name` but a missing `type` is a genuine mistake the caller must
/// see and fix — `describe_malformed_fields` still reports it, and still
/// reports an all-null entry's siblings. This drops only entries that carry no
/// signal at all, so nothing a caller expressed is silently discarded.
///
/// Runs AFTER `describe_malformed_fields`, never before. That check reports
/// problems by array position, and dropping first renumbers the array — a
/// problem the caller has at `fields[1]` would be reported as `fields[0]` and
/// send it to rewrite an entry that was already correct. `describe_malformed_fields`
/// therefore skips these entries itself, which leaves the positions intact and
/// makes it the one place the "declares nothing" rule is applied. It also means
/// an array of ONLY such entries has already been rejected by the time this
/// runs, so this can never empty a non-empty `fields` array.
fn drop_empty_field_entries(params: &mut Value, key: &str) {
    let Some(fields) = params.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    fields.retain(|entry| !is_informationless_field_entry(entry));
}

/// Whether a `fields`-shaped entry declares nothing at all: an object whose
/// every value is null, or one with no keys.
///
/// A non-object is NOT informationless — it is a caller mistake, and
/// `describe_malformed_fields` reports it with its position rather than
/// removing it silently.
fn is_informationless_field_entry(entry: &Value) -> bool {
    entry
        .as_object()
        .is_some_and(|obj| obj.values().all(Value::is_null))
}

/// Report a missing REQUIRED TOP-LEVEL key on `create_schema` with an
/// instruction the caller can act on, before serde reports it as a bare name.
///
/// Sibling of [`describe_malformed_fields`], which does the same job one level
/// down (a malformed entry *inside* the `fields` array). This covers the case
/// that array-level check cannot see: a payload whose `fields` are all
/// well-formed but which omits the schema's own `name`.
///
/// **Why this is needed rather than left to the grammar.** Tool-call arguments
/// are NOT constrained to the tool's JSON schema on this stack. llama.cpp emits
/// `tool-create-schema ::= ("create_schema") gemma4-dict`, where `gemma4-dict`
/// is any well-formed JSON object — so `required: ["name", "fields"]` is never
/// enforced during sampling. That is a documented upstream limitation, not a
/// local defect: a llama.cpp collaborator states "Gemma 4 only forces the
/// structure, not the arguments", because `json-schema-to-grammar.cpp` "only
/// produces rules for JSON and not Gemma's fc notation"
/// (ggml-org/llama.cpp discussion 21839). The PR that would add
/// schema-constrained decoding for this format has been open and unreviewed for
/// months, and its own reported blocker is enum constraints on required
/// properties — exactly the shape `create_schema` has. So the constraint has to
/// be applied here, at our boundary.
///
/// **Why this reports rather than repairs.** The four `repair_*` functions in
/// `agent_loop` fix malformations that are mechanical and unambiguous — a key
/// carrying its own quote marks means the same key without them. A missing
/// schema name is not that: the correct value would have to be inferred from
/// the user's prose, which is interpretation, and the repair doctrine
/// deliberately stops short of it. Naming the type wrongly is worse than asking
/// again, because the id derives from it and every later call must match.
///
/// Measured on the locked model: 17 of 17 failing `create_schema` calls in one
/// run began `{"fields":[…]}` with a correct, complete fields array and no
/// top-level `name`. Serde's own message for that is `missing field \`name\``,
/// which carries no repair instruction — and the model re-sent the identical
/// payload until the duplicate-call guard broke the loop.
fn describe_missing_top_level_keys(params: &Value) -> Result<(), MarkdownError> {
    // Only meaningful for an object payload; anything else fails later with a
    // clearer type error than this function could add.
    let Some(obj) = params.as_object() else {
        return Ok(());
    };

    // What is wrong with `name`, phrased to complete "\"name\" is required and
    // …". `None` is the measured case; the others reach the same bare serde
    // error this function exists to replace (`invalid type: null, expected a
    // string`), which names neither the key nor what to do about it. A model
    // that appends `"name": null` to a field entry — the shape
    // `drop_empty_field_entries` exists for — writes it at the top level too.
    let name_problem: Option<String> = match obj.get("name") {
        None => Some("was not sent".to_string()),
        Some(Value::Null) => Some("was sent as null".to_string()),
        Some(Value::String(s)) if s.trim().is_empty() => Some("was sent empty".to_string()),
        Some(Value::String(_)) => None,
        Some(other) => Some(format!("was sent as {}", json_type_name(other))),
    };
    let Some(name_problem) = name_problem else {
        return Ok(());
    };

    // Reflect back what the call DID carry, so the model can see its fields
    // survived and that only the one key needs adding. Without this the model
    // has no signal distinguishing "add a key" from "the whole call was wrong",
    // and it re-sends the payload rewritten rather than extended.
    let field_names: Vec<String> = obj
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|f| f.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let kept = if field_names.is_empty() {
        String::new()
    } else {
        format!(
            " Your \"fields\" are correct and must be re-sent unchanged ({}).",
            field_names.join(", ")
        )
    };

    Err(MarkdownError::invalid_params(format!(
        // No "Invalid parameters:" prefix — Display for InvalidParams already
        // writes "invalid params: ".
        "\"name\" is required and {name_problem}. It must be a non-empty string: the DISPLAY \
         NAME of the type being created, in the user's own words, singular — e.g. \
         {{\"name\": \"Ticket\", \"fields\": [...]}}. It is a top-level parameter, NOT an entry \
         in \"fields\".{kept} Re-send the same call with \"name\" set."
    )))
}

/// Whether a field name carries a `<namespace>:` prefix (`custom:capacity`).
///
/// Requires exactly one `:` with a non-empty segment on each side, so neither a
/// bare name (`capacity`) nor a malformed one (`:capacity`, `custom:`,
/// `a:b:c`) counts as prefixed. `validate_schema_field_name` rejects the
/// malformed forms outright, but this does not lean on that: a check that
/// silently depends on validation order in another module is one refactor away
/// from admitting the names it exists to exclude.
fn has_namespace_prefix(name: &str) -> bool {
    let mut parts = name.split(':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(namespace), Some(bare), None) if !namespace.is_empty() && !bare.is_empty()
    )
}

/// Warn on explicit field names that shadow a reserved core property.
///
/// A name colliding with a reserved core property is reported as a warning
/// rather than rejected, leaving the choice with the caller — this mirrors how
/// the description-inference route treated the same collision before it was
/// deleted (ADR-063). `fields` is otherwise stored verbatim.
fn warn_reserved_property_names(fields: Vec<SchemaField>) -> (Vec<SchemaField>, Vec<String>) {
    let mut warnings = Vec::new();
    for field in &fields {
        if RESERVED_CORE_PROPERTIES.contains(&field.name.as_str()) {
            warnings.push(format!(
                "Field name '{}' matches a reserved core property and may be shadowed by it. \
                 Consider a more specific name.",
                field.name
            ));
        }
    }
    (fields, warnings)
}

/// Fill in `friendlyName` for any field in `new_fields` where the caller
/// omitted it, then disambiguate a derived value that collides with a
/// friendly_name already in use by `existing` or by an earlier field in this
/// same batch.
///
/// `SchemaField::friendly_name` is non-optional in storage, but callers of
/// `create_schema`/`update_schema` (the agent included) are not required to
/// supply it — `#[serde(default)]` on the field accepts an absent value as an
/// empty string, and this is the one place that empty string gets replaced
/// with a real label, via [`derive_friendly_name`]. This is THE write
/// boundary: every field that passes through here on its way into storage
/// carries a populated `friendly_name`, so no reader anywhere else needs a
/// fallback or null-check.
///
/// Collision disambiguation exists because stripping a namespace prefix for
/// display (`derive_friendly_name`'s job) is a display-only operation, but it
/// is reachable *today*, not just in some future release: adding
/// `custom:status` next to an existing core `status` field derives "Status"
/// for both, since `custom:` and bare `status` are different storage keys
/// but the same stripped/humanized display text. Only a *derived* value is
/// ever adjusted — a friendly_name the caller explicitly supplied is never
/// rewritten out from under them, even if it collides; that ambiguity is
/// their call, not this function's to silently correct.
fn apply_friendly_name_defaults(existing: &[SchemaField], new_fields: &mut [SchemaField]) {
    let mut taken: std::collections::HashSet<String> =
        existing.iter().map(|f| f.friendly_name.clone()).collect();

    for field in new_fields.iter_mut() {
        let was_omitted = field.friendly_name.trim().is_empty();
        if was_omitted {
            field.friendly_name = crate::models::schema::derive_friendly_name(&field.name);
            if taken.contains(&field.friendly_name) {
                field.friendly_name = disambiguate_friendly_name(&field.friendly_name, &field.name);
            }
        }
        taken.insert(field.friendly_name.clone());
    }
}

/// Append a disambiguator to `label` so it no longer collides with another
/// field's display label, using `name`'s namespace prefix when it has one
/// (`"Status" -> "Status (custom)"`) or the full field name otherwise
/// (`"Employee name" -> "Employee name (employeeName)"`). `name` is unique
/// within a schema (a duplicate is rejected before this runs), so the result
/// is guaranteed unique too.
///
/// `pub(crate)` rather than private: `NodeService::update_schema_field_friendly_name`
/// (`services/node_service/schema.rs`) reuses this to apply the exact same
/// derive-and-disambiguate treatment `apply_friendly_name_defaults` gives an
/// omitted `friendly_name` on create/`add_fields`, so a blank value can never
/// reach storage through either path.
pub(crate) fn disambiguate_friendly_name(label: &str, name: &str) -> String {
    match name.split_once(':') {
        Some((prefix, _)) if !prefix.is_empty() => format!("{label} ({prefix})"),
        _ => format!("{label} ({name})"),
    }
}

/// Whether `schema_id` names a schema NodeSpace ships, rather than a
/// user-defined one. A missing schema is reported as non-core; the update path
/// below surfaces the not-found error with better context.
async fn schema_is_core(
    node_service: &Arc<NodeService>,
    schema_id: &str,
) -> Result<bool, MarkdownError> {
    let schema = node_service
        .get_schema_node(schema_id)
        .await
        .map_err(|e| MarkdownError::internal_error(format!("Failed to get schema: {}", e)))?;
    Ok(schema.is_some_and(|s| s.is_core))
}

/// Reject relationships whose `targetType` does not name an existing schema.
///
/// `TARGET_TYPE_MUST_EXIST` (`skill_rules.rs`) already tells the model this in
/// prose, but nothing previously enforced it: `handle_create_schema` and
/// `handle_update_schema` both persisted a relationship's `targetType`
/// verbatim with no existence check, so a model that ignored the prose rule
/// (or split one request into two schemas and referenced the second before it
/// existed) got a silent success and a dangling reference instead of an
/// actionable error. `targetType: None` is left unvalidated — omitting the
/// target entirely is the documented escape hatch for "the type doesn't exist
/// yet."
///
/// `pending_schema_id` names the schema the calling operation is creating,
/// which does not exist yet and so cannot be found by lookup. A relationship
/// targeting it is nonetheless always resolvable — the target is guaranteed to
/// exist the moment the call commits — so it is accepted rather than treated
/// as dangling. Self-reference is a routine modelling shape (`supersedes` on
/// an ADR, `blocks` on a task, `parent` on a category), and rejecting it was a
/// false positive, not a safety property. Every other target keeps the lookup:
/// those genuinely can dangle, which is the case this guard was written for.
/// `handle_update_schema` passes `None` — the schema it edits already exists,
/// so a self-reference there resolves through the ordinary lookup.
async fn validate_relationship_targets_exist(
    node_service: &Arc<NodeService>,
    relationships: &[crate::models::schema::SchemaRelationship],
    pending_schema_id: Option<&str>,
) -> Result<(), MarkdownError> {
    for rel in relationships {
        let Some(target_type) = rel.target_type.as_deref() else {
            continue;
        };
        if pending_schema_id == Some(target_type) {
            continue;
        }
        let exists = node_service
            .get_schema_node(target_type)
            .await
            .map_err(|e| MarkdownError::internal_error(format!("Failed to get schema: {}", e)))?
            .is_some();
        if !exists {
            return Err(MarkdownError::invalid_params(format!(
                "Relationship '{}' targets '{}', which is not an existing schema. \
                 targetType must name a schema that already exists — omit the relationship \
                 entirely if the target type doesn't exist yet, rather than inventing one. \
                 (A relationship pointing back at the schema being created is allowed: \
                 give its schema ID, the snake_case form of the name.)",
                rel.name, target_type
            )));
        }
    }
    Ok(())
}

/// Reject relationship declarations named after a built-in structural
/// relationship (`has_child`, `mentions`, `member_of`, `has_role`) — in either
/// direction.
///
/// **Forward `name`.** Declarations and the built-in primitives share the one
/// `relationship` table's `relationship_type` column, so a name collision would
/// make every type-keyed relationship query ambiguous — a correctness hazard,
/// not just a display glitch. Checked here so the error surfaces before any
/// write, and re-checked in `NodeService::set_schema_relationships` (the write
/// path) via the same shared predicate.
///
/// **`reverse_name`.** A different failure, so worth stating separately: a
/// reverse name is never written to `relationship_type` — it is a resolution
/// alias, matched by [`resolve_relationship_name`] to reach the inbound side of
/// an edge stored under the forward name. It therefore cannot make stored data
/// ambiguous. What it can do is nothing at all: that resolver short-circuits on
/// `BUILTIN_RELATIONSHIP_NAMES` before it ever consults a declaration, so
/// `reverseName: "has_child"` is unreachable — the built-in always wins, and
/// the reverse spelling the author chose silently resolves to something else.
/// Rejecting it keeps a declaration from being accepted as inert.
///
/// Both halves are checked because a relationship must name its edge from both
/// ends, so both names land in a namespace a caller can traverse by.
fn reject_reserved_relationship_names(
    relationships: &[crate::models::schema::SchemaRelationship],
) -> Result<(), MarkdownError> {
    for rel in relationships {
        for (which, name) in [("name", &rel.name), ("reverseName", &rel.reverse_name)] {
            if crate::models::schema::is_builtin_relationship(name) {
                return Err(MarkdownError::invalid_params(format!(
                    "Relationship {} '{}' is reserved for a built-in structural relationship \
                     ({}). Choose a different name.",
                    which,
                    name,
                    crate::models::schema::BUILTIN_RELATIONSHIP_NAMES.join(", ")
                )));
            }
        }
    }
    Ok(())
}

/// Validate the `edgeFields` declared on each relationship.
///
/// Mirrors the node-side `validate_schema_field` enum rule (an enum must
/// declare its values) and adds the two guards an edge enum needs to be
/// trustworthy as a closed vocabulary:
///
/// - `coreValues` is required on an `enum` edge field and rejected on any other
///   type, so a value set can never sit on a field nothing validates against.
/// - a declared `default` must itself be a member of that set, so the schema
///   cannot advertise a value its own value check would reject.
///
/// Note that validating the default is not the same as applying it: nothing
/// fills an omitted edge key in from `default` at write time (edge `required` is
/// likewise unenforced), so an edge created without the key is stored without
/// it. This guard only ensures the declared default is legal if and when
/// something does apply it.
///
/// Enum values must also be unique: a duplicated `value` makes the label
/// ambiguous at display time and silently shadows one of the two entries.
///
/// Unlike `SchemaField`, an edge enum has no `userValues`/`extensible` half —
/// see [`EdgeField::core_values`](nodespace_types::EdgeField::core_values).
fn validate_edge_field_declarations(
    relationships: &[crate::models::schema::SchemaRelationship],
) -> Result<(), MarkdownError> {
    for rel in relationships {
        let Some(edge_fields) = rel.edge_fields.as_deref() else {
            continue;
        };
        for field in edge_fields {
            let is_enum = field.field_type == "enum";

            if !is_enum {
                if field.core_values.is_some() {
                    return Err(MarkdownError::invalid_params(format!(
                        "Edge field '{}' on relationship '{}' declares coreValues but has type \
                         '{}'. coreValues is only meaningful on an enum edge field — set \
                         \"type\": \"enum\" or drop coreValues.",
                        field.name, rel.name, field.field_type
                    )));
                }
                continue;
            }

            let values = field.core_values.as_deref().unwrap_or(&[]);
            if values.is_empty() {
                return Err(MarkdownError::invalid_params(format!(
                    "Enum edge field '{}' on relationship '{}' must declare its permitted values \
                     in coreValues, e.g. \"coreValues\": [{{\"value\": \"owner\", \"label\": \
                     \"Owner\"}}]. An enum with no declared values admits nothing.",
                    field.name, rel.name
                )));
            }

            for (i, ev) in values.iter().enumerate() {
                if values[..i].iter().any(|prev| prev.value == ev.value) {
                    return Err(MarkdownError::invalid_params(format!(
                        "Enum edge field '{}' on relationship '{}' declares the value '{}' more \
                         than once in coreValues. Each value must be unique.",
                        field.name, rel.name, ev.value
                    )));
                }
            }

            if let Some(default) = &field.default {
                let Some(default_str) = default.as_str() else {
                    return Err(MarkdownError::invalid_params(format!(
                        "Enum edge field '{}' on relationship '{}' has a default of {}, but an \
                         enum default must be a string naming one of its coreValues.",
                        field.name,
                        rel.name,
                        json_type_name(default)
                    )));
                };
                if !values.iter().any(|ev| ev.value == default_str) {
                    return Err(MarkdownError::invalid_params(format!(
                        "Enum edge field '{}' on relationship '{}' has default '{}', which is not \
                         one of its declared values ({}).",
                        field.name,
                        rel.name,
                        default_str,
                        values
                            .iter()
                            .map(|ev| ev.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Reject a relationship declaration that omits `reverseName` or
/// `reverseCardinality`, before serde reports it as a bare `missing field`.
///
/// A relationship is declared once — one `relationship` row between the two
/// schema nodes — but read from both ends, so a declaration that names only
/// the forward half leaves the stored edge half-declared: the target's side
/// had to synthesize a label (`"Invoice (Customer)"` rather than `"Invoices"`)
/// and had no cardinality at all, so nothing downstream could say how many
/// sources may point at a node. Naming the inverse is a modeling decision only
/// the author can make, so it is required rather than derived.
///
/// [`SchemaRelationship`](crate::models::schema::SchemaRelationship) carries
/// both as non-`Option` fields, which is what makes this an invariant every
/// reader can rely on. This check exists because that type-level guarantee
/// alone produces `missing field reverseName` — a message naming the key but
/// not what to put in it. Sibling of [`describe_missing_top_level_keys`]: same
/// reason, one level down. Reported per relationship (by name and array
/// position) so a caller repairing a multi-relationship payload knows which
/// entry to fix.
fn describe_missing_reverse_fields(relationships: &Value) -> Result<(), MarkdownError> {
    let Some(entries) = relationships.as_array() else {
        return Ok(());
    };

    for (index, entry) in entries.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            // A non-object entry is a different mistake; serde reports it with
            // a clearer type error than this function could add.
            continue;
        };

        // Absent, null, and (for the name) blank are the same defect from the
        // caller's side: the reverse half was not supplied. Treat them alike so
        // the repair instruction is the same in every case.
        let mut missing: Vec<&str> = Vec::new();
        match obj.get("reverseName") {
            Some(Value::String(s)) if !s.trim().is_empty() => {}
            _ => missing.push("reverseName"),
        }
        match obj.get("reverseCardinality") {
            Some(Value::String(s)) if !s.trim().is_empty() => {}
            _ => missing.push("reverseCardinality"),
        }
        if missing.is_empty() {
            continue;
        }

        // Reflect the caller's own forward declaration back in the example, so
        // the fix is a two-key addition to the entry they already sent rather
        // than a rewrite of it.
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<relationship>");
        let direction = obj
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("out");
        let cardinality = obj
            .get("cardinality")
            .and_then(Value::as_str)
            .unwrap_or("many");
        // `targetType` is legitimately omitted (an untyped relationship — the
        // documented escape hatch for "the target type doesn't exist yet"), so
        // the message has to read correctly without one rather than printing a
        // placeholder where a type name belongs.
        let target_type = obj.get("targetType").and_then(Value::as_str);
        let target_phrase = match target_type {
            Some(tt) => format!("a '{tt}'"),
            None => "the target".to_string(),
        };
        let target_type_key = match target_type {
            Some(tt) => format!("\"targetType\":\"{tt}\","),
            None => String::new(),
        };
        let missing_phrase = match missing.as_slice() {
            [one] => format!("\"{one}\""),
            _ => "\"reverseName\" and \"reverseCardinality\"".to_string(),
        };

        return Err(MarkdownError::invalid_params(format!(
            "Relationship '{name}' (entry {index}) is missing {missing_phrase}. Every relationship \
             must name the edge from BOTH ends: it is stored once and read from either side, \
             so the target's end needs its own name and cardinality. \"reverseName\" is what \
             this edge is called read from {target_phrase} — a name you choose, plural where \
             that end may hold many (\"invoices\", not \"Invoice (Customer)\") — and \
             \"reverseCardinality\" is \"one\" or \"many\", saying how many '{name}' sources \
             may point at one target. Corrected: \
             {{\"name\":\"{name}\",{target_type_key}\"direction\":\"{direction}\",\
             \"cardinality\":\"{cardinality}\",\"reverseName\":\"...\",\
             \"reverseCardinality\":\"many\"}}."
        )));
    }

    Ok(())
}

/// Name of a JSON value's type, for error messages.
pub(crate) fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Input parameters for create_schema
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSchemaParams {
    /// Schema name (e.g., "Invoice", "Customer")
    pub name: String,
    /// Brief prose summary of what this entity type represents. Stored as a
    /// child subtree for semantic discovery; not parsed into fields.
    #[serde(default)]
    pub description: Option<String>,
    /// Explicit field definitions
    #[serde(default)]
    pub fields: Option<Vec<SchemaField>>,
    /// Optional relationship definitions
    #[serde(default)]
    pub relationships: Option<Vec<crate::models::schema::SchemaRelationship>>,
    /// Optional template for computing display title from field values.
    /// Use `{field_name}` tokens that reference fields defined in `fields`.
    /// Example: `"{first_name} {last_name}"` for a customer schema.
    #[serde(default)]
    pub title_template: Option<String>,
    /// Optional template for rendering a compact property summary inline below the node title.
    /// Uses the same `{field_name}` syntax. Evaluated client-side only.
    /// Example: `"{status} · {company}"` → `"Active · Acme Corp"`.
    #[serde(default)]
    pub properties_header_summary_template: Option<String>,
}

/// Output from schema creation
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSchemaOutput {
    /// ID for the generated schema (snake_case of name)
    pub schema_id: String,
    /// Whether this is a core schema
    pub is_core: bool,
    /// Schema version
    pub version: u32,
    /// Schema description
    pub description: String,
    /// List of created fields
    pub fields: Vec<SchemaField>,
    /// List of created relationships
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<crate::models::schema::SchemaRelationship>,
    /// Optional warnings, e.g. a field name shadowing a reserved core property
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

/// Create a custom schema with fields and relationships
///
/// # Tool: create_schema
///
/// Creates a new schema definition from explicit field and relationship
/// definitions. Field names are stored as given (bare, not namespace-prefixed).
/// A name colliding with a reserved core property is reported in `warnings`.
///
/// # Parameters
/// - `name`: Schema name (e.g., "Invoice", "Customer")
/// - `description`: Optional prose summary, stored for semantic discovery only
/// - `fields`: Explicit field definitions
/// - `relationships`: Optional relationship definitions to other schemas
///
/// # Returns
/// - `schema_id`: Generated schema ID (snake_case)
/// - `fields`: List of created fields
/// - `relationships`: List of created relationships
/// - `warnings`: e.g. a field name shadowing a reserved core property
///
/// # Errors
/// - `INVALID_PARAMS`: If name is empty or `fields` is missing
/// - `INTERNAL_ERROR`: If schema creation fails
pub async fn handle_create_schema(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MarkdownError> {
    // Locate malformed field entries before serde sees them. Deserializing the
    // whole payload at once reports only the missing key ("missing field
    // `type`") with no indication of WHICH array element is at fault, which
    // leaves an LLM caller unable to repair the call: it re-sends with a
    // different part mutated and degrades the arguments further on each retry.
    describe_malformed_fields(&params, "fields")?;
    // Runs after the per-entry check so a payload wrong in both places reports
    // the field problems first: those are what the caller must rebuild, whereas
    // a missing `name` is a one-key addition to an otherwise-correct call.
    describe_missing_top_level_keys(&params)?;

    // Before serde: a relationship missing its reverse half deserializes to a
    // bare "missing field" naming the key but not what the reverse half is FOR
    // or what value to choose.
    if let Some(relationships) = params.get("relationships") {
        describe_missing_reverse_fields(relationships)?;
    }

    // Runs AFTER both checks, not before: an entry whose every value is null
    // carries no information to validate, but removing it first renumbers the
    // array and makes the positions those checks report point at the wrong
    // entry. `describe_malformed_fields` skips such entries itself and rejects
    // an array made only of them, so by here at least one real field survives.
    let mut params = params;
    drop_empty_field_entries(&mut params, "fields");

    let params: CreateSchemaParams = serde_json::from_value(params)
        .map_err(|e| MarkdownError::invalid_params(format!("{e}")))?;

    if params.name.trim().is_empty() {
        return Err(MarkdownError::invalid_params(
            "name cannot be empty".to_string(),
        ));
    }

    // `fields` absent from the request (`None`) is always an error — the caller
    // never defined what the type holds. `fields: []` (`Some(vec![])`) is always
    // valid: an explicit, deliberate choice such as a relationship-only schema.
    let Some(explicit_fields) = params.fields else {
        return Err(MarkdownError::invalid_params(
            "\"fields\" is required. List every field explicitly, e.g. \
             [{\"name\":\"amount\",\"type\":\"number\"}]. \"description\" is a prose \
             summary only and is not parsed into fields."
                .to_string(),
        ));
    };

    let (mut stored_fields, warnings) = warn_reserved_property_names(explicit_fields);
    // A brand-new schema has no existing fields to collide with — every
    // field in this batch is only checked against its own siblings.
    apply_friendly_name_defaults(&[], &mut stored_fields);

    // Get relationships (default to empty)
    let relationships = params.relationships.unwrap_or_default();

    // Generate schema ID. Needed before validation so a relationship targeting
    // the schema this call is creating can be recognised as self-referential.
    let schema_id = crate::services::node_service::normalize_schema_id(&params.name);

    // Reject reserved relationship names and dangling targetTypes BEFORE the
    // schema node exists, so a bad declaration can't leave a half-created
    // schema behind. (`set_schema_relationships` re-checks reserved names —
    // via the same shared predicate — as the write-path invariant.) A target
    // naming `schema_id` itself is the one case that cannot dangle, so it is
    // accepted here rather than deferred to a post-write check — validation
    // still runs in full before the first write.
    //
    // A name of only punctuation (`"!!!"`) passes the `trim().is_empty()` check
    // above and normalizes to `""`. `create_node_with_parent` rejects that
    // downstream, but an empty `pending_schema_id` would make the exemption
    // vacuously true for `targetType: ""` first, trading the actionable
    // targetType error for a lower-level one. Don't offer an id nothing can
    // resolve.
    let pending_schema_id = (!schema_id.is_empty()).then_some(schema_id.as_str());
    reject_reserved_relationship_names(&relationships)?;
    validate_edge_field_declarations(&relationships)?;
    validate_relationship_targets_exist(node_service, &relationships, pending_schema_id).await?;

    // Check if schema already exists — return a clear error so the agent knows
    // to use create_node instead of retrying create_schema. The rejection
    // carries the existing type's real, rendered definition: without it the
    // agent has no fact to report and fills the gap by describing the fields
    // from the (rejected) call it just made, presenting them as confirmed.
    if let Ok(Some(existing_schema)) = node_service.get_schema_node(&schema_id).await {
        let existing_definition =
            crate::ops::entity_types_block::EntityTypeDescriptor::from_schema(&existing_schema)
                .render_line();
        return Err(MarkdownError::invalid_params(format!(
            "Schema '{}' already exists — it was NOT modified, and the fields in this call \
             were NOT applied. Its actual definition is: {}. Use create_node with \
             node_type='{}' to create instances. Describe the type using only the definition \
             above.",
            params.name, existing_definition, schema_id
        )));
    }

    // Schema properties (description is stored as a child node subtree, and
    // relationship declarations as relationship-table rows — neither lives in
    // properties)
    let description_text = params
        .description
        .clone()
        .unwrap_or_else(|| format!("Schema for {}", params.name));
    let mut properties = serde_json::json!({
        "isCore": false,
        "schemaVersion": 1,
        "fields": &stored_fields,
    });
    if let Some(ref template) = params.title_template {
        properties["titleTemplate"] = serde_json::Value::String(template.clone());
    }
    if let Some(ref template) = params.properties_header_summary_template {
        properties["propertiesHeaderSummaryTemplate"] = serde_json::Value::String(template.clone());
    }

    // Create schema node params — no explicit ID; create_node_with_parent derives it from content
    let schema_node_params = CreateNodeParams {
        id: None,
        node_type: "schema".to_string(),
        content: params.name.clone(),
        parent_id: None,
        position: crate::services::InsertPositionOwned::End,
        properties,
        lifecycle_status: None,
    };

    // ADR-069 §1b/S3: the schema node, its relationship declarations, and its
    // description subtree land in ONE transaction. Previously three
    // independent atomic writes — a failure on the second left a schema node
    // live with zero relationship rows; a failure on the third left it with
    // no description subtree, semantically undiscoverable via embedding
    // search until someone re-ran update_schema with a description. Both are
    // now impossible: any failure here rolls back the whole create.
    let relationships_for_tx = relationships.clone();
    let description_text_for_tx = description_text.clone();
    let node_service_for_tx = Arc::clone(node_service);
    // Same id `schema_id` above already computed (schema nodes derive their id
    // from content) — kept as its own binding because it's the value the
    // transaction closure actually produced, not merely asserted in advance.
    let _created_schema_id: String = node_service
        .with_transaction(move |tx| {
            let node_service = Arc::clone(&node_service_for_tx);
            let relationships = relationships_for_tx.clone();
            let description_text = description_text_for_tx.clone();
            Box::pin(async move {
                // create_node_with_parent_in_tx already returns
                // NodeServiceError directly — propagate it as-is rather than
                // flattening to transaction_failed, so a genuine validation
                // failure (InvalidUpdate, UnknownNodeType, ...) stays
                // distinguishable from the transaction machinery itself
                // breaking (ADR-069 §2a). The outer map_err below
                // discriminates on this.
                let created_schema_id = node_service
                    .create_node_with_parent_in_tx(tx, schema_node_params)
                    .await?;

                if !relationships.is_empty() {
                    node_service
                        .set_schema_relationships_in_tx(tx, &created_schema_id, &relationships)
                        .await?;
                }

                create_description_subtree_in_tx(
                    &node_service,
                    tx,
                    &created_schema_id,
                    &description_text,
                )
                .await
                .map_err(|e| match e {
                    MarkdownError::InvalidParams(msg) => NodeServiceError::invalid_update(msg),
                    other => NodeServiceError::transaction_failed(other.to_string()),
                })?;

                Ok(created_schema_id)
            })
        })
        .await
        .map_err(|e| match e {
            NodeServiceError::InvalidUpdate(_) => MarkdownError::invalid_params(e.to_string()),
            other => MarkdownError::internal_error(format!(
                "Failed to create schema '{}': {}",
                schema_id, other
            )),
        })?;

    let output = CreateSchemaOutput {
        schema_id: schema_id.clone(),
        is_core: false,
        version: 1,
        description: description_text,
        fields: stored_fields,
        relationships,
        warnings: if warnings.is_empty() {
            None
        } else {
            Some(warnings)
        },
    };

    serde_json::to_value(&output)
        .map_err(|e| MarkdownError::internal_error(format!("Failed to serialize output: {}", e)))
}

/// A single field rename operation within update_schema
///
/// Two conceptually different renames share this shape:
/// - **identity rename** (`from` != `to`): rekeys `name`, migrates every
///   existing node's property data, and is breaking for `titleTemplate`/CEL/
///   query-filter references — unchanged from before `friendly_name` existed.
/// - **display rename** (`from` == `to`, `friendly_name` set): updates only
///   the display label, migrates nothing. Also legal combined with an
///   identity rename in one entry (both `to` and `friendly_name` set) —
///   applied atomically as one schema update rather than two round trips.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldRename {
    /// Current field name
    pub from: String,
    /// New field name (pass the same value as `from` for a display-only
    /// rename that changes `friendly_name` without migrating data)
    pub to: String,
    /// New display label for this field. Optional — omit to leave
    /// `friendly_name` exactly as stored (including when it was auto-derived
    /// from the old `name` and is now stale; see
    /// `NodeService::rename_schema_field`'s doc comment for why that is not
    /// re-derived automatically).
    #[serde(default)]
    pub friendly_name: Option<String>,
}

/// Parameters for update_schema (batch operations)
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSchemaParams {
    /// Schema ID to update
    pub schema_id: String,
    /// Fields to add
    #[serde(default)]
    pub add_fields: Option<Vec<SchemaField>>,
    /// Field names to remove
    #[serde(default)]
    pub remove_fields: Option<Vec<String>>,
    /// Field renames — rekeys property data on all existing nodes of this type
    /// and updates the schema definition atomically.
    #[serde(default)]
    pub rename_fields: Option<Vec<FieldRename>>,
    /// Relationships to add
    #[serde(default)]
    pub add_relationships: Option<Vec<crate::models::schema::SchemaRelationship>>,
    /// Relationship names to remove (soft-delete: edge table preserved)
    #[serde(default)]
    pub remove_relationships: Option<Vec<String>>,
    /// New description (optional)
    #[serde(default)]
    pub description: Option<String>,
    /// Set or update the title template. Pass `null` (absent) to leave unchanged.
    /// Use `{field_name}` tokens referencing fields defined in the schema.
    /// Example: `"{first_name} {last_name}"`
    #[serde(default)]
    pub title_template: Option<String>,
    /// Set or update the properties header summary template. Pass `null` (absent) to leave unchanged.
    /// Uses the same `{field_name}` syntax. Evaluated client-side only.
    /// Example: `"{status} · {company}"`
    #[serde(default)]
    pub properties_header_summary_template: Option<String>,
    /// If true, proceed with the schema update even if active plays would be
    /// affected. If false (default), return an error listing the affected plays.
    #[serde(default)]
    pub force: bool,
}

/// Output for schema update operations
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaUpdateOutput {
    pub schema_id: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields_added: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields_removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields_renamed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationships_added: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationships_removed: Option<usize>,
    /// Plays affected by this schema change (present when force=true and plays were affected)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_plays: Option<Vec<String>>,
}

/// Update a schema with multiple changes
///
/// # Tool: update_schema
///
/// Batch update a schema's fields and relationships. Useful when making
/// multiple changes at once. Relationship changes are persisted as
/// relationship-table declaration edges (see `crate::models::schema`);
/// removing a declaration that live instance edges depend on is rejected.
///
/// # Parameters
/// - `schema_id`: ID of the schema to update
/// - `add_fields`: Fields to add
/// - `remove_fields`: Field names to remove
/// - `add_relationships`: Relationships to add
/// - `remove_relationships`: Relationship names to remove (soft-delete)
/// - `description`: New description (optional)
pub async fn handle_update_schema(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MarkdownError> {
    // See `describe_malformed_fields` — locate a bad entry before serde reports
    // only the absent key with no position.
    describe_malformed_fields(&params, "add_fields")?;

    // Same reverse-half check `handle_create_schema` runs, on the key that
    // carries new declarations here. A relationship added by an update is a new
    // stored edge and must be named from both ends exactly as one declared at
    // create time.
    if let Some(relationships) = params.get("add_relationships") {
        describe_missing_reverse_fields(relationships)?;
    }

    // `add_fields` is `fields` under another name and takes the same treatment:
    // the model appends an informationless `{"description":null,"name":null}`
    // here too, and failing an otherwise-correct batch over it drives the same
    // degrading-retry loop. `describe_malformed_fields` above already skips
    // these entries, so without this they would reach serde and fail with the
    // bare error that check exists to replace.
    let mut params = params;
    drop_empty_field_entries(&mut params, "add_fields");

    let mut params: UpdateSchemaParams = serde_json::from_value(params)
        .map_err(|e| MarkdownError::invalid_params(format!("{e}")))?;

    // --- Phase 0: Verify schema exists, validate renames, run play impact check ---
    // Schema existence is verified upfront so rename/play validation errors are reported
    // before any mutations execute. The fetched schema is also the pre-mutation snapshot the
    // protection-level and grammar checks below validate `remove_fields`/`rename_fields`
    // against, before Phase 1 or Phase 2 touch anything.
    let schema_before = node_service
        .get_schema_node(&params.schema_id)
        .await
        .map_err(|e| MarkdownError::internal_error(format!("Failed to get schema: {}", e)))?
        .ok_or_else(|| {
            MarkdownError::invalid_params(format!("Schema '{}' not found", params.schema_id))
        })?;

    // Validate renames before executing any mutations (including the play guard below)
    if let Some(ref renames) = params.rename_fields {
        let mut seen_sources = std::collections::HashSet::new();
        let mut seen_destinations = std::collections::HashSet::new();
        for rename in renames {
            if rename.from.trim().is_empty() || rename.to.trim().is_empty() {
                return Err(MarkdownError::invalid_params(
                    "rename_fields entries must have non-empty 'from' and 'to'".to_string(),
                ));
            }
            // `from == to` is legal exactly when it carries a `friendly_name`
            // — a display-only rename that changes the label without
            // migrating data (see `FieldRename`'s doc comment). With no
            // `friendly_name`, `from == to` changes nothing and is rejected
            // as before.
            if rename.from == rename.to && rename.friendly_name.is_none() {
                return Err(MarkdownError::invalid_params(format!(
                    "rename_fields: 'from' and 'to' are the same with no 'friendly_name' \
                     given, so this entry would change nothing: '{}'",
                    rename.from
                )));
            }
            if !seen_sources.insert(&rename.from) {
                return Err(MarkdownError::invalid_params(format!(
                    "rename_fields: duplicate source field name '{}'",
                    rename.from
                )));
            }
            if !seen_destinations.insert(&rename.to) {
                return Err(MarkdownError::invalid_params(format!(
                    "rename_fields: duplicate destination field name '{}'",
                    rename.to
                )));
            }
        }
    }

    // --- Phase 0: Reject mutations that touch a Core/System-protected field ---
    //
    // `SchemaField` carries a protection level, and `SchemaNode::can_delete_field`
    // / `can_modify_field` exist to enforce it — a Core/System field (e.g.
    // `task.status`) must never be deletable or renameable by a caller. Neither
    // was ever consulted on this path: `remove_fields` below deletes by name
    // alone, and `rename_schema_field` rekeys by name alone, so either could
    // permanently strip a protected field from a core schema with no recovery
    // path through the API.
    //
    // Checked here, against `schema_before` (the schema as it stood before this
    // call touched anything), and before Phase 1 executes a single rename: a
    // rename migrates every existing node's property data and rewrites the
    // schema as it goes, so validating the whole batch upfront — rather than
    // per-entry as Phase 1 runs — keeps a batch with one bad entry from
    // partially applying before the rejection is returned.
    if let Some(ref remove_names) = params.remove_fields {
        for name in remove_names {
            if let Some(field) = schema_before.get_field(name) {
                if !schema_before.can_delete_field(name) {
                    return Err(MarkdownError::invalid_params(format!(
                        "Field '{}' on schema '{}' is {}-protected and cannot be removed — \
                         only User-protected fields may be removed via update_schema. Core \
                         and System fields are immutable through this API.",
                        name, params.schema_id, field.protection
                    )));
                }
            }
        }
    }

    if let Some(ref renames) = params.rename_fields {
        for rename in renames {
            if let Some(field) = schema_before.get_field(&rename.from) {
                if !schema_before.can_modify_field(&rename.from) {
                    return Err(MarkdownError::invalid_params(format!(
                        "Field '{}' on schema '{}' is {}-protected and cannot be renamed or \
                         relabeled — only User-protected fields may be modified via \
                         update_schema. Core and System fields are immutable through this API.",
                        rename.from, params.schema_id, field.protection
                    )));
                }
            }

            // The destination name's grammar must be rejected here too, before
            // Phase 1 runs — not left to Phase 2's re-validation. An identity
            // rename is persisted by `rename_schema_field` immediately,
            // migrating every node's property data as it executes; Phase 2's
            // check runs only after that migration has already committed, so a
            // malformed name (spaces, punctuation, more than one namespace
            // prefix) would already be durably written to the schema and every
            // node instance by the time it was caught.
            crate::behaviors::validate_schema_field_name(&rename.to).map_err(|e| {
                MarkdownError::invalid_params(format!(
                    "rename_fields: invalid destination name '{}': {}",
                    rename.to, e
                ))
            })?;
        }
    }

    // Check if any active plays would be affected by this schema change.
    // Done before any mutations so a blocked rename doesn't partially execute.
    let affected =
        crate::playbook::validation::check_schema_change_impact(&params.schema_id, node_service)
            .await
            .map_err(|e| MarkdownError::internal_error(format!("Impact analysis failed: {}", e)))?;

    if !affected.is_empty() && !params.force {
        let names: Vec<String> = affected.iter().map(|a| a.to_string()).collect();
        return Err(MarkdownError::invalid_params(format!(
            "Schema change would affect {} active play(s): {}. Use force=true to proceed.",
            affected.len(),
            names.join("; ")
        )));
    }

    let affected_names: Option<Vec<String>> = if !affected.is_empty() {
        Some(
            affected
                .iter()
                .map(|a| format!("{} ({})", a.play_name, a.play_id))
                .collect(),
        )
    } else {
        None
    };

    // --- Phase 0: Reject unprefixed field names on a type NodeSpace owns ---
    //
    // A bare name on a core type can be claimed by a core property in a future
    // release, and the user's field would then collide with it. User-defined
    // types carry no such risk — NodeSpace never adds core fields to them — so
    // their fields are stored under the caller's names.
    //
    // This runs before Phase 1 rather than validating the final field list,
    // because `rename_schema_field` migrates node property data and rewrites the
    // schema per rename, committing each one as it goes. Validating afterwards
    // would return an error with the offending rename already persisted across
    // every node instance. Both routes that can introduce a name are checked
    // here: `add_fields` supplies one directly, and `rename_fields` can turn an
    // already-prefixed field into a bare one.
    if schema_is_core(node_service, &params.schema_id).await? {
        let reject = |name: &str, how: &str| {
            MarkdownError::invalid_params(format!(
                "Field '{name}' {how} core schema '{}' must carry a namespace prefix \
                 (e.g. 'custom:{name}'). Unprefixed names on core types are reserved \
                 for core properties.",
                params.schema_id
            ))
        };

        if let Some(ref add_fields) = params.add_fields {
            for field in add_fields {
                if !has_namespace_prefix(&field.name) {
                    return Err(reject(&field.name, "added to"));
                }
            }
        }

        if let Some(ref renames) = params.rename_fields {
            for rename in renames {
                if !has_namespace_prefix(&rename.to) {
                    return Err(reject(&rename.to, "renamed on"));
                }
            }
        }
    }

    // --- Phase 1: Process renames (identity renames migrate data; display-only renames do not) ---
    let mut fields_renamed = 0;
    if let Some(ref renames) = params.rename_fields {
        for rename in renames {
            if rename.from == rename.to {
                // Display-only: `friendly_name` is guaranteed present here —
                // the validation pass above rejects `from == to` with no
                // `friendly_name`. No node property data is touched.
                let friendly_name = rename.friendly_name.as_deref().unwrap_or_default();
                node_service
                    .update_schema_field_friendly_name(
                        &params.schema_id,
                        &rename.from,
                        friendly_name,
                    )
                    .await
                    .map_err(|e| {
                        MarkdownError::invalid_params(format!(
                            "Field friendly_name update failed: {}",
                            e
                        ))
                    })?;
            } else {
                // Identity rename: rekeys `name` and migrates every existing
                // node's property data. `rename_schema_field` deliberately
                // leaves `friendly_name` untouched (see its own doc comment),
                // so an entry that ALSO supplies `friendly_name` applies that
                // as a second, immediately-following update — one logical
                // change, two schema-definition writes rather than asking
                // the caller to split it across two `rename_fields` entries.
                node_service
                    .rename_schema_field(&params.schema_id, &rename.from, &rename.to)
                    .await
                    .map_err(|e| {
                        MarkdownError::invalid_params(format!("Field rename failed: {}", e))
                    })?;
                if let Some(ref friendly_name) = rename.friendly_name {
                    // Not atomic with the rename above — they are two
                    // separate schema-node writes (the data migration inside
                    // `rename_schema_field` is itself a distinct step from
                    // the schema-definition rewrite, so there is no single
                    // write these two calls could be merged into that would
                    // also cover it). If this second call fails, the rename
                    // has ALREADY happened: the field now exists under `to`,
                    // and a retry of this same `rename_fields` entry would
                    // fail differently (`from` no longer exists). Say so
                    // explicitly rather than returning an error that reads
                    // like nothing happened — the caller (model, then user)
                    // needs to know the storage key already changed.
                    node_service
                        .update_schema_field_friendly_name(
                            &params.schema_id,
                            &rename.to,
                            friendly_name,
                        )
                        .await
                        .map_err(|e| {
                            MarkdownError::invalid_params(format!(
                                "Field '{}' was successfully renamed to '{}' (and its node data \
                                 migrated), but updating its display label failed: {}. Do NOT \
                                 retry with the original 'from'/'to' pair — the field now exists \
                                 as '{}'. To retry the label update alone, send \
                                 {{\"from\": \"{}\", \"to\": \"{}\", \"friendlyName\": ...}}.",
                                rename.from, rename.to, e, rename.to, rename.to, rename.to
                            ))
                        })?;
                }
            }
            fields_renamed += 1;
        }
    }

    // --- Phase 2: Re-fetch schema (reflects any renames applied above) ---
    let schema = node_service
        .get_schema_node(&params.schema_id)
        .await
        .map_err(|e| MarkdownError::internal_error(format!("Failed to get schema: {}", e)))?
        .ok_or_else(|| {
            MarkdownError::invalid_params(format!(
                "Schema '{}' not found after renames",
                params.schema_id
            ))
        })?;

    // Process fields
    let mut fields = schema.fields.clone();
    let mut fields_added = 0;
    let mut fields_removed = 0;

    if let Some(remove_names) = &params.remove_fields {
        let before = fields.len();
        fields.retain(|f| !remove_names.contains(&f.name));
        fields_removed = before - fields.len();
    }

    if let Some(ref mut add_fields) = params.add_fields {
        // Check for duplicates before adding
        for field in add_fields.iter() {
            if fields.iter().any(|f| f.name == field.name) {
                return Err(MarkdownError::invalid_params(format!(
                    "Field '{}' already exists in schema '{}'",
                    field.name, params.schema_id
                )));
            }
        }
        // Write-boundary friendly_name defaulting, with the current field
        // set (existing + removals already applied) as collision context —
        // see `apply_friendly_name_defaults`.
        apply_friendly_name_defaults(&fields, add_fields);
        fields_added = add_fields.len();
        fields.extend(add_fields.clone());
    }

    // Process relationships (`schema.relationships` arrives hydrated from the
    // relationship table; the final set is persisted back through
    // `set_schema_relationships` below)
    let mut relationships = schema.relationships.clone();
    let mut relationships_added = 0;
    let mut relationships_removed = 0;

    if let Some(remove_names) = &params.remove_relationships {
        let before = relationships.len();
        relationships.retain(|r| !remove_names.contains(&r.name));
        relationships_removed = before - relationships.len();
    }

    if let Some(ref add_rels) = params.add_relationships {
        // Check for duplicates before adding
        for rel in add_rels {
            if relationships.iter().any(|r| r.name == rel.name) {
                return Err(MarkdownError::invalid_params(format!(
                    "Relationship '{}' already exists in schema '{}'",
                    rel.name, params.schema_id
                )));
            }
        }
        reject_reserved_relationship_names(add_rels)?;
        validate_edge_field_declarations(add_rels)?;
        // Reject a targetType that doesn't exist yet — see
        // validate_relationship_targets_exist. No pending schema here: the
        // schema being edited was loaded above, so a relationship targeting
        // it resolves through the ordinary existence lookup.
        validate_relationship_targets_exist(node_service, add_rels, None).await?;
        relationships_added = add_rels.len();
        relationships.extend(add_rels.clone());
    }

    // Resolve title_template: use new value if provided, otherwise keep existing
    let title_template = params.title_template.or(schema.title_template);

    // Resolve properties_header_summary_template: use new value if provided, otherwise keep existing
    let properties_header_summary_template = params
        .properties_header_summary_template
        .or(schema.properties_header_summary_template);

    // Build updated properties (description is stored as a child subtree and
    // relationship declarations as relationship-table rows — neither lives in
    // properties)
    let mut properties = serde_json::json!({
        "isCore": schema.is_core,
        "schemaVersion": schema.schema_version,
        "fields": fields,
    });
    if let Some(ref template) = title_template {
        properties["titleTemplate"] = serde_json::Value::String(template.clone());
    }
    if let Some(ref template) = properties_header_summary_template {
        properties["propertiesHeaderSummaryTemplate"] = serde_json::Value::String(template.clone());
    }

    // Validate the updated schema before saving (update_node_unchecked bypasses the behavior
    // pipeline, so we run SchemaNodeBehavior validation explicitly here)
    let temp_node = Node::new(
        "schema".to_string(),
        schema.content.clone(),
        properties.clone(),
    );
    let mut updated_schema = SchemaNode::from_node(temp_node).map_err(|e| {
        MarkdownError::invalid_params(format!("Failed to build schema for validation: {}", e))
    })?;
    // from_node never populates relationships (they aren't in properties);
    // hand the validator the final declaration set explicitly.
    updated_schema.relationships = relationships.clone();
    SchemaNodeBehavior
        .validate_schema_node(&updated_schema)
        .map_err(|e| MarkdownError::invalid_params(format!("Schema validation failed: {}", e)))?;

    // ADR-069 §1b/S3, closing F2's most damaging step: relationship
    // declarations, the fields/templates update, and the description
    // subtree replace land in ONE transaction. Previously up to three
    // independent atomic writes — a failure updating fields after
    // declarations had already changed left `get_schema_with_relationships`
    // returning an internally inconsistent `SchemaNode` (new relationships,
    // old fields). Any failure in this group now rolls back all of it.
    //
    // Declarations still go first within the group: this is where the
    // live-instance-edge guard runs (removing/retargeting a declaration with
    // edges under it is rejected), and a rejection must leave the schema
    // fully untouched — fields included. That ordering guarantee is now
    // backed by the transaction rather than by hoping nothing fails after it.
    //
    // Phase 1 (per-rename field renames above) is NOT included in this
    // boundary — each rename is its own already-atomic unit (S3's
    // `rename_schema_field` fix), and merging N independent renames plus this
    // group into one transaction is a larger composition than this pass
    // covers. The existing "Do NOT retry with the original pair" guidance
    // for a friendly_name failure after a successful rename (Phase 1 above)
    // therefore still applies — that compensation-by-error-message is
    // unchanged by this fix.
    let schema_id_for_tx = params.schema_id.clone();
    let relationships_for_tx = relationships.clone();
    let description_for_tx = params.description.clone();
    let node_service_for_tx = Arc::clone(node_service);
    node_service
        .with_transaction(move |tx| {
            let node_service = Arc::clone(&node_service_for_tx);
            let schema_id = schema_id_for_tx.clone();
            let relationships = relationships_for_tx.clone();
            let description = description_for_tx.clone();
            let properties = properties.clone();
            Box::pin(async move {
                if relationships_added > 0 || relationships_removed > 0 {
                    node_service
                        .set_schema_relationships_in_tx(tx, &schema_id, &relationships)
                        .await?;
                }

                let update = NodeUpdate {
                    properties: Some(properties),
                    ..Default::default()
                };
                node_service
                    .update_node_unchecked_in_tx(tx, &schema_id, update)
                    .await?;

                if let Some(ref new_description) = description {
                    crate::db::SqliteStore::delete_children_subtree_unchecked_in_tx(
                        tx.store_tx(),
                        &schema_id,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::transaction_failed(format!(
                            "Failed to delete description subtree for schema '{schema_id}': {e}"
                        ))
                    })?;
                    create_description_subtree_in_tx(
                        &node_service,
                        tx,
                        &schema_id,
                        new_description,
                    )
                    .await
                    .map_err(|e| {
                        // Preserve InvalidParams as InvalidUpdate rather than
                        // flattening to transaction_failed: the outer match
                        // below discriminates InvalidUpdate -> invalid_params
                        // (a client mistake) from everything else ->
                        // internal_error, and TransactionFailed is reserved
                        // for the commit/rollback machinery itself breaking
                        // (ADR-069 §2a), not a validation failure surfaced
                        // through a different error type at this boundary.
                        match e {
                            MarkdownError::InvalidParams(msg) => {
                                NodeServiceError::invalid_update(msg)
                            }
                            other => NodeServiceError::transaction_failed(other.to_string()),
                        }
                    })?;
                }

                Ok(())
            })
        })
        .await
        .map_err(|e| match e {
            NodeServiceError::InvalidUpdate(_) => MarkdownError::invalid_params(e.to_string()),
            other => MarkdownError::internal_error(format!("Failed to update schema: {}", other)),
        })?;

    let output = SchemaUpdateOutput {
        schema_id: params.schema_id,
        success: true,
        fields_added: if fields_added > 0 {
            Some(fields_added)
        } else {
            None
        },
        fields_removed: if fields_removed > 0 {
            Some(fields_removed)
        } else {
            None
        },
        fields_renamed: if fields_renamed > 0 {
            Some(fields_renamed)
        } else {
            None
        },
        relationships_added: if relationships_added > 0 {
            Some(relationships_added)
        } else {
            None
        },
        relationships_removed: if relationships_removed > 0 {
            Some(relationships_removed)
        } else {
            None
        },
        affected_plays: affected_names,
    };

    serde_json::to_value(&output)
        .map_err(|e| MarkdownError::internal_error(format!("Failed to serialize output: {}", e)))
}

// ============================================================================
// Description Subtree Helpers
// ============================================================================

/// Create a description child subtree under a schema node.
///
/// Parses the markdown description into node types (text, header, etc.) and
/// inserts them as children of the schema node via bulk insert. The subtree
/// is then included in the schema's embedding via `get_aggregated_content`,
/// enabling synonym-based semantic discovery of schemas.
///
/// `_in_tx`-only (ADR-069 §1b/S3): both callers (`handle_create_schema`,
/// `handle_update_schema`) compose this into an outer transaction via
/// `NodeService::bulk_create_hierarchy_in_tx` — there is deliberately no
/// standalone non-`_in_tx` version, since nothing calls this outside a
/// transaction.
async fn create_description_subtree_in_tx(
    node_service: &Arc<NodeService>,
    tx: &crate::services::node_service::NodeServiceTx<'_>,
    schema_id: &str,
    description: &str,
) -> Result<(), MarkdownError> {
    use crate::markdown::prepare_nodes_from_markdown;

    if description.trim().is_empty() {
        return Ok(());
    }

    let prepared = prepare_nodes_from_markdown(description, Some(schema_id.to_string()))?;
    if prepared.is_empty() {
        return Ok(());
    }

    let bulk_nodes: Vec<(
        String,
        String,
        String,
        Option<String>,
        f64,
        serde_json::Value,
    )> = prepared
        .into_iter()
        .map(|n| {
            (
                n.id,
                n.node_type,
                n.content,
                n.parent_id,
                n.order,
                n.properties,
            )
        })
        .collect();

    node_service
        .bulk_create_hierarchy_in_tx(tx, bulk_nodes)
        .await
        .map_err(|e| {
            // Preserve the client-mistake/server-fault distinction rather
            // than flattening everything to `internal_error`: a genuinely
            // invalid description (e.g. content failing behavior or schema
            // field validation) is a client mistake, not a server fault,
            // and callers one layer up (`handle_update_schema`) discriminate
            // on this to decide invalid_params vs. internal_error.
            // `prepare_bulk_hierarchy_nodes` can surface either variant
            // depending on which validator rejects first (behaviors ->
            // ValidationFailed, schema field checks -> InvalidUpdate).
            match e {
                NodeServiceError::InvalidUpdate(msg) => MarkdownError::invalid_params(format!(
                    "Failed to create description subtree for schema '{schema_id}': {msg}"
                )),
                NodeServiceError::ValidationFailed(msg) => MarkdownError::invalid_params(format!(
                    "Failed to create description subtree for schema '{schema_id}': {msg}"
                )),
                other => MarkdownError::internal_error(format!(
                    "Failed to create description subtree for schema '{}': {}",
                    schema_id, other
                )),
            }
        })?;

    Ok(())
}

#[cfg(test)]
#[path = "schema_test.rs"]
mod schema_test;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::SchemaProtectionLevel;

    #[test]
    fn test_has_namespace_prefix() {
        assert!(has_namespace_prefix("custom:capacity"));
        assert!(has_namespace_prefix("org:region"));

        assert!(!has_namespace_prefix("capacity"));

        // Malformed forms are not prefixes. `validate_schema_field_name` also
        // rejects these, but this must not depend on that happening first.
        assert!(!has_namespace_prefix(":capacity"));
        assert!(!has_namespace_prefix("custom:"));
        assert!(!has_namespace_prefix("a:b:c"));
        assert!(!has_namespace_prefix(":"));
        assert!(!has_namespace_prefix(""));
    }

    #[test]
    fn test_normalize_schema_id() {
        use crate::services::node_service::normalize_schema_id;
        assert_eq!(normalize_schema_id("Invoice"), "invoice");
        assert_eq!(normalize_schema_id("Customer Profile"), "customer_profile");
        assert_eq!(normalize_schema_id("code_block"), "code_block");
        assert_eq!(normalize_schema_id("Project"), "project");
    }

    #[test]
    fn test_integration_schema_id_generation() {
        use crate::services::node_service::normalize_schema_id;
        let entity_name = "Customer Invoice";
        let schema_id = normalize_schema_id(entity_name);
        // normalize_schema_id joins on '_' (see its dedicated unit tests); the core
        // hardcoded schema ids that use hyphens (code-block, …) are not generated
        // through this path.
        assert_eq!(schema_id, "customer_invoice");
    }

    fn field(name: &str) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            friendly_name: name.to_string(),
            field_type: "string".to_string(),
            local_only: false,
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
        }
    }

    #[test]
    fn test_warn_reserved_property_names_warns_on_collision() {
        let (fields, warnings) =
            warn_reserved_property_names(vec![field("status"), field("capacity")]);

        // Explicit fields are stored verbatim — never silently rewritten.
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "status");
        assert_eq!(fields[1].name, "capacity");

        assert!(
            warnings
                .iter()
                .any(|w| w.contains("status") && w.contains("reserved core property")),
            "Expected a reserved-core-property warning for 'status', got {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("capacity")),
            "Unreserved field name should not warn, got {warnings:?}"
        );
    }

    #[test]
    fn test_warn_reserved_property_names_no_collision_no_warnings() {
        let (fields, warnings) = warn_reserved_property_names(vec![field("email")]);
        assert_eq!(fields.len(), 1);
        assert!(warnings.is_empty());
    }

    // --- validate_edge_field_declarations ------------------------------------

    use crate::models::schema::{
        EdgeField, EnumValue, RelationshipCardinality, RelationshipDirection, SchemaRelationship,
    };

    /// A relationship carrying exactly the given edge fields.
    fn rel_with_edge_fields(edge_fields: Vec<EdgeField>) -> SchemaRelationship {
        SchemaRelationship {
            name: "member_of_org".to_string(),
            target_type: Some("collection".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: "orgs".to_string(),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: Some(edge_fields),
            description: None,
        }
    }

    fn edge_field(name: &str, field_type: &str) -> EdgeField {
        EdgeField {
            name: name.to_string(),
            field_type: field_type.to_string(),
            core_values: None,
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        }
    }

    fn rbac_values() -> Vec<EnumValue> {
        vec![
            EnumValue {
                value: "owner".to_string(),
                label: "Owner".to_string(),
            },
            EnumValue {
                value: "editor".to_string(),
                label: "Editor".to_string(),
            },
            EnumValue {
                value: "viewer".to_string(),
                label: "Viewer".to_string(),
            },
        ]
    }

    #[test]
    fn edge_enum_with_values_and_valid_default_is_accepted() {
        // The motivating RBAC shape from the issue.
        let mut role = edge_field("role", "enum");
        role.core_values = Some(rbac_values());
        role.required = Some(true);
        role.default = Some(serde_json::json!("viewer"));

        let rels = vec![rel_with_edge_fields(vec![role])];
        assert!(validate_edge_field_declarations(&rels).is_ok());
    }

    #[test]
    fn edge_enum_without_core_values_is_rejected() {
        let rels = vec![rel_with_edge_fields(vec![edge_field("role", "enum")])];
        let err = validate_edge_field_declarations(&rels).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("role") && msg.contains("coreValues"),
            "expected an error naming the field and coreValues, got: {msg}"
        );
    }

    #[test]
    fn edge_enum_with_empty_core_values_is_rejected() {
        let mut role = edge_field("role", "enum");
        role.core_values = Some(vec![]);
        let rels = vec![rel_with_edge_fields(vec![role])];
        assert!(validate_edge_field_declarations(&rels).is_err());
    }

    #[test]
    fn core_values_on_a_non_enum_edge_field_is_rejected() {
        // A value set on a field nothing validates against is a declaration
        // whose author believed it would be enforced.
        let mut role = edge_field("role", "string");
        role.core_values = Some(rbac_values());
        let rels = vec![rel_with_edge_fields(vec![role])];
        let err = validate_edge_field_declarations(&rels).unwrap_err();
        assert!(
            err.to_string().contains("only meaningful on an enum"),
            "got: {err}"
        );
    }

    #[test]
    fn edge_enum_default_outside_the_declared_set_is_rejected() {
        let mut role = edge_field("role", "enum");
        role.core_values = Some(rbac_values());
        role.default = Some(serde_json::json!("admin"));

        let rels = vec![rel_with_edge_fields(vec![role])];
        let err = validate_edge_field_declarations(&rels).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("admin") && msg.contains("owner"),
            "error should name the bad default and the legal values, got: {msg}"
        );
    }

    #[test]
    fn edge_enum_non_string_default_is_rejected() {
        let mut role = edge_field("role", "enum");
        role.core_values = Some(rbac_values());
        role.default = Some(serde_json::json!(3));

        let rels = vec![rel_with_edge_fields(vec![role])];
        assert!(validate_edge_field_declarations(&rels).is_err());
    }

    #[test]
    fn edge_enum_duplicate_values_are_rejected() {
        let mut role = edge_field("role", "enum");
        let mut values = rbac_values();
        values.push(EnumValue {
            value: "owner".to_string(),
            label: "Owner (duplicate)".to_string(),
        });
        role.core_values = Some(values);

        let rels = vec![rel_with_edge_fields(vec![role])];
        let err = validate_edge_field_declarations(&rels).unwrap_err();
        assert!(err.to_string().contains("more than once"), "got: {err}");
    }

    #[test]
    fn non_enum_edge_fields_and_relationships_without_edge_fields_are_untouched() {
        let plain = vec![
            rel_with_edge_fields(vec![
                edge_field("billing_date", "date"),
                edge_field("payment_terms", "string"),
            ]),
            SchemaRelationship {
                edge_fields: None,
                ..rel_with_edge_fields(vec![])
            },
        ];
        assert!(validate_edge_field_declarations(&plain).is_ok());
    }
}
