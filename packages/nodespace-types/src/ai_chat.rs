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
//!
//! A native chat's messages are [`AiChatMessageNode`]s (`ai-chat-message`):
//! its `has_child` children, in conversation order. What a message wrote,
//! looked up or asked to delete is recorded on edges from it
//! ([`AiChatWroteEdge`], [`AiChatResolvedEdge`], [`AiChatPendingDeleteEdge`]).

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

/// Who sent a message in a native chat (ADR-088). A message written without
/// a role is the user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum AiChatMessageRole {
    #[default]
    User,
    Assistant,
    System,
}

impl AiChatMessageRole {
    pub const ALL: [(AiChatMessageRole, &'static str); 3] = [
        (AiChatMessageRole::User, "User"),
        (AiChatMessageRole::Assistant, "Assistant"),
        (AiChatMessageRole::System, "System"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

/// The relationship from a message to a node it wrote (ADR-088 §3).
pub const AI_CHAT_WROTE: &str = "wrote";
/// The relationship from a message to a node its reads surfaced.
pub const AI_CHAT_RESOLVED: &str = "resolved";
/// The relationship from a message to a node it asked to delete, held until
/// the user answers.
pub const AI_CHAT_PENDING_DELETE: &str = "pending_delete";
/// The relationship from a native chat to a node it pins (ADR-090 §4).
pub const AI_CHAT_PINS: &str = "pins";

/// One successful write tool call, as its message's `wrote` edge records it.
///
/// The agent session is rebuilt from the stored conversation on every turn, so
/// this is the durable evidence that a write happened. Tool results are not
/// kept: the record establishes that the write happened and lets a later call
/// be recognised as the same write. The one exception is the edges a
/// relationship write evicted (`replaced`), a side effect the call's
/// arguments do not describe and a later turn needs to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct AiChatWrite {
    /// The call's position among the message's writes, across all of its
    /// `wrote` edges.
    pub seq: u32,

    /// Name of the tool that performed the write (`create_node`, ...).
    pub tool: String,

    /// Short human-readable label for what was written, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// The call's arguments, canonicalised. With `tool` this is the write's
    /// identity for the cross-turn duplicate guard: a later call matching both
    /// is the same write. Either the canonical JSON verbatim or, when that is
    /// too large to store, `sha256:<hex>` of it; canonical JSON always starts
    /// with `{`, so the two forms cannot be confused.
    pub canonical_args: String,

    /// Edges the write evicted, each rendered `"from -[type]-> to"`. Only a
    /// relationship write that replaced a cardinality-one edge has any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub replaced: Vec<String>,
}

/// The fields of a `wrote` edge: every write the message made to the node,
/// in call order. One edge joins a message and a node, and a turn may write
/// the same node more than once (create it, then link it), so the edge holds
/// a list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AiChatWroteEdge {
    pub writes: Vec<AiChatWrite>,
}

/// The fields of a `resolved` edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AiChatResolvedEdge {
    /// Name of the read tool that surfaced the node.
    pub tool: String,
}

/// The fields of a `pending_delete` edge: what the user was shown when the
/// delete was proposed. A node that differs from it when the user answers is
/// not deleted, so nothing the user did not see is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AiChatPendingDeleteEdge {
    /// The node's version.
    pub version: i64,
    /// How many nodes beneath it the delete cascades to.
    pub descendant_count: u64,
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

impl AiChatTurnOutcome {
    pub const ALL: [(AiChatTurnOutcome, &'static str); 3] = [
        (AiChatTurnOutcome::Acted, "Acted"),
        (AiChatTurnOutcome::Clarified, "Clarified"),
        (AiChatTurnOutcome::Replied, "Replied"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acted => "acted",
            Self::Clarified => "clarified",
            Self::Replied => "replied",
        }
    }
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
}

impl AiChatNativeNode {
    /// Read a native chat from a node, in storage shape (bucketed per schema)
    /// or already flattened. Its messages are its `ai-chat-message` children,
    /// not part of the node.
    ///
    /// # Errors
    ///
    /// [`ValidationError::InvalidNodeType`] when the node is not exactly an
    /// `ai-chat-native` node.
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        let (envelope, props) = typed_envelope(node, CoreNodeType::AiChatNative)?;
        Ok(Self {
            envelope,
            base: AiChatBase::from_flat(&props),
            provider: enum_prop(&props, "provider"),
            turn_status: enum_prop(&props, "turn_status"),
            context_tokens: props
                .get("context_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or_default(),
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
    /// The id the harness gave its own conversation, which its resume flag
    /// takes. Recorded when the session ends, for the harnesses that keep
    /// one. Names state on this machine, so it never leaves it.
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
        let (envelope, props) = typed_envelope(node, CoreNodeType::AiChatPty)?;
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

/// An `ai-chat-message` node: one message of a native chat (ADR-088 §3).
///
/// A `has_child` child of its chat, in conversation order. Its text is its
/// `content`; a clarifying question is the content too, with the choices it
/// offers in `options`. What the message wrote, looked up or asked to delete
/// is on its `wrote`, `resolved` and `pending_delete` edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessageNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the message's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    /// Who sent the message.
    pub role: AiChatMessageRole,
    /// When the message was sent (RFC 3339), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// The model's chain-of-thought toward the answer, when captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// How the turn that produced an assistant message ended. `None` for a
    /// user message and for assistant text no turn produced (a failed turn's
    /// error notice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AiChatTurnOutcome>,
    /// The choices offered with a clarifying question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub options: Vec<String>,
}

impl AiChatMessageNode {
    /// Read a message from a node, in storage shape or already flattened.
    ///
    /// # Errors
    ///
    /// [`ValidationError::InvalidNodeType`] when the node is not exactly an
    /// `ai-chat-message` node.
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        let (envelope, props) = typed_envelope(node, CoreNodeType::AiChatMessage)?;
        Ok(Self {
            envelope,
            role: enum_prop(&props, "role"),
            timestamp: string_prop(&props, "timestamp"),
            reasoning: string_prop(&props, "reasoning"),
            outcome: props
                .get("outcome")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            options: props
                .get("options")
                .and_then(|v| v.as_array())
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|o| o.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Split a node of the chat family into its envelope and its flat field
/// values.
///
/// The fields are read across the type's bucket and the ones it inherits,
/// nearest first. The envelope keeps what the chain does not declare:
/// extension fields.
fn typed_envelope(
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

    fn message(content: &str, properties: serde_json::Value) -> Node {
        Node::new(
            "ai-chat-message".to_string(),
            content.to_string(),
            properties,
        )
    }

    #[test]
    fn a_native_chat_reads_its_own_bucket_and_the_inherited_one() {
        let node = native(json!({
            "ai-chat": { "agent": "nodespace", "model": "gemma-4-e4b" },
            "ai-chat-native": {
                "provider": "openai-compat",
                "turn_status": "processing",
                "context_tokens": 42,
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
        assert_eq!(chat.base.model, None);
    }

    #[test]
    fn a_flattened_node_reads_the_same_as_a_bucketed_one() {
        let chat = AiChatNativeNode::from_node(native(json!({
            "agent": "nodespace",
            "turn_status": "processing"
        })))
        .unwrap();
        assert_eq!(chat.base.agent, "nodespace");
        assert_eq!(chat.turn_status, AiChatTurnStatus::Processing);
    }

    /// A native chat carries no messages: they are its children.
    #[test]
    fn a_native_chat_has_no_messages_field() {
        let chat = AiChatNativeNode::from_node(native(json!({}))).unwrap();
        let wire = serde_json::to_value(&chat).unwrap();
        assert!(wire.get("messages").is_none());
    }

    #[test]
    fn a_message_reads_its_fields_and_keeps_its_text_as_content() {
        let node = message(
            "Which one?",
            json!({ "ai-chat-message": {
                "role": "assistant",
                "timestamp": "2026-10-02T10:00:00Z",
                "reasoning": "Two tasks match.",
                "outcome": "clarified",
                "options": ["Login bug", "Logout bug"],
                "custom:flag": 1
            } }),
        );
        let message = AiChatMessageNode::from_node(node).unwrap();
        assert_eq!(message.envelope.content, "Which one?");
        assert_eq!(message.role, AiChatMessageRole::Assistant);
        assert_eq!(message.timestamp.as_deref(), Some("2026-10-02T10:00:00Z"));
        assert_eq!(message.reasoning.as_deref(), Some("Two tasks match."));
        assert_eq!(message.outcome, Some(AiChatTurnOutcome::Clarified));
        assert_eq!(message.options, ["Login bug", "Logout bug"]);
        assert_eq!(message.envelope.properties, json!({ "custom:flag": 1 }));
    }

    #[test]
    fn a_message_with_no_fields_is_a_user_message_with_nothing_optional() {
        let message = AiChatMessageNode::from_node(message("hi", json!({}))).unwrap();
        assert_eq!(message.role, AiChatMessageRole::User);
        assert_eq!(message.outcome, None);
        assert!(message.options.is_empty());
        let wire = serde_json::to_value(&message).unwrap();
        assert_eq!(wire["role"], "user");
        for absent in ["timestamp", "reasoning", "outcome", "options"] {
            assert!(wire.get(absent).is_none(), "{absent} is left off the wire");
        }
    }

    /// No validated write stores a value outside a closed enum. One that got
    /// there some other way reads as the field's default, and the node still
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

        let message = AiChatMessageNode::from_node(message(
            "hi",
            json!({ "ai-chat-message": { "role": "tool_call", "outcome": "done" } }),
        ))
        .unwrap();
        assert_eq!(message.role, AiChatMessageRole::default());
        assert_eq!(message.outcome, None);
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
    fn a_typed_struct_is_not_borrowed_by_another_type() {
        let pty = Node::new("ai-chat-pty".to_string(), "S".to_string(), json!({}));
        assert!(AiChatNativeNode::from_node(pty.clone()).is_err());
        let base = Node::new("ai-chat".to_string(), "S".to_string(), json!({}));
        assert!(AiChatNativeNode::from_node(base.clone()).is_err());
        assert!(AiChatPtyNode::from_node(base).is_err());
        assert!(AiChatMessageNode::from_node(pty.clone()).is_err());
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
        for (role, _) in AiChatMessageRole::ALL {
            assert_eq!(serde_json::to_value(role).unwrap(), json!(role.as_str()));
        }
        for (outcome, _) in AiChatTurnOutcome::ALL {
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                json!(outcome.as_str())
            );
        }
    }

    /// The edge shapes are stored with snake_case keys, and an unknown key is
    /// refused, not carried.
    #[test]
    fn a_wrote_edge_round_trips_its_writes_and_refuses_an_unknown_key() {
        let edge = AiChatWroteEdge {
            writes: vec![
                AiChatWrite {
                    seq: 0,
                    tool: "create_node".to_string(),
                    summary: Some("Buy milk".to_string()),
                    canonical_args: r#"{"content":"Buy milk"}"#.to_string(),
                    replaced: Vec::new(),
                },
                AiChatWrite {
                    seq: 1,
                    tool: "create_relationship".to_string(),
                    summary: None,
                    canonical_args: "sha256:abc".to_string(),
                    replaced: vec!["a -[assignee]-> b".to_string()],
                },
            ],
        };
        let stored = serde_json::to_value(&edge).unwrap();
        assert_eq!(
            stored,
            json!({ "writes": [
                {
                    "seq": 0,
                    "tool": "create_node",
                    "summary": "Buy milk",
                    "canonical_args": r#"{"content":"Buy milk"}"#
                },
                {
                    "seq": 1,
                    "tool": "create_relationship",
                    "canonical_args": "sha256:abc",
                    "replaced": ["a -[assignee]-> b"]
                }
            ] })
        );
        assert_eq!(
            serde_json::from_value::<AiChatWroteEdge>(stored).unwrap(),
            edge
        );
        assert!(serde_json::from_value::<AiChatWroteEdge>(json!({
            "writes": [{ "seq": 0, "tool": "t", "canonical_args": "{}", "nodeId": "x" }]
        }))
        .is_err());
        assert!(serde_json::from_value::<AiChatPendingDeleteEdge>(
            json!({ "version": 3, "descendant_count": 0, "count": 1 })
        )
        .is_err());
        assert_eq!(
            serde_json::to_value(AiChatPendingDeleteEdge {
                version: 3,
                descendant_count: 2
            })
            .unwrap(),
            json!({ "version": 3, "descendant_count": 2 })
        );
        assert!(
            serde_json::from_value::<AiChatResolvedEdge>(json!({ "tool": "get_node" })).is_ok()
        );
    }
}
