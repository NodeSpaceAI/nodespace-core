//! `extends` chain resolution — ancestry, effective fields, and cycle detection.
//!
//! Per ADR-078, a schema may declare exactly one parent via `extends`,
//! composing its effective field set as its own fields plus every ancestor's
//! (additive only — no override, no narrowing). An instance created under an
//! extending schema carries *that* schema's id as its real `node_type`.
//!
//! Three things live here, all built on one primitive — the ancestor chain:
//!
//! - [`resolve_ancestor_chain`] walks `extends` edges from a schema toward its
//!   root, nearest ancestor first. This is the ordering every other consumer
//!   depends on: it is the property-bucket read order (nearer scope wins a key
//!   collision) and the `maps_to` resolution order.
//! - [`resolve_effective_fields`] flattens that chain's field definitions into
//!   one set, and is what validation, defaulting and schema comprehension read
//!   instead of a schema's own directly-declared `fields`.
//! - [`detect_cycle`] rejects `A extends B extends A` before a write lands.
//!   No existing relationship in NodeSpace requires acyclicity, so this is new
//!   logic rather than a reuse of `validate_relationship_targets_exist`'s flat
//!   existence check.
//!
//! **Edge direction is load-bearing.** `set_schema_declarations` stores a
//! declaration as `in_node` = the declaring (child) schema, `out_node` = the
//! target (parent). So walking ancestors follows `in_node → out_node`, and
//! walking descendants follows `out_node → in_node`. Inverting either silently
//! inverts the closure, which is why both directions are resolved here rather
//! than reconstructed per call site.

use crate::models::schema::EXTENDS_RELATIONSHIP;
use crate::models::{SchemaField, SchemaNode};

/// Maximum `extends` chain depth.
///
/// Mirrors the depth cap the recursive relationship queries already use
/// (`MENTION_CONTAINERS_QUERY`, the collection-subtree query). Cycle detection
/// below makes an unbounded walk impossible in practice; this is the
/// belt-and-braces guard for a chain that is merely absurd rather than cyclic,
/// and it keeps a corrupted edge set from hanging a write path.
pub const MAX_EXTENDS_DEPTH: usize = 100;

/// How a schema's parent is read during a chain walk.
///
/// Resolution needs to run in three different contexts — against committed
/// state, against a pending write not yet persisted, and against fixtures in
/// tests — so the walk is written against this trait rather than against the
/// store directly. Implementors answer one question: which schema, if any,
/// does `schema_id` extend?
pub trait ParentLookup {
    /// The parent of `schema_id`, or `None` if it declares no `extends`.
    fn parent_of(&self, schema_id: &str) -> Option<String>;
}

impl<F> ParentLookup for F
where
    F: Fn(&str) -> Option<String>,
{
    fn parent_of(&self, schema_id: &str) -> Option<String> {
        self(schema_id)
    }
}

/// An `extends` cycle, named by the path that closes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendsCycle {
    /// The schemas forming the cycle, in walk order, ending with the repeat.
    pub path: Vec<String>,
}

impl std::fmt::Display for ExtendsCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.join(" extends "))
    }
}

/// Walk `extends` edges from `schema_id` toward the root.
///
/// Returns the chain **including `schema_id` itself as the first element**,
/// nearest ancestor first — `["issue", "task"]` for an issue extending task.
/// That ordering is the contract: property buckets are read in this order so a
/// nearer scope wins any key collision, and `maps_to` resolves along it.
///
/// A cycle terminates the walk at the repeat rather than looping; callers that
/// must reject a cycle call [`detect_cycle`] first. Reaching
/// [`MAX_EXTENDS_DEPTH`] likewise truncates rather than erroring, so a
/// corrupted edge set degrades to a short chain instead of hanging a write.
pub fn resolve_ancestor_chain(schema_id: &str, lookup: &impl ParentLookup) -> Vec<String> {
    let mut chain = vec![schema_id.to_string()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(schema_id.to_string());

    let mut current = schema_id.to_string();
    while chain.len() < MAX_EXTENDS_DEPTH {
        let Some(parent) = lookup.parent_of(&current) else {
            break;
        };
        if !seen.insert(parent.clone()) {
            break;
        }
        chain.push(parent.clone());
        current = parent;
    }

    chain
}

/// Detect a cycle reachable from `schema_id`, treating `pending_parent` as its
/// parent instead of whatever `lookup` reports.
///
/// Both `create_schema` and `update_schema` call this *before* writing, so the
/// edge under consideration does not exist yet (creation) or is about to be
/// replaced (re-targeting). Passing it explicitly is what lets one function
/// serve both, and is why this does not simply walk committed state.
///
/// A self-extend (`A extends A`) is a cycle of length one and is reported the
/// same way.
pub fn detect_cycle(
    schema_id: &str,
    pending_parent: &str,
    lookup: &impl ParentLookup,
) -> Option<ExtendsCycle> {
    let mut path = vec![schema_id.to_string()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(schema_id.to_string());

    let mut current = pending_parent.to_string();
    loop {
        path.push(current.clone());

        if !seen.insert(current.clone()) {
            return Some(ExtendsCycle { path });
        }
        if path.len() > MAX_EXTENDS_DEPTH {
            // Depth exhaustion without a repeat means the edge set is
            // corrupt rather than cyclic. Report it as a cycle anyway: the
            // caller's only useful response is the same refusal to write.
            return Some(ExtendsCycle { path });
        }

        match lookup.parent_of(&current) {
            Some(next) => current = next,
            None => return None,
        }
    }
}

/// Flatten a resolved chain's field definitions into one effective set.
///
/// `chain_fields` is the per-schema field list in [`resolve_ancestor_chain`]
/// order (own schema first, then each ancestor). A field declared by a nearer
/// scope shadows a same-named field from a further one.
///
/// **Shadowing here is a backstop, not a feature.** Redeclaration is rejected
/// at write time (see `validate_no_field_redeclaration`), so a well-formed
/// chain never produces a collision. Preferring the nearer declaration keeps
/// resolution total if a collision arises anyway — through a retroactive
/// ancestor field addition, the unresolved edge case ADR-078 names — rather
/// than dropping a field or returning both.
pub fn flatten_chain_fields(chain_fields: &[Vec<SchemaField>]) -> Vec<SchemaField> {
    let mut out: Vec<SchemaField> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for fields in chain_fields {
        for field in fields {
            if seen.insert(field.name.clone()) {
                out.push(field.clone());
            }
        }
    }

    out
}

/// Resolve one stored enum value to what it means at `target_scope` (ADR-078).
///
/// Walks `maps_to` from the value as stored toward the target scope: an
/// `issue` node storing `status: "backlog"` reads as `todo` for a consumer
/// operating at `task`'s scope, because `backlog` was added to the inherited
/// field carrying `maps_to: "todo"`.
///
/// `scope_fields` is the effective field set at the *target* scope — what that
/// consumer can see. A value already present there needs no resolution: it
/// means the same thing at both scopes. A value absent from it is resolved one
/// `maps_to` hop at a time until it lands on one that is present.
///
/// Returns `None` when the value cannot be resolved into the target scope,
/// which a caller should treat as "does not match" rather than substituting
/// the raw value — surfacing an unresolvable value to a base-scoped consumer
/// is exactly the hazard `maps_to` exists to prevent.
///
/// `node_fields` is the effective set at the node's own scope, where the
/// `maps_to` declarations live.
pub fn resolve_value_at_scope(
    field_name: &str,
    stored_value: &str,
    node_fields: &[SchemaField],
    scope_fields: &[SchemaField],
) -> Option<String> {
    // Visible at the target scope already — nothing to resolve.
    if field_has_value(scope_fields, field_name, stored_value) {
        return Some(stored_value.to_string());
    }

    let field = node_fields.iter().find(|f| f.name == field_name)?;

    let mut current = stored_value.to_string();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(current.clone());

    // Bounded by the chain depth cap: each hop must reach a value declared
    // further up, and a malformed `maps_to` cycle terminates on the visited
    // set rather than looping.
    for _ in 0..MAX_EXTENDS_DEPTH {
        let next = field
            .core_values
            .iter()
            .flatten()
            .chain(field.user_values.iter().flatten())
            .find(|ev| ev.value == current)
            .and_then(|ev| ev.maps_to.clone())?;

        if !seen.insert(next.clone()) {
            return None;
        }
        if field_has_value(scope_fields, field_name, &next) {
            return Some(next);
        }
        current = next;
    }

    None
}

/// Whether a field in this set declares `value` at any protection level.
fn field_has_value(fields: &[SchemaField], field_name: &str, value: &str) -> bool {
    fields
        .iter()
        .find(|f| f.name == field_name)
        .map(|f| {
            f.core_values
                .iter()
                .flatten()
                .chain(f.user_values.iter().flatten())
                .any(|ev| ev.value == value)
        })
        .unwrap_or(false)
}

/// The `extends` target declared by a schema, if any.
///
/// Reads the hydrated `relationships` list rather than properties: on the core
/// path a schema's declarations live in the relationship table and are
/// hydrated on read, so a `SchemaNode` built by a bare `from_node` has an
/// empty list and will report `None` here regardless of what is stored.
pub fn declared_parent(schema: &SchemaNode) -> Option<String> {
    schema
        .relationships
        .iter()
        .find(|rel| rel.name == EXTENDS_RELATIONSHIP)
        .and_then(|rel| rel.target_type.clone())
}

/// Build the `SchemaRelationship` an `extends` key persists as.
///
/// Centralised so the shape cannot drift between create and update: every
/// field but the target is fixed, and the cardinality is what enforces
/// single-parent extension at the storage layer.
pub fn extends_declaration(parent_schema_id: &str) -> crate::models::schema::SchemaRelationship {
    use crate::models::schema::{
        RelationshipCardinality, RelationshipDirection, SchemaRelationship,
        EXTENDED_BY_RELATIONSHIP,
    };

    SchemaRelationship {
        name: EXTENDS_RELATIONSHIP.to_string(),
        target_type: Some(parent_schema_id.to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::One,
        required: None,
        reverse_name: EXTENDED_BY_RELATIONSHIP.to_string(),
        reverse_cardinality: RelationshipCardinality::Many,
        edge_fields: None,
        description: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::EnumValue;
    use std::collections::HashMap;

    /// Fixture parent lookup: a plain child → parent map.
    fn lookup(pairs: &[(&str, &str)]) -> impl ParentLookup {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(c, p)| (c.to_string(), p.to_string()))
            .collect();
        move |id: &str| map.get(id).cloned()
    }

    fn field(name: &str) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            field_type: "string".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn chain_is_self_first_then_ancestors() {
        let l = lookup(&[("bug", "issue"), ("issue", "task")]);
        assert_eq!(
            resolve_ancestor_chain("bug", &l),
            vec!["bug", "issue", "task"]
        );
    }

    #[test]
    fn chain_of_unextended_schema_is_just_itself() {
        let l = lookup(&[]);
        assert_eq!(resolve_ancestor_chain("task", &l), vec!["task"]);
    }

    #[test]
    fn chain_terminates_on_a_cycle_rather_than_looping() {
        let l = lookup(&[("a", "b"), ("b", "a")]);
        // Terminates at the repeat; `detect_cycle` is what rejects this.
        assert_eq!(resolve_ancestor_chain("a", &l), vec!["a", "b"]);
    }

    #[test]
    fn chain_is_capped_at_max_depth() {
        let pairs: Vec<(String, String)> = (0..500)
            .map(|i| (format!("s{}", i), format!("s{}", i + 1)))
            .collect();
        let map: HashMap<String, String> = pairs.into_iter().collect();
        let l = move |id: &str| map.get(id).cloned();
        assert_eq!(resolve_ancestor_chain("s0", &l).len(), MAX_EXTENDS_DEPTH);
    }

    #[test]
    fn detects_a_direct_cycle() {
        // issue already extends task; pointing task at issue closes the loop.
        let l = lookup(&[("issue", "task")]);
        let cycle = detect_cycle("task", "issue", &l).expect("cycle should be detected");
        assert_eq!(cycle.path, vec!["task", "issue", "task"]);
    }

    #[test]
    fn detects_a_transitive_cycle_through_two_intermediates() {
        let l = lookup(&[("b", "c"), ("c", "d"), ("d", "a")]);
        let cycle = detect_cycle("a", "b", &l).expect("cycle should be detected");
        assert_eq!(cycle.path, vec!["a", "b", "c", "d", "a"]);
    }

    #[test]
    fn detects_a_self_extend() {
        let l = lookup(&[]);
        let cycle = detect_cycle("a", "a", &l).expect("self-extend is a cycle");
        assert_eq!(cycle.path, vec!["a", "a"]);
    }

    #[test]
    fn accepts_an_acyclic_chain() {
        let l = lookup(&[("task", "thing")]);
        assert_eq!(detect_cycle("issue", "task", &l), None);
    }

    #[test]
    fn flatten_keeps_chain_order_and_dedupes_to_nearest() {
        let own = vec![field("severity")];
        let parent = vec![field("status"), field("severity")];
        let flat = flatten_chain_fields(&[own, parent]);

        let names: Vec<&str> = flat.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["severity", "status"]);
    }

    #[test]
    fn flatten_of_an_unextended_schema_is_its_own_fields() {
        let own = vec![field("status"), field("priority")];
        let flat = flatten_chain_fields(std::slice::from_ref(&own));
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn extends_declaration_has_the_fixed_shape() {
        use crate::models::schema::{RelationshipCardinality, RelationshipDirection};

        let rel = extends_declaration("task");
        assert_eq!(rel.name, "extends");
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.reverse_name, "extended_by");
        assert!(matches!(rel.direction, RelationshipDirection::Out));
        assert!(matches!(rel.cardinality, RelationshipCardinality::One));
        assert!(matches!(
            rel.reverse_cardinality,
            RelationshipCardinality::Many
        ));
    }

    // ------------------------------------------------------------------
    // maps_to / scope-relative value resolution (ADR-078)
    // ------------------------------------------------------------------

    fn enum_field(name: &str, core: &[&str], user: &[(&str, &str)]) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            field_type: "enum".to_string(),
            extensible: Some(true),
            core_values: Some(core.iter().map(|v| EnumValue::new(*v, *v)).collect()),
            user_values: Some(
                user.iter()
                    .map(|(v, m)| EnumValue {
                        value: v.to_string(),
                        label: v.to_string(),
                        maps_to: Some(m.to_string()),
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn a_value_the_target_scope_already_has_needs_no_resolution() {
        let base = enum_field("status", &["todo", "done"], &[]);
        let child = enum_field("status", &["todo", "done"], &[("backlog", "todo")]);

        assert_eq!(
            resolve_value_at_scope("status", "todo", &[child], &[base]),
            Some("todo".to_string()),
            "a value meaning the same at both scopes passes through unchanged"
        );
    }

    #[test]
    fn an_extended_value_resolves_to_its_maps_to_target() {
        let base = enum_field("status", &["todo", "done"], &[]);
        let child = enum_field("status", &["todo", "done"], &[("backlog", "todo")]);

        // The whole point: a task-scoped consumer reading an issue's stored
        // `backlog` sees `todo`, a value it was written to understand.
        assert_eq!(
            resolve_value_at_scope("status", "backlog", &[child], &[base]),
            Some("todo".to_string())
        );
    }

    #[test]
    fn resolution_follows_multiple_hops_up_a_chain() {
        let root = enum_field("status", &["todo"], &[]);
        // mid adds `queued` -> todo; leaf adds `triage` -> queued.
        let leaf = enum_field(
            "status",
            &["todo"],
            &[("queued", "todo"), ("triage", "queued")],
        );

        assert_eq!(
            resolve_value_at_scope("status", "triage", &[leaf], &[root]),
            Some("todo".to_string()),
            "resolution walks until it lands on a value the target scope has"
        );
    }

    #[test]
    fn an_unresolvable_value_returns_none_rather_than_the_raw_value() {
        let base = enum_field("status", &["todo", "done"], &[]);
        // `orphan` carries no maps_to — only reachable if validation was
        // bypassed, but resolution must still refuse to leak it.
        let child = SchemaField {
            user_values: Some(vec![EnumValue::new("orphan", "Orphan")]),
            ..enum_field("status", &["todo", "done"], &[])
        };

        assert_eq!(
            resolve_value_at_scope("status", "orphan", &[child], &[base]),
            None,
            "surfacing an unresolvable value to a base-scoped consumer is the \
             hazard maps_to exists to prevent"
        );
    }

    #[test]
    fn a_maps_to_cycle_terminates_instead_of_looping() {
        let base = enum_field("status", &["todo"], &[]);
        let child = enum_field("status", &[], &[("a", "b"), ("b", "a")]);

        assert_eq!(
            resolve_value_at_scope("status", "a", &[child], &[base]),
            None,
            "a malformed maps_to cycle must terminate on the visited set"
        );
    }

    #[test]
    fn resolution_of_an_unknown_field_is_none() {
        let base = enum_field("status", &["todo"], &[]);
        let child = enum_field("status", &["todo"], &[]);

        assert_eq!(
            resolve_value_at_scope("severity", "high", &[child], &[base]),
            None
        );
    }
}
