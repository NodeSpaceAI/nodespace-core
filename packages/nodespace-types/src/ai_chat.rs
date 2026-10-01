use serde::{Deserialize, Serialize};

use crate::node::NodeEnvelope;

/// A graph write completed during an assistant turn.
///
/// Mirrors `nodespace_core::models::AiChatCompletedWrite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatCompletedWrite {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Edges the write evicted, rendered `"from -[type]-> to"`. Only a
    /// cardinality-one `create_relationship` populates it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub replaced: Vec<String>,
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatResolvedEntity {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
}

/// A node an agent turn asked to delete, held until the user confirms.
///
/// Mirrors `nodespace_core::models::AiChatPendingDeletion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AiChatPendingDeletion {
    pub node_id: String,
    pub title: String,
    pub node_type: String,
    pub version: i64,
    pub descendant_count: u64,
}

/// A single message in an ai-chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
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
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub completed_writes: Vec<AiChatCompletedWrite>,
    /// Graph entities this assistant turn's reads surfaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
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
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub options: Vec<String>,

    /// Deletes this assistant turn is asking the user to confirm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub pending_deletions: Vec<AiChatPendingDeletion>,

    /// How this assistant turn ended, when an agent turn produced it.
    /// Mirrors `nodespace_core::models::AiChatTurnOutcome`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AiChatTurnOutcome>,
}

/// How an agent turn ended.
///
/// Mirrors `nodespace_core::models::AiChatTurnOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatTurnOutcome {
    Acted,
    Clarified,
    Replied,
}

/// Wire shape for ai-chat nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "ai-chat"`. Fields map
/// directly to the TypeScript `AiChatNode` interface.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
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
