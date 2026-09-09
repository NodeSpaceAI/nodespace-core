use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::helpers::is_active_lifecycle;

/// A graph write completed during an assistant turn.
///
/// Mirrors `nodespace_core::models::AiChatCompletedWrite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatCompletedWrite {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The write's identity for the cross-turn duplicate guard: canonical JSON
    /// verbatim, or `sha256:<hex>` of it when too large to store. Always
    /// present — this is the struct that serialises to the frontend, so making
    /// it optional here would contradict the TypeScript mirror.
    pub canonical_args: String,
}

/// A concrete graph entity a read-only tool call surfaced during an assistant
/// turn.
///
/// Mirrors `nodespace_core::models::AiChatResolvedEntity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatResolvedEntity {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
}

/// A single message in an ai-chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Graph writes this assistant turn completed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_writes: Vec<AiChatCompletedWrite>,
    /// Graph entities this assistant turn's reads surfaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_entities: Vec<AiChatResolvedEntity>,

    /// The clarifying question, when this message is a `route_clarify` turn
    /// (ADR-038) rather than an ordinary reply. `content` still carries the
    /// flattened text; this plus `options` is the same data unflattened, for
    /// the frontend to render clickable options with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,

    /// Concrete options offered alongside `question`. Only meaningful when
    /// `question` is `Some`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Wire shape for ai-chat nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "ai-chat"`. Fields map
/// directly to the TypeScript `AiChatNode` interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatNode {
    pub id: String,
    #[serde(rename = "nodeType")]
    pub node_type: String,
    pub content: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub properties: serde_json::Value,
    #[serde(default, skip_serializing_if = "is_active_lifecycle")]
    pub lifecycle_status: String,
    /// Inference turn state (`"idle"` / `"processing"`), daemon-owned.
    pub turn_status: String,
    /// Session lifecycle (`"active"` / `"archived"`), PTY-owned. Independent
    /// of `turn_status` — see `nodespace_core::models::AiChatNode`'s module
    /// docs for why these are two properties rather than one shared `status`.
    pub session_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<AiChatMessage>,
}
