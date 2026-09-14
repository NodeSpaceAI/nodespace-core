//! Domain Events for SqliteStore
//!
//! This module defines the domain events emitted by SqliteStore when data changes.
//! These events follow the observer pattern, allowing other parts of the system
//! (like the Tauri layer) to subscribe to data changes without coupling to the
//! database layer implementation.
//!
//! # Architecture
//!
//! Events are emitted using tokio's broadcast channel, allowing multiple subscribers
//! to receive notifications asynchronously.
//!
//! # Event Flow
//!
//! 1. SqliteStore performs a data operation (create, update, delete)
//! 2. Domain event is emitted via broadcast channel
//! 3. All subscribers receive the event asynchronously
//! 4. LiveQueryService (Tauri layer) listens to events and forwards to frontend
//!
//! # Unified Relationship Event System
//!
//! All relationships (`has_child`, `member_of`, `mentions`, and custom types)
//! use a generic `RelationshipEvent` struct with `relationship_type` for discrimination.
//! This allows adding new relationship types without modifying the event system.

use crate::models::Node;
use serde::{Deserialize, Serialize};

/// Unified relationship event for all relationship types
///
/// This generic structure supports all relationship types: `has_child`, `member_of`, `mentions`,
/// and any future custom relationship types. It replaces the enum-based approach
/// that required modifying the event system for each new relationship type.
///
/// # Relationship Types
///
/// - `"has_child"` - Hierarchical parent-child relationship with `order` property
/// - `"member_of"` - Collection membership (node belongs to collection)
/// - `"mentions"` - Bidirectional reference between nodes
/// - Custom types - Any string representing a user-defined relationship
///
/// # Properties
///
/// Type-specific data stored in `properties`:
/// - `has_child`: `{"order": 1.5}`
/// - `mentions`: `{"context": "optional context"}`
/// - `member_of`: `{}` (no additional properties)
/// - Custom: User-defined JSON properties
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipEvent {
    /// Unique relationship ID (string convention, e.g., "relationship:abc123")
    pub id: String,
    /// Source node ID (the "in" node in the relationship graph edge)
    pub from_id: String,
    /// Target node ID (the "out" node in the relationship graph edge)
    pub to_id: String,
    /// Relationship type: "has_child", "mentions", "member_of", or custom types
    pub relationship_type: String,
    /// Type-specific properties (order for hierarchy, context for mentions, etc.)
    pub properties: serde_json::Value,
}

impl RelationshipEvent {
    /// Construct an event with `from_id` / `to_id` normalized to the
    /// full prefixed id form (`node:<key>`). Required by the
    /// serialization-contract test below — every consumer that
    /// parses these fields splits on `:` and rejects bare ids.
    /// Callers in `NodeService` often hold bare ids (date-page
    /// nodes use `"2026-05-20"`, regular nodes use bare UUIDs), so
    /// this constructor is the single normalization point producers
    /// should go through.
    pub fn new(
        id: String,
        from_id: &str,
        to_id: &str,
        relationship_type: impl Into<String>,
        properties: serde_json::Value,
    ) -> Self {
        Self {
            id,
            from_id: node_thing(from_id),
            to_id: node_thing(to_id),
            relationship_type: relationship_type.into(),
            properties,
        }
    }
}

/// Normalize a node id to its full prefixed id form
/// (`node:<key>`). Pass-through if the input already contains `:`.
/// `pub(crate)` so the `RelationshipDeleted` inline-field variant
/// can use the same normalization at its emit sites in
/// `services::node_service` without duplicating the helper.
pub(crate) fn node_thing(id: &str) -> String {
    if id.contains(':') {
        id.to_string()
    } else {
        format!("node:{id}")
    }
}

/// Describes a single property change for play trigger matching
///
/// Computed by diffing pre-mutation and post-mutation node properties.
/// Used by the playbook engine for fine-grained `property_changed` triggers.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyChange {
    /// Property key that changed (namespaced, e.g., "task.status")
    pub key: String,
    /// Previous value (None if property was added)
    pub old_value: Option<serde_json::Value>,
    /// New value (None if property was removed)
    pub new_value: Option<serde_json::Value>,
}

/// Playbook execution context carried on events for cycle detection
///
/// When the playbook engine executes actions that mutate the graph, the resulting
/// events carry this context so the engine can track chain depth and attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybookExecutionContext {
    /// UUID of the root user event that started this chain
    pub originating_event_id: String,
    /// Current chain depth (max 10)
    pub depth: u8,
    /// Playbook that produced this mutation
    pub source_playbook_id: String,
}

/// Reserved node-property key carrying the causal chain depth persisted onto
/// a node produced by a play action (ADR-060 §5).
///
/// `PlaybookExecutionContext::depth` above is the in-process analogue of this
/// value: it lives only in the `EventEnvelope` that accompanies a mutation,
/// for the lifetime of the local broadcast channel. Sync transports a node's
/// committed `properties`, not that transient envelope, so a causal chain
/// that hops from one device to another needs its depth carried on the node
/// itself to survive the hop — this is that carrier.
///
/// Follows the `_`-prefixed internal-bookkeeping convention already used for
/// `_seed` and `_schemaVersion`: `NodeService::normalize_flat_properties_to_namespace`
/// keeps any `_`-prefixed key at the top level of `properties`, independent
/// of `node_type`, rather than nesting it under the node's own type
/// namespace — required here since a play action can create or update a
/// node of any type. The same `_` prefix keeps it out of CEL condition
/// bindings: `playbook::cel::node_to_cel_value` filters `_`-prefixed keys in
/// both the type-namespace branch and the top-level branch it takes (the
/// latter is where this property actually lands, since it is never nested
/// under a type namespace). It is internal engine bookkeeping, not a user-
/// or schema-visible property — nothing about the `_` prefix stops an
/// ordinary `create_node`/`update_node` call from writing or clobbering it,
/// though (see `persisted_chain_depth`'s doc).
pub const PLAYBOOK_CHAIN_DEPTH_PROPERTY: &str = "_playbookChainDepth";

/// Read the causal chain depth persisted on a node's raw `properties`, if any,
/// bounded to `0..=max_depth`.
///
/// This property lives in a node's ordinary `properties`, which nothing in
/// `NodeService` restricts a non-engine caller from writing or corrupting —
/// it is untrusted input from the engine's point of view, not a value this
/// process necessarily produced itself (a node synced in from another
/// device, or written directly by any `create_node`/`update_node` caller,
/// carries whatever value was put there). Returns `None` — treated by
/// callers the same as a node never touched by a play action, falling back
/// to depth 0 — when the property is absent, the stored value doesn't fit a
/// `u8`, or it falls outside `0..=max_depth`. Callers pass `MAX_CHAIN_DEPTH`
/// as `max_depth`; nothing this engine ever writes (see `stamp_chain_depth`
/// in `playbook::actions`) produces a value above it, so anything larger is
/// corrupt or externally-tampered data, not a legitimately deep chain — it
/// is rejected outright rather than clamped, so it can't be silently
/// coerced into a valid-looking depth. This is a filter, not a guarantee:
/// a value written or deleted to fall *within* the valid range is
/// indistinguishable from a genuine chain at that depth. The engine's
/// arithmetic on the returned value must still not assume it is in range on
/// its own — see `playbook::engine::exceeds_max_chain_depth`'s use of
/// saturating arithmetic for the defense-in-depth half of this.
pub fn persisted_chain_depth(properties: &serde_json::Value, max_depth: u8) -> Option<u8> {
    let depth = properties
        .get(PLAYBOOK_CHAIN_DEPTH_PROPERTY)
        .and_then(|v| v.as_u64())
        .and_then(|depth| u8::try_from(depth).ok())?;
    (depth <= max_depth).then_some(depth)
}

/// Reserved `source_client_id` for writes applied by the local-first sync
/// service (ADR-027's origin-tagging convention: `NodeService::with_client
/// ("sync-service")`).
///
/// Consumers that must not treat a sync-applied write as if it were a fresh
/// local mutation — the playbook engine's local-origin gate (ADR-073) is the
/// first such consumer — compare `EventMetadata::source_client_id` against
/// this constant rather than hardcoding the literal string.
pub const SYNC_SERVICE_CLIENT_ID: &str = "sync-service";

/// Metadata for cross-cutting concerns on domain events
///
/// Wraps `DomainEvent` in an envelope so metadata like `source_client_id` lives
/// in one place instead of being duplicated across every event variant.
#[derive(Debug, Clone, PartialEq)]
pub struct EventMetadata {
    /// Client that originated the mutation (e.g., "tauri-main", "playbook-engine")
    pub source_client_id: Option<String>,
    /// Playbook execution context (None for user/MCP mutations)
    pub playbook_context: Option<PlaybookExecutionContext>,
}

/// Envelope wrapping DomainEvent with metadata
///
/// Carried on the broadcast channel. All subscribers receive envelopes.
/// `source_client_id` has been moved from individual event variants into
/// `metadata` to eliminate duplication and support future metadata fields.
#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
    /// The domain event payload
    pub event: DomainEvent,
    /// Cross-cutting metadata (source client, playbook context, etc.)
    pub metadata: EventMetadata,
}

/// Domain events emitted by SqliteStore
///
/// These events are emitted whenever data changes in the database.
/// They represent domain-level changes, not database operations.
///
/// Source client identification is carried in `EventMetadata` (on the
/// `EventEnvelope` wrapper), not on individual variants.
///
#[derive(Debug, Clone, PartialEq)]
pub enum DomainEvent {
    /// A new node was created
    NodeCreated { node_id: String, node_type: String },

    /// An existing node was updated — carries the full committed node so
    /// subscribers (WatchNodes, LocalAgentService) don't need a re-fetch.
    NodeUpdated {
        node_id: String,
        node_type: String,
        /// The committed node state after the update.
        node: Node,
        /// Properties that changed (empty if pre-mutation state unavailable)
        changed_properties: Vec<PropertyChange>,
    },

    /// A node was deleted
    NodeDeleted {
        id: String,
        /// Node type (e.g., "schema", "collection") - included so consumers can
        /// apply structural bypass logic without fetching the already-deleted node
        node_type: String,
    },

    // ============================================================================
    // Unified Relationship Events
    // All relationship types (has_child, member_of, mentions, custom) use these
    // generic events. No backward compatibility - old EdgeCreated/etc removed.
    // ============================================================================
    /// A new relationship was created (unified format for all relationship types)
    ///
    /// Supports: `has_child`, `member_of`, `mentions`, and custom relationship types.
    RelationshipCreated { relationship: RelationshipEvent },

    /// An existing relationship was updated (unified format for all relationship types)
    ///
    /// Typically used for reordering (updating `order` property on `has_child` relationships).
    RelationshipUpdated { relationship: RelationshipEvent },

    /// A relationship was deleted (unified format for all relationship types)
    ///
    /// Contains relationship ID and node IDs for handlers that need them
    /// (e.g., hierarchy operations need from_id/to_id to update the structure tree).
    RelationshipDeleted {
        /// The relationship ID (string convention, e.g., "relationship:abc123")
        id: String,
        /// Source node ID (the "from" node in the relationship)
        from_id: String,
        /// Target node ID (the "to" node in the relationship)
        to_id: String,
        /// Relationship type hint for handlers that need it
        relationship_type: String,
    },

    /// A fire-and-forget background import (currently: the async branch of
    /// `handle_create_nodes_from_markdown`) failed after the caller had
    /// already been told the operation succeeded.
    ///
    /// ADR-069 §5/F16: the bulk insert itself is a single atomic
    /// transaction (`bulk_create_hierarchy_root_notify`) — this event does
    /// not change that. What it fixes is that a failure there previously
    /// surfaced only via `tracing::error!`, invisible to anything but log
    /// inspection. `root_id` names the (empty) root container the caller
    /// already received an id for, so a subscriber can at least flag that
    /// specific node as failed-to-populate rather than the user discovering
    /// an empty container with no explanation.
    BackgroundImportFailed {
        /// The root node id the caller was already given before the
        /// background task started (and that the caller believes has content).
        root_id: String,
        /// Human-readable failure detail, already formatted for display —
        /// not a machine-parsed error code.
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks in the `node_thing` normalizer contract: bare ids (date
    /// pages, raw UUIDs) get prefixed; anything already containing
    /// `:` passes through unchanged. The producer-side
    /// `RelationshipEvent::new` and the
    /// `RelationshipDeleted`-emit-sites both rely on this shape.
    #[test]
    fn node_thing_normalizes_bare_ids_and_passes_through_prefixed() {
        assert_eq!(node_thing("2026-05-20"), "node:2026-05-20");
        assert_eq!(node_thing("some-uuid"), "node:some-uuid");
        assert_eq!(node_thing("node:already-prefixed"), "node:already-prefixed");
        // Edge case: any `:` makes it pass-through, even foreign
        // tables. Producers are expected to pass ids in the
        // `<table>:<key>` form when crossing table boundaries.
        assert_eq!(node_thing("relationship:abc"), "relationship:abc");
    }

    /// `RelationshipEvent::new` normalizes both endpoint ids through
    /// `node_thing`. Locks the contract so producers can pass bare
    /// ids and rely on the constructor for the prefix.
    #[test]
    fn relationship_event_new_normalizes_both_endpoints() {
        let rel = RelationshipEvent::new(
            "relationship:abc:def".to_string(),
            "abc",
            "def",
            "has_child",
            serde_json::json!({"order": 1.0}),
        );
        assert_eq!(rel.from_id, "node:abc");
        assert_eq!(rel.to_id, "node:def");
    }

    /// Contract test: Documents and enforces the exact JSON format for RelationshipEvent
    ///
    /// IMPORTANT: The frontend TypeScript types MUST match this format.
    /// This is the unified format that supports all relationship types.
    #[test]
    fn test_relationship_event_serialization_contract() {
        // Test has_child relationship (hierarchy)
        let has_child = RelationshipEvent {
            id: "relationship:abc123".to_string(),
            from_id: "node:parent-123".to_string(),
            to_id: "node:child-456".to_string(),
            relationship_type: "has_child".to_string(),
            properties: serde_json::json!({"order": 1.5}),
        };

        let json = serde_json::to_string(&has_child).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify camelCase field names
        assert_eq!(parsed.get("id").unwrap(), "relationship:abc123");
        assert_eq!(parsed.get("fromId").unwrap(), "node:parent-123");
        assert_eq!(parsed.get("toId").unwrap(), "node:child-456");
        assert_eq!(parsed.get("relationshipType").unwrap(), "has_child");
        assert_eq!(parsed.get("properties").unwrap().get("order").unwrap(), 1.5);

        // Test member_of relationship (collection membership)
        let member_of = RelationshipEvent {
            id: "relationship:xyz789".to_string(),
            from_id: "node:item-001".to_string(),
            to_id: "node:collection-002".to_string(),
            relationship_type: "member_of".to_string(),
            properties: serde_json::json!({}),
        };

        let json = serde_json::to_string(&member_of).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.get("relationshipType").unwrap(), "member_of");
        assert!(parsed
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty());

        // Test mentions relationship
        let mentions = RelationshipEvent {
            id: "relationship:mention-456".to_string(),
            from_id: "node:source-123".to_string(),
            to_id: "node:target-456".to_string(),
            relationship_type: "mentions".to_string(),
            properties: serde_json::json!({"context": "see also"}),
        };

        let json = serde_json::to_string(&mentions).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.get("relationshipType").unwrap(), "mentions");
        assert_eq!(
            parsed.get("properties").unwrap().get("context").unwrap(),
            "see also"
        );
    }

    /// Test RelationshipEvent round-trip deserialization
    #[test]
    fn test_relationship_event_deserialization() {
        let original = RelationshipEvent {
            id: "relationship:test123".to_string(),
            from_id: "node:from-id".to_string(),
            to_id: "node:to-id".to_string(),
            relationship_type: "custom_type".to_string(),
            properties: serde_json::json!({"custom_prop": "value", "number": 42}),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: RelationshipEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "relationship:test123");
        assert_eq!(deserialized.from_id, "node:from-id");
        assert_eq!(deserialized.to_id, "node:to-id");
        assert_eq!(deserialized.relationship_type, "custom_type");
        assert_eq!(deserialized.properties.get("custom_prop").unwrap(), "value");
        assert_eq!(deserialized.properties.get("number").unwrap(), 42);
    }

    // -----------------------------------------------------------------------
    // persisted_chain_depth (ADR-060 §5)
    // -----------------------------------------------------------------------

    #[test]
    fn persisted_chain_depth_reads_the_reserved_property() {
        // `json!`'s object-literal syntax treats a bare key as a string
        // literal, not a variable reference, so the constant must be
        // parenthesized to be used as a key here.
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 6 });
        assert_eq!(persisted_chain_depth(&props, 10), Some(6));
    }

    #[test]
    fn persisted_chain_depth_none_when_node_never_touched_by_a_play_action() {
        let props = serde_json::json!({ "task": { "status": "open" } });
        assert_eq!(persisted_chain_depth(&props, 10), None);
    }

    #[test]
    fn persisted_chain_depth_none_for_empty_properties() {
        assert_eq!(persisted_chain_depth(&serde_json::json!({}), 10), None);
    }

    #[test]
    fn persisted_chain_depth_none_for_value_too_large_to_fit_a_u8() {
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 9999 });
        assert_eq!(persisted_chain_depth(&props, 10), None);
    }

    #[test]
    fn persisted_chain_depth_none_for_non_numeric_value() {
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): "not-a-number" });
        assert_eq!(persisted_chain_depth(&props, 10), None);
    }

    /// Regression: before this bound check existed, a persisted value of
    /// exactly `u8::MAX` (255) passed straight through as `Some(255)`. The
    /// caller's cycle-depth guard then computed `255 + 1`, which overflows a
    /// `u8` and silently wraps to `0` in a release build (this repo's
    /// release profile leaves `overflow-checks` at its default of off) --
    /// `0 > MAX_CHAIN_DEPTH` is false, so the guard would incorrectly pass
    /// and the chain's depth tracking would be reset. A value that fits in a
    /// `u8` but exceeds the caller's `max_depth` must now be rejected here,
    /// not just values that don't fit the type at all.
    #[test]
    fn persisted_chain_depth_none_for_255_even_though_it_fits_a_u8() {
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 255 });
        assert_eq!(persisted_chain_depth(&props, 10), None);
    }

    #[test]
    fn persisted_chain_depth_accepts_a_value_exactly_at_max_depth() {
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 10 });
        assert_eq!(persisted_chain_depth(&props, 10), Some(10));
    }

    #[test]
    fn persisted_chain_depth_none_one_past_max_depth() {
        let props = serde_json::json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 11 });
        assert_eq!(persisted_chain_depth(&props, 10), None);
    }
}
