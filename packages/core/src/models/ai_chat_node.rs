//! Strongly-Typed AiChatNode
//!
//! Provides compile-time type safety for `ai-chat` nodes, replacing the fragile
//! raw `serde_json::Value` path lookups that the daemon previously used to read
//! and write chat messages, status, model, and provider.
//!
//! Mirrors the [`TaskNode`](crate::models::TaskNode) pattern: data is stored in
//! the node's `properties` under a type namespace (`properties["ai-chat"]`), and
//! `from_node` / `into_node` convert between the universal [`Node`] and this
//! strongly-typed struct.
//!
//! # Status values
//!
//! Two orthogonal state machines used to share one `status` key: a turn state
//! the daemon drives (`processing` while inference runs, `idle` once it
//! completes) and a session lifecycle the PTY path uses (`active` /
//! `archived`). They are now two independent properties instead — `status`
//! no longer exists on this struct:
//!
//! - [`AiChatNode::turn_status`] — daemon-owned, `"idle"` / `"processing"`.
//! - [`AiChatNode::session_status`] — PTY-owned, `"active"` / `"archived"`.
//!
//! Each carries its own enum in the schema (`core_schemas.rs`), so a
//! nonsensical combination like `session_status: "archived"` with
//! `turn_status: "processing"` is representable and independently valid —
//! archiving a session no longer requires forgetting what turn state it was
//! in, and vice versa. Each is written by exactly one owner: the daemon never
//! writes `session_status`, and the PTY path never writes `turn_status`.

use crate::models::{Node, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The node type discriminator for ai-chat nodes.
pub const AI_CHAT_NODE_TYPE: &str = "ai-chat";

/// The properties namespace key under which ai-chat data is stored.
const AI_CHAT_NAMESPACE: &str = "ai-chat";

fn default_version() -> i64 {
    1
}

/// A graph write completed during an assistant turn.
///
/// Only successful, state-changing tool calls are recorded. This is the durable
/// evidence that a turn's write actually happened: the agent session is rebuilt
/// from scratch on every turn, so without it the next turn sees the user's
/// original instruction alongside a prose claim of completion and no proof the
/// write occurred — and may repeat it.
///
/// Carries the tool name, the affected node, a short label, and the canonical
/// arguments the call was made with. Tool *results* are not persisted: the
/// purpose is to establish *that* the write happened and to recognise a later
/// call as the same write, not to replay its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatCompletedWrite {
    /// Name of the tool that performed the write (e.g. `"create_node"`).
    pub tool: String,

    /// ID of the node the write produced or affected, when the tool reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,

    /// Short human-readable label for the written node, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// The call's arguments, canonicalised (JSON key order normalised, parameter
    /// aliases resolved). Together with `tool` this is the write's identity for
    /// the cross-turn duplicate guard: a later call matching both is the same
    /// write, not a new one.
    ///
    /// Two forms, both produced by `canonical_args_identity`: the canonical JSON
    /// verbatim when it is small enough to store, or `sha256:<hex>` of that same
    /// string when it is not (see `CANONICAL_ARGS_MAX_CHARS`). The digest keeps
    /// large writes — an entire markdown import, say — guarded without copying
    /// their content into this message history a second time. The forms cannot
    /// be confused: canonical JSON always starts with `{`.
    ///
    /// Always present. An identity is what makes a recorded write enforceable,
    /// so a write recorded without one would be indistinguishable from an
    /// unguarded tool while still looking wired up.
    pub canonical_args: String,
}

/// A concrete graph entity a read-only tool call surfaced during an assistant
/// turn.
///
/// `completed_writes` gives the next turn durable proof of what a write-tool
/// call did; nothing analogous existed for reads, so a turn that merely
/// *looked up* a node (`search_nodes`, `get_node`, ...) left no structured
/// trace once the ephemeral session ended — only the assistant's prose reply
/// survived, with no node id in it. A follow-up like "update that" then has
/// nothing to resolve "that" against. This is the read-side counterpart:
/// minimal identity only (no mutable fields, so it cannot go stale in a way
/// that misleads), populated from the same tool-execution records
/// `completed_writes` already derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatResolvedEntity {
    /// ID of the node a read tool surfaced (as a `nodespace://` URI, matching
    /// the form the model uses to refer to nodes elsewhere).
    pub node_id: String,

    /// Short human-readable title for the node, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The node's type (e.g. `"task"`), when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
}

/// A single message in an ai-chat conversation.
///
/// Mirrors the frontend `AiChatMessage` TypeScript interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessage {
    /// Sender role: `"user"`, `"assistant"`, or `"system"`.
    pub role: String,

    /// Message text.
    pub content: String,

    /// When the message was created (RFC3339), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    /// Model chain-of-thought reasoning toward the answer, when captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    /// Graph writes this assistant turn completed. Empty for user messages and
    /// for assistant turns that only read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_writes: Vec<AiChatCompletedWrite>,

    /// Concrete graph entities this assistant turn's read-only tool calls
    /// surfaced (deduplicated by node id). Empty for user messages and for
    /// assistant turns whose reads found nothing. See
    /// [`AiChatResolvedEntity`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_entities: Vec<AiChatResolvedEntity>,

    /// The clarifying question, when this message is a `route_clarify` turn
    /// (ADR-038) rather than an ordinary reply. `content` still carries the
    /// flattened `"{opener}. {question}\n\n- opt1\n- opt2"` text for plain-text
    /// readers and the LLM-facing history; this field plus `options` is the
    /// same data unflattened, so the frontend can render clickable options
    /// instead of parsing markdown bullets back out of prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,

    /// Concrete options offered alongside `question`. Only meaningful when
    /// `question` is `Some`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Strongly-typed view of an `ai-chat` node.
///
/// Construct from a generic [`Node`] with [`AiChatNode::from_node`] and convert
/// back with [`AiChatNode::into_node`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatNode {
    /// Unique identifier.
    pub id: String,

    /// Node type (always `"ai-chat"`).
    #[serde(rename = "nodeType")]
    pub node_type: String,

    /// Title/label of the chat (the node's primary content).
    pub content: String,

    /// Optimistic concurrency control version.
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp.
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp.
    pub modified_at: DateTime<Utc>,

    /// Inference turn state (`"idle"` / `"processing"`), daemon-owned. See
    /// the module docs for why this is split from `session_status`.
    #[serde(default)]
    pub turn_status: String,

    /// Session lifecycle (`"active"` / `"archived"`), PTY-owned. See the
    /// module docs for why this is split from `turn_status`.
    #[serde(default)]
    pub session_status: String,

    /// Inference provider (e.g. `"native"`, `"openai-compat"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Model identifier used for inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Ordered conversation history.
    #[serde(default)]
    pub messages: Vec<AiChatMessage>,
}

impl AiChatNode {
    /// Build an [`AiChatNode`] from a generic [`Node`].
    ///
    /// Reads ai-chat fields from the `properties["ai-chat"]` namespace, falling
    /// back to flat `properties` when the namespace is absent (e.g. after
    /// `flatten_properties_for_api` has promoted them). Mirrors the dual
    /// nested/flat handling in [`TaskNode::from_node`](crate::models::TaskNode).
    ///
    /// Reads ONLY the canonical snake_case schema names (`turn_status`,
    /// `session_status`) — deliberately not dual-cased. An earlier version of
    /// this function checked camelCase first as a defensive fallback for a
    /// frontend write that (at the time) sent `turnStatus`/`sessionStatus`
    /// verbatim. That turned out to be actively wrong, not just redundant:
    /// updates here deep-merge onto the existing stored object rather than
    /// replacing it, so a stray camelCase key, once written, is never
    /// cleaned up — the daemon's own canonical-key rewrites land beside it,
    /// not over it. Checking camelCase first then means a *stale* value
    /// permanently shadows every subsequent *fresh* canonical write (this
    /// broke `ai_chat_send_to_idle_test`: the turn completed and the daemon
    /// correctly wrote `turn_status: "idle"`, but the stale `turnStatus:
    /// "processing"` from the original send kept winning forever). The real
    /// fix is at the write side: every writer — daemon and frontend alike —
    /// must use this exact canonical key, so there is only ever one key to
    /// read.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::InvalidNodeType`] when the node is not an
    /// ai-chat node.
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != AI_CHAT_NODE_TYPE {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected '{AI_CHAT_NODE_TYPE}', got '{}'",
                node.node_type
            )));
        }

        // Prefer the nested namespace; fall back to flat properties.
        let props = node
            .properties
            .get(AI_CHAT_NAMESPACE)
            .filter(|v| v.is_object())
            .unwrap_or(&node.properties);

        let turn_status = props
            .get("turn_status")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let session_status = props
            .get("session_status")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let provider = props
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let model = props
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Decoded per message, not as one `Vec`. Decoding the whole array at
        // once means a single unreadable message discards the entire
        // conversation — and because a caller that then writes the node back
        // (status updates do) re-serialises from this struct, that loss is
        // persisted. Per-message decoding contains the damage to the message
        // actually at fault, and logs it rather than failing silently.
        let messages = props
            .get("messages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(
                        |m| match serde_json::from_value::<AiChatMessage>(m.clone()) {
                            Ok(msg) => Some(msg),
                            Err(e) => {
                                tracing::error!(
                                    node_id = %node.id,
                                    error = %e,
                                    "dropping unreadable ai-chat message; the rest of the \
                                     conversation is preserved"
                                );
                                None
                            }
                        },
                    )
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            id: node.id,
            node_type: AI_CHAT_NODE_TYPE.to_string(),
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            turn_status,
            session_status,
            provider,
            model,
            messages,
        })
    }

    /// Serialize the ai-chat fields as the namespace value
    /// (`{ "turn_status": ..., "session_status": ..., "provider": ...,
    /// "model": ..., "messages": [...] }`).
    ///
    /// Use this to splice the typed fields back into an existing `properties`
    /// object without disturbing sibling namespaces — the read-modify-write
    /// pattern the daemon relies on.
    ///
    /// `AiChatNode` is the authoritative model for the `"ai-chat"` namespace:
    /// this rebuilds the namespace value entirely from the struct's fields, so
    /// every key stored under `"ai-chat"` MUST be represented as a field here.
    /// An unmodeled key would be dropped on the next write — when adding a field
    /// to the stored shape, add it to the struct too. (Sibling namespaces
    /// outside `"ai-chat"` are untouched; only this namespace is rebuilt.)
    pub fn to_properties_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "turn_status".to_string(),
            serde_json::json!(self.turn_status),
        );
        map.insert(
            "session_status".to_string(),
            serde_json::json!(self.session_status),
        );
        if let Some(provider) = &self.provider {
            map.insert("provider".to_string(), serde_json::json!(provider));
        }
        if let Some(model) = &self.model {
            map.insert("model".to_string(), serde_json::json!(model));
        }
        map.insert(
            "messages".to_string(),
            serde_json::to_value(&self.messages).unwrap_or(serde_json::Value::Array(vec![])),
        );
        serde_json::Value::Object(map)
    }

    /// Convert back into a universal [`Node`], storing ai-chat fields under the
    /// `properties["ai-chat"]` namespace.
    ///
    /// Note: this produces a fresh `properties` object containing only the
    /// ai-chat namespace. When you need to preserve sibling namespaces, prefer
    /// merging [`AiChatNode::to_properties_value`] into the original
    /// `node.properties` instead.
    pub fn into_node(self) -> Node {
        let mut properties = serde_json::Map::new();
        properties.insert(AI_CHAT_NAMESPACE.to_string(), self.to_properties_value());

        Node {
            id: self.id,
            node_type: AI_CHAT_NODE_TYPE.to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties: serde_json::Value::Object(properties),
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title: None, // ai-chat nodes have no indexed title
            lifecycle_status: "active".to_string(),
        }
    }

    /// Build a temporary [`Node`] without consuming `self`.
    pub fn as_node(&self) -> Node {
        self.clone().into_node()
    }
}
