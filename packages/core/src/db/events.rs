//! Domain Events for SurrealStore
//!
//! This module defines the domain events emitted by SurrealStore when data changes.
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
//! 1. SurrealStore performs a data operation (create, update, delete)
//! 2. Domain event is emitted via broadcast channel
//! 3. All subscribers receive the event asynchronously
//! 4. LiveQueryService (Tauri layer) listens to events and forwards to frontend
//!
//! # Unified Relationship Event System (Issue #811)
//!
//! All relationships (`has_child`, `member_of`, `mentions`, and custom types)
//! use a generic `RelationshipEvent` struct with `relationship_type` for discrimination.
//! This allows adding new relationship types without modifying the event system.

use serde::{Deserialize, Serialize};

/// Unified relationship event for all relationship types (Issue #811)
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipEvent {
    /// Unique relationship ID in SurrealDB format (e.g., "relationship:abc123")
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

/// Domain events emitted by SurrealStore
///
/// These events are emitted whenever data changes in the database.
/// They represent domain-level changes, not database operations.
///
/// Each event includes an optional `source_client_id` to identify the originating client,
/// allowing event subscribers to filter out their own events (prevent feedback loops).
///
/// Node events send only the `node_id` (not full payload) for efficiency.
/// Subscribers fetch the full node data via `get_node()` if needed (Issue #724).
/// This reduces bandwidth during bulk operations and lets clients decide what they need.
#[derive(Debug, Clone)]
pub enum DomainEvent {
    /// A new node was created
    NodeCreated {
        node_id: String,
        /// Node type (e.g., "collection", "task", "text") - included for reactive UI updates
        /// that need to know the type without fetching the full node
        node_type: String,
        source_client_id: Option<String>,
    },

    /// An existing node was updated
    NodeUpdated {
        node_id: String,
        source_client_id: Option<String>,
    },

    /// A node was deleted
    NodeDeleted {
        id: String,
        source_client_id: Option<String>,
    },

    // ============================================================================
    // Unified Relationship Events (Issue #811)
    // All relationship types (has_child, member_of, mentions, custom) use these
    // generic events. No backward compatibility - old EdgeCreated/etc removed.
    // ============================================================================
    /// A new relationship was created (unified format for all relationship types)
    ///
    /// Supports: `has_child`, `member_of`, `mentions`, and custom relationship types.
    RelationshipCreated {
        relationship: RelationshipEvent,
        source_client_id: Option<String>,
    },

    /// An existing relationship was updated (unified format for all relationship types)
    ///
    /// Typically used for reordering (updating `order` property on `has_child` relationships).
    RelationshipUpdated {
        relationship: RelationshipEvent,
        source_client_id: Option<String>,
    },

    /// A relationship was deleted (unified format for all relationship types)
    ///
    /// Contains relationship ID and node IDs for handlers that need them
    /// (e.g., hierarchy operations need from_id/to_id to update the structure tree).
    RelationshipDeleted {
        /// The SurrealDB relationship ID (e.g., "relationship:abc123")
        id: String,
        /// Source node ID (the "from" node in the relationship)
        from_id: String,
        /// Target node ID (the "to" node in the relationship)
        to_id: String,
        /// Relationship type hint for handlers that need it
        relationship_type: String,
        source_client_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract test: Documents and enforces the exact JSON format for RelationshipEvent (Issue #811)
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
}
