//! The AI-chat type family (ADR-088): the one definition of each chat type's
//! wire shape, and of the values nested inside it.
//!
//! `ai-chat` is an abstract base. A chat is always one of its subtypes:
//!
//! - [`AiChatNativeNode`] (`ai-chat-native`): a conversation run by
//!   NodeSpace's own agent loop, with local inference or a compatible
//!   endpoint.
//! - [`AiChatPtyNode`] (`ai-chat-pty`): an external coding agent in a
//!   terminal, captured when the session ends.
//!
//! Both carry the base's fields ([`AiChatBase`]) and their own. The base's
//! fields are stored in the `ai-chat` bucket and each subtype's in its own
//! (ADR-078); [`AiChatNativeNode::from_node`] and [`AiChatPtyNode::from_node`]
//! read a node in either storage or flattened shape.

use serde::{Deserialize, Serialize};

use crate::convert::{core_promoted_fields, flatten_namespaced_properties_at_scope};
use crate::core_type::CoreNodeType;
use crate::node::{Node, NodeEnvelope, ValidationError};

/// The `agent` of a chat NodeSpace's own agent loop runs. A terminal chat's
/// `agent` is its harness (`claude-code`, `codex`, ...).
pub const NODESPACE_AGENT: &str = "nodespace";

/// Where a native chat's inference runs.
///
/// `openai-compat` covers every remotely served model, Ollama included: it is
/// reached through its OpenAI-compatible `/v1` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum AiChatProvider {
    #[default]
    Native,
    OpenaiCompat,
}

impl AiChatProvider {
    /// Every provider, as `(variant, display label)`: the schema's enum.
    pub const ALL: [(AiChatProvider, &'static str); 2] = [
        (AiChatProvider::Native, "Native (Local)"),
        (AiChatProvider::OpenaiCompat, "OpenAI-compatible"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::OpenaiCompat => "openai-compat",
        }
    }
}

/// Whether an inference turn is running on a native chat. Written by the
/// daemon, apart from the `processing` a client sets to request a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatTurnStatus {
    #[default]
    Idle,
    Processing,
}

impl AiChatTurnStatus {
    pub const ALL: [(AiChatTurnStatus, &'static str); 2] = [
        (AiChatTurnStatus::Idle, "Idle"),
        (AiChatTurnStatus::Processing, "Processing"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Processing => "processing",
        }
    }
}

/// Whether a terminal chat's session is running. A finished session is
/// `ended`; resuming one sets it back to `active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatSessionStatus {
    #[default]
    Active,
    Ended,
}

impl AiChatSessionStatus {
    pub const ALL: [(AiChatSessionStatus, &'static str); 2] = [
        (AiChatSessionStatus::Active, "Active"),
        (AiChatSessionStatus::Ended, "Ended"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Ended => "ended",
        }
    }
}

/// Who sent a message in a native chat (ADR-088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatMessageRole {
    User,
    Assistant,
    System,
}

impl AiChatMessageRole {
    pub const ALL: [AiChatMessageRole; 3] = [Self::User, Self::Assistant, Self::System];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
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
/// call as the same write, not to replay its output. The one exception is the
/// edges a relationship write evicted (`replaced`) — a side effect the call's
/// arguments do not describe, and which a later turn needs to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
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

    /// Edges the write evicted as a side effect, each rendered the way
    /// `summary` renders a relationship (`"from -[type]-> to"`). Only
    /// `create_relationship` populates it: a cardinality-one end is honored by
    /// replacing the prior edge, and this is the only place a later turn can
    /// still find the previous holder once the reply prose is gone from
    /// history. Empty for every other write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub replaced: Vec<String>,

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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
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

/// A node an agent turn asked to delete, held until the user confirms.
///
/// The agent's `delete_node` does not delete: it resolves the target and the
/// turn ends asking the user to confirm. The confirmation message carries these
/// records, and only an affirmative reply to that message deletes — against
/// exactly these ids, not a re-resolution of the request. `version` and
/// `descendant_count` are what the user was shown; a change to either between
/// the question and the answer aborts the delete rather than removing
/// something the user did not see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AiChatPendingDeletion {
    /// Bare node id (no `nodespace://` prefix).
    pub node_id: String,

    /// How the node was named to the user.
    pub title: String,

    /// The node's type (e.g. `"task"`).
    pub node_type: String,

    /// The node's version when the user was asked.
    pub version: i64,

    /// Nodes beneath it that the delete cascades to (ADR-041).
    pub descendant_count: u64,
}

/// A single message in an ai-chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessage {
    /// Who sent the message.
    pub role: AiChatMessageRole,

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
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub completed_writes: Vec<AiChatCompletedWrite>,

    /// Concrete graph entities this assistant turn's read-only tool calls
    /// surfaced (deduplicated by node id). Empty for user messages and for
    /// assistant turns whose reads found nothing. See
    /// [`AiChatResolvedEntity`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
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
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub options: Vec<String>,

    /// Deletes this assistant turn is asking the user to confirm. Only the
    /// user's next message can confirm them; see [`AiChatPendingDeletion`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub pending_deletions: Vec<AiChatPendingDeletion>,

    /// How this assistant turn ended, when an agent turn produced it. `None`
    /// for user messages and for assistant text no turn produced (a failed
    /// turn's error notice). See [`AiChatTurnOutcome`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AiChatTurnOutcome>,
}

/// How an agent turn ended — the structural record ADR-038's "at most one
/// clarification per intent" contract is enforced from.
///
/// The session is rebuilt from persisted messages every turn, so whether the
/// current intent already asked the user something has to be answerable from
/// the history alone. The reply's text cannot answer it: a turn that asked in
/// its own words carries no marker, and reading the model's prose for a
/// question is the unreliable channel ADR-038 built `route_clarify` to avoid.
/// What the turn *did* — change the graph, ask through the clarify composer,
/// or neither — is known exactly when it ends, so that is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatTurnOutcome {
    /// A write succeeded, or the turn asked to confirm a delete. Resolves the
    /// current intent: the user's next message starts a new one.
    Acted,
    /// The turn put a composed clarifying question to the user.
    Clarified,
    /// Anything else: a prose reply, including one after reads only or after a
    /// write that failed. Does not resolve the intent, and counts against its
    /// one clarification: reading and then replying looks the same whether the
    /// reply showed what was found or asked the user about it.
    Replied,
}

/// The fields every chat carries, declared by the abstract `ai-chat` schema
/// and embedded by each subtype's struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatBase {
    /// Who runs the conversation: [`NODESPACE_AGENT`], or a terminal harness.
    pub agent: String,
    /// The model identifier, when known. Never a harness name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Prose summary of the conversation, filled per subtype.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// When the chat was last active (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active: Option<String>,
}

impl AiChatBase {
    fn from_flat(props: &serde_json::Value) -> Self {
        Self {
            agent: string_prop(props, "agent").unwrap_or_default(),
            model: string_prop(props, "model"),
            summary: string_prop(props, "summary"),
            last_active: string_prop(props, "last_active"),
        }
    }
}

/// An `ai-chat-native` node: a conversation run by NodeSpace's agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatNativeNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the chat's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(flatten)]
    pub base: AiChatBase,
    pub provider: AiChatProvider,
    pub turn_status: AiChatTurnStatus,
    /// Approximate token count of the conversation's context.
    pub context_tokens: u64,
    /// The conversation, in order.
    pub messages: Vec<AiChatMessage>,
}

impl AiChatNativeNode {
    /// Read a native chat from a node, in storage shape (bucketed per schema)
    /// or already flattened.
    ///
    /// Reads only the declared snake_case names. Messages are decoded one by
    /// one, not as one `Vec`: a single unreadable message would otherwise
    /// discard the whole conversation, and a caller that then writes the
    /// messages back would persist the loss. The unreadable ones are returned
    /// by [`Self::from_node_reporting`] for a caller that logs them.
    ///
    /// # Errors
    ///
    /// [`ValidationError::InvalidNodeType`] when the node is not exactly an
    /// `ai-chat-native` node.
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        Self::from_node_reporting(node).map(|(chat, _unreadable)| chat)
    }

    /// [`Self::from_node`], also returning the decode error of each message
    /// that could not be read and was left out.
    pub fn from_node_reporting(node: Node) -> Result<(Self, Vec<String>), ValidationError> {
        let (envelope, props) = chat_envelope(node, CoreNodeType::AiChatNative)?;

        let mut unreadable = Vec::new();
        let messages = props
            .get("messages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(
                        |m| match serde_json::from_value::<AiChatMessage>(m.clone()) {
                            Ok(message) => Some(message),
                            Err(e) => {
                                unreadable.push(e.to_string());
                                None
                            }
                        },
                    )
                    .collect()
            })
            .unwrap_or_default();

        let chat = Self {
            envelope,
            base: AiChatBase::from_flat(&props),
            provider: enum_prop(&props, "provider"),
            turn_status: enum_prop(&props, "turn_status"),
            context_tokens: props
                .get("context_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or_default(),
            messages,
        };
        Ok((chat, unreadable))
    }

    /// The conversation state a turn writes, as a properties patch: the turn
    /// status and the whole message list, under their storage names. The
    /// update pipeline places both in this type's bucket and leaves every
    /// other field as stored.
    pub fn conversation_patch(&self) -> serde_json::Value {
        serde_json::json!({
            "turn_status": self.turn_status,
            "messages": self.messages,
        })
    }
}

/// An `ai-chat-pty` node: an external coding agent in a terminal.
///
/// It has no messages: NodeSpace sees only the terminal's output stream. What
/// capture records when the session ends is the base's `summary` and the
/// fields below.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatPtyNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the chat's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(flatten)]
    pub base: AiChatBase,
    pub session_status: AiChatSessionStatus,
    /// The session's id. Names state on this machine, so it never leaves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The raw terminal scrollback. Never leaves the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The exit code of the session's process, once it has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
}

impl AiChatPtyNode {
    /// Read a terminal chat from a node, in storage shape (bucketed per
    /// schema) or already flattened.
    ///
    /// # Errors
    ///
    /// [`ValidationError::InvalidNodeType`] when the node is not exactly an
    /// `ai-chat-pty` node.
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        let (envelope, props) = chat_envelope(node, CoreNodeType::AiChatPty)?;
        Ok(Self {
            envelope,
            base: AiChatBase::from_flat(&props),
            session_status: enum_prop(&props, "session_status"),
            session_id: string_prop(&props, "session_id"),
            transcript: string_prop(&props, "transcript"),
            exit_code: props.get("exit_code").and_then(|v| v.as_i64()),
        })
    }
}

/// Split a chat node into its envelope and its flat field values.
///
/// The fields are read across the subtype's bucket and the inherited
/// `ai-chat` one, nearest first. The envelope keeps what the chain does not
/// declare: extension fields.
fn chat_envelope(
    node: Node,
    core: CoreNodeType,
) -> Result<(NodeEnvelope, serde_json::Value), ValidationError> {
    if !core.is_exactly(&node.node_type) {
        return Err(ValidationError::InvalidNodeType(format!(
            "Expected '{core}', got '{}'",
            node.node_type
        )));
    }
    let chain: Vec<&str> = core.chain().into_iter().map(CoreNodeType::as_str).collect();
    let props = flatten_namespaced_properties_at_scope(&node.properties, &chain);

    let mut extension = props.clone();
    if let Some(obj) = extension.as_object_mut() {
        for field in core_promoted_fields(core) {
            obj.remove(field.storage);
        }
    }
    let envelope = NodeEnvelope {
        properties: extension,
        ..node
    };
    Ok((envelope, props))
}

fn string_prop(props: &serde_json::Value, key: &str) -> Option<String> {
    props.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// A closed-enum field. An absent value reads as the schema's default, which
/// is the enum's; so does a value outside the vocabulary, which no validated
/// write can store.
fn enum_prop<T: serde::de::DeserializeOwned + Default>(props: &serde_json::Value, key: &str) -> T {
    props
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn native(properties: serde_json::Value) -> Node {
        Node::new("ai-chat-native".to_string(), "Chat".to_string(), properties)
    }

    #[test]
    fn a_native_chat_reads_its_own_bucket_and_the_inherited_one() {
        let node = native(json!({
            "ai-chat": { "agent": "nodespace", "model": "gemma-4-e4b" },
            "ai-chat-native": {
                "provider": "openai-compat",
                "turn_status": "processing",
                "context_tokens": 42,
                "messages": [{ "role": "user", "content": "hi" }],
                "custom:pinned": true
            },
            "task": { "status": "open" }
        }));
        let chat = AiChatNativeNode::from_node(node).unwrap();
        assert_eq!(chat.base.agent, NODESPACE_AGENT);
        assert_eq!(chat.base.model.as_deref(), Some("gemma-4-e4b"));
        assert_eq!(chat.provider, AiChatProvider::OpenaiCompat);
        assert_eq!(chat.turn_status, AiChatTurnStatus::Processing);
        assert_eq!(chat.context_tokens, 42);
        assert_eq!(chat.messages.len(), 1);
        // Only the extension field is left; the dormant `task` bucket is not
        // part of the chain.
        assert_eq!(chat.envelope.properties, json!({ "custom:pinned": true }));
    }

    #[test]
    fn absent_fields_read_as_the_schema_defaults() {
        let chat = AiChatNativeNode::from_node(native(json!({}))).unwrap();
        assert_eq!(chat.provider, AiChatProvider::Native);
        assert_eq!(chat.turn_status, AiChatTurnStatus::Idle);
        assert_eq!(chat.context_tokens, 0);
        assert!(chat.messages.is_empty());
        assert_eq!(chat.base.model, None);
    }

    #[test]
    fn a_flattened_node_reads_the_same_as_a_bucketed_one() {
        let chat = AiChatNativeNode::from_node(native(json!({
            "agent": "nodespace",
            "turn_status": "processing",
            "messages": []
        })))
        .unwrap();
        assert_eq!(chat.base.agent, "nodespace");
        assert_eq!(chat.turn_status, AiChatTurnStatus::Processing);
    }

    #[test]
    fn one_unreadable_message_is_left_out_and_reported() {
        let node = native(json!({ "ai-chat-native": { "messages": [
            { "role": "user", "content": "kept" },
            { "content": "no role" },
            { "role": "assistant", "content": "also kept" }
        ] } }));
        let (chat, unreadable) = AiChatNativeNode::from_node_reporting(node).unwrap();
        let kept: Vec<&str> = chat.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(kept, ["kept", "also kept"]);
        assert_eq!(unreadable.len(), 1);
    }

    /// A role is one of the three the type names. A message stored with any
    /// other is unreadable like any malformed message: left out and reported,
    /// and the chat still reads.
    #[test]
    fn a_message_with_a_role_outside_the_vocabulary_is_left_out_and_reported() {
        let node = native(json!({ "ai-chat-native": { "messages": [
            { "role": "user", "content": "kept" },
            { "role": "tool_call", "content": "search" },
            { "role": "system", "content": "also kept" }
        ] } }));
        let (chat, unreadable) = AiChatNativeNode::from_node_reporting(node).unwrap();
        let roles: Vec<AiChatMessageRole> = chat.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [AiChatMessageRole::User, AiChatMessageRole::System]);
        assert_eq!(unreadable.len(), 1);
    }

    /// No validated write stores a value outside a closed enum. One that got
    /// there some other way reads as the field's default, and the chat still
    /// reads.
    #[test]
    fn a_stored_value_outside_a_closed_enum_reads_as_the_default() {
        let chat = AiChatNativeNode::from_node(native(json!({
            "ai-chat-native": { "provider": "pty", "turn_status": "archived" }
        })))
        .unwrap();
        assert_eq!(chat.provider, AiChatProvider::default());
        assert_eq!(chat.turn_status, AiChatTurnStatus::default());

        let pty = Node::new(
            "ai-chat-pty".to_string(),
            "Session".to_string(),
            json!({ "ai-chat-pty": { "session_status": "archived" } }),
        );
        let chat = AiChatPtyNode::from_node(pty).unwrap();
        assert_eq!(chat.session_status, AiChatSessionStatus::default());
    }

    #[test]
    fn the_conversation_patch_names_only_the_turn_state_and_the_messages() {
        let mut chat = AiChatNativeNode::from_node(native(json!({
            "ai-chat": { "agent": "nodespace", "model": "m" },
            "ai-chat-native": { "provider": "openai-compat", "messages": [] }
        })))
        .unwrap();
        chat.turn_status = AiChatTurnStatus::Processing;
        chat.messages.push(AiChatMessage {
            role: AiChatMessageRole::User,
            content: "hi".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
            pending_deletions: Vec::new(),
            outcome: None,
        });
        assert_eq!(
            chat.conversation_patch(),
            json!({
                "turn_status": "processing",
                "messages": [{ "role": "user", "content": "hi" }]
            })
        );
    }

    #[test]
    fn a_terminal_chat_reads_its_session_fields() {
        let node = Node::new(
            "ai-chat-pty".to_string(),
            "Session".to_string(),
            json!({
                "ai-chat": { "agent": "claude-code", "summary": "Fixed the build" },
                "ai-chat-pty": {
                    "session_status": "ended",
                    "session_id": "abc",
                    "transcript": "hello",
                    "exit_code": 0
                }
            }),
        );
        let chat = AiChatPtyNode::from_node(node).unwrap();
        assert_eq!(chat.base.agent, "claude-code");
        assert_eq!(chat.base.summary.as_deref(), Some("Fixed the build"));
        assert_eq!(chat.session_status, AiChatSessionStatus::Ended);
        assert_eq!(chat.session_id.as_deref(), Some("abc"));
        assert_eq!(chat.transcript.as_deref(), Some("hello"));
        assert_eq!(chat.exit_code, Some(0));
        assert_eq!(chat.envelope.properties, json!({}));
    }

    #[test]
    fn a_subtype_struct_is_not_borrowed_by_another_type() {
        let pty = Node::new("ai-chat-pty".to_string(), "S".to_string(), json!({}));
        assert!(AiChatNativeNode::from_node(pty.clone()).is_err());
        let base = Node::new("ai-chat".to_string(), "S".to_string(), json!({}));
        assert!(AiChatNativeNode::from_node(base.clone()).is_err());
        assert!(AiChatPtyNode::from_node(base).is_err());
        assert!(AiChatPtyNode::from_node(pty).is_ok());
    }

    #[test]
    fn the_enums_serialize_as_their_stored_values() {
        for (provider, _) in AiChatProvider::ALL {
            assert_eq!(
                serde_json::to_value(provider).unwrap(),
                json!(provider.as_str())
            );
        }
        for (status, _) in AiChatTurnStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        for (status, _) in AiChatSessionStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        for role in AiChatMessageRole::ALL {
            assert_eq!(serde_json::to_value(role).unwrap(), json!(role.as_str()));
        }
    }
}
