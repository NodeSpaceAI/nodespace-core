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

use serde::{Deserialize, Serialize};

/// Represents hierarchy relationships between nodes (parent-child with ordering)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchyRelationship {
    pub parent_id: String,
    pub child_id: String,
    pub order: f64,
}

/// Represents mention relationships between nodes (bidirectional references)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRelationship {
    pub source_id: String,
    pub target_id: String,
}

/// Represents different types of edge relationships between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EdgeRelationship {
    /// Hierarchical parent-child relationship with ordering
    #[serde(rename = "hierarchy")]
    Hierarchy(HierarchyRelationship),
    /// Bidirectional mention/reference relationship
    #[serde(rename = "mention")]
    Mention(MentionRelationship),
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

    /// A new edge relationship (hierarchy or mention) was created
    EdgeCreated {
        relationship: EdgeRelationship,
        source_client_id: Option<String>,
    },

    /// An existing edge relationship was updated
    EdgeUpdated {
        relationship: EdgeRelationship,
        source_client_id: Option<String>,
    },

    /// An edge relationship was deleted
    EdgeDeleted {
        id: String,
        source_client_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract test: Documents and enforces the exact JSON format for EdgeRelationship
    ///
    /// IMPORTANT: The frontend TypeScript types in event-types.ts MUST match this format.
    /// If this test fails, either the Rust serialization or TypeScript types need updating.
    ///
    /// Serde's `#[serde(tag = "type")]` produces an INTERNALLY-TAGGED format where the
    /// discriminator field is merged with the struct fields (NOT nested).
    #[test]
    fn test_edge_relationship_serialization_contract() {
        // Test Hierarchy variant
        let hierarchy = EdgeRelationship::Hierarchy(HierarchyRelationship {
            parent_id: "parent-123".to_string(),
            child_id: "child-456".to_string(),
            order: 1.5,
        });

        let json = serde_json::to_string(&hierarchy).unwrap();

        // Internally-tagged format: type field merged with struct fields (FLAT, not nested)
        // Expected: {"type":"hierarchy","parentId":"...","childId":"...","order":1.5}
        // NOT: {"type":"hierarchy","hierarchy":{"parentId":"...","childId":"...","order":1.5}}
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.get("type").unwrap(), "hierarchy");
        assert_eq!(parsed.get("parentId").unwrap(), "parent-123");
        assert_eq!(parsed.get("childId").unwrap(), "child-456");
        assert_eq!(parsed.get("order").unwrap(), 1.5);
        // Verify NOT nested - there should be no "hierarchy" key
        assert!(
            parsed.get("hierarchy").is_none(),
            "Should NOT be nested under 'hierarchy' key"
        );

        // Test Mention variant
        let mention = EdgeRelationship::Mention(MentionRelationship {
            source_id: "source-123".to_string(),
            target_id: "target-456".to_string(),
        });

        let json = serde_json::to_string(&mention).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.get("type").unwrap(), "mention");
        assert_eq!(parsed.get("sourceId").unwrap(), "source-123");
        assert_eq!(parsed.get("targetId").unwrap(), "target-456");
        // Verify NOT nested
        assert!(
            parsed.get("mention").is_none(),
            "Should NOT be nested under 'mention' key"
        );
    }

    /// Test that deserialization works correctly (round-trip)
    #[test]
    fn test_edge_relationship_deserialization() {
        // Hierarchy round-trip
        let original = EdgeRelationship::Hierarchy(HierarchyRelationship {
            parent_id: "parent-123".to_string(),
            child_id: "child-456".to_string(),
            order: 1.5,
        });
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: EdgeRelationship = serde_json::from_str(&json).unwrap();

        match deserialized {
            EdgeRelationship::Hierarchy(h) => {
                assert_eq!(h.parent_id, "parent-123");
                assert_eq!(h.child_id, "child-456");
                assert!((h.order - 1.5).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Hierarchy variant"),
        }

        // Mention round-trip
        let original = EdgeRelationship::Mention(MentionRelationship {
            source_id: "source-123".to_string(),
            target_id: "target-456".to_string(),
        });
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: EdgeRelationship = serde_json::from_str(&json).unwrap();

        match deserialized {
            EdgeRelationship::Mention(m) => {
                assert_eq!(m.source_id, "source-123");
                assert_eq!(m.target_id, "target-456");
            }
            _ => panic!("Expected Mention variant"),
        }
    }
}
