//! Schema Management Types
//!
//! This module contains data structures for managing user-defined entity schemas
//! in NodeSpace. Schemas are stored as regular nodes with `node_type = 'schema'`.
//! Field definitions live in the schema node's `properties.fields` JSON;
//! relationship declarations are NOT stored in properties — each one is a row in
//! the `relationship` table between the declaring and target schema nodes.
//!
//! ## Schema Protection Levels
//!
//! - `Core`: Cannot be modified or deleted (UI components depend on these fields)
//! - `User`: Fully modifiable/deletable by users
//! - `System`: Auto-managed internal fields, read-only
//!
//! ## Relationships
//!
//! Schemas can define typed relationships to other node types. A declaration is
//! stored as a `relationship` table row: `in_node` = the declaring schema node,
//! `out_node` = the target schema node (or the declaring schema itself when
//! `target_type` is `None` — an untyped relationship accepting any target),
//! `relationship_type` = the declared name, and the full [`SchemaRelationship`]
//! serialized into the row's `properties` JSON. Instance-level edges reuse the
//! same table with the same `relationship_type`; the two are distinguished by
//! their endpoints (declaration edges connect schema nodes, instance edges
//! connect instance nodes).
//!
//! Reads are consolidated in `SqliteStore::get_schema_declarations` /
//! `get_all_schema_declarations`, which hydrate `SchemaNode.relationships`;
//! writes go through `SqliteStore::set_schema_declarations`.
//!
//! - **Bidirectional querying**: both directions query the same edge rows
//! - **Edge fields**: custom properties on the relationship itself
//! - **Cardinality**: "one" or "many" constraints (enforced at application level)
//!
//! ## Source of truth
//!
//! These field/relationship primitives are the wire shapes shared with the Tauri
//! command layer, so they live in `nodespace-types` and are re-exported here.
//! There is exactly one definition per type — adding a field in `nodespace-types`
//! surfaces here automatically, and the round-trip tests in that crate fail if a
//! field is dropped at the conversion boundary.

pub use nodespace_types::{
    derive_friendly_name, EdgeField, EnumValue, RelationshipCardinality, RelationshipDirection,
    SchemaField, SchemaProtectionLevel, SchemaRelationship,
};

/// Built-in structural relationship types, paired with the name each reads by
/// from the target's end. These are not schema-declared: they have hardcoded
/// semantics (hierarchy, mentions, collection membership, roles) and their own
/// UI affordances.
///
/// Because schema relationship declarations and these primitives share the one
/// `relationship` table's `relationship_type` column, a declared relationship
/// must never take one of these names — schema creation/update rejects them.
///
/// A schema-declared relationship carries its own reverse name in
/// [`SchemaRelationship::reverse_name`], which is required rather than derived
/// because naming an inverse is a modeling decision only the author can make.
/// The built-ins predate that convention and have no `SchemaRelationship`
/// behind them, so their inverses are fixed here instead — the one place they
/// exist as data.
///
/// These are user-visible keys: once a resolver can traverse by reverse name,
/// they appear in query filters and Play conditions, so renaming one is a
/// breaking change to every call site.
pub const BUILTIN_RELATIONSHIPS: [(&str, &str); 4] = [
    ("member_of", "has_member"),
    ("has_child", "child_of"),
    ("mentions", "mentioned_by"),
    ("has_role", "role_of"),
];

/// Built-in structural relationship type names (the declaring side).
///
/// Spelled out per index because const context has no iteration. The assertion
/// below makes a length mismatch a compile error — without it, adding a fifth
/// entry to [`BUILTIN_RELATIONSHIPS`] would silently leave this array short by
/// one, and the missing name would stop being treated as reserved.
pub const BUILTIN_RELATIONSHIP_NAMES: [&str; 4] = [
    BUILTIN_RELATIONSHIPS[0].0,
    BUILTIN_RELATIONSHIPS[1].0,
    BUILTIN_RELATIONSHIPS[2].0,
    BUILTIN_RELATIONSHIPS[3].0,
];

const _: () = assert!(
    BUILTIN_RELATIONSHIPS.len() == BUILTIN_RELATIONSHIP_NAMES.len(),
    "BUILTIN_RELATIONSHIP_NAMES must list every BUILTIN_RELATIONSHIPS entry"
);

/// Whether `name` is one of the built-in structural relationship types.
pub fn is_builtin_relationship(name: &str) -> bool {
    BUILTIN_RELATIONSHIP_NAMES.contains(&name)
}

/// The reverse name for a built-in structural relationship — the label its edge
/// reads by from the target's end (`has_child` → `child_of`).
///
/// `None` for anything else: a schema-declared relationship's reverse name comes
/// from its own [`SchemaRelationship::reverse_name`], never from here.
pub fn builtin_reverse_name(name: &str) -> Option<&'static str> {
    BUILTIN_RELATIONSHIPS
        .iter()
        .find(|(forward, _)| *forward == name)
        .map(|(_, reverse)| *reverse)
}
