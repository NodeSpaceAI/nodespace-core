//! Tauri command layer types.
//!
//! Wire types (`Node`, `NodeUpdate`, `TaskNode`, etc.) and conversion functions
//! (`node_to_typed_value`) are re-exported from `nodespace-types`, which is the
//! single source of truth shared with `nodespace-core`.
//!
//! This module retains only `LocalAgentStatus`, which is specific to the Tauri
//! command layer and mirrors the gRPC JSON blob returned by the daemon.

pub use nodespace_types::{
    node_to_typed_value, nodes_to_typed_values, AiChatMessage, AiChatNode, DeleteResult, EdgeField,
    EnumValue, Node, NodeQuery, NodeReference, NodeUpdate, OrderBy, PersonNodeUpdate,
    ProjectNodeUpdate, RelationshipCardinality,
    RelationshipDirection, SchemaField, SchemaNode, SchemaProtectionLevel, SchemaRelationship,
    TaskNode, TaskNodeUpdate, TaskPriority, TaskStatus, ValidationError,
};

use serde::{Deserialize, Serialize};

/// Current status of a local agent session.
///
/// Deserializes from the JSON blob returned by the daemon's `GetStatus` RPC.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LocalAgentStatus {
    #[default]
    Idle,
    Thinking,
    ToolExecution {
        tool_name: String,
    },
    Streaming,
    Error {
        message: String,
    },
}
