//! A native chat's conversation, as the agent loop reads and writes it
//! (ADR-088 §3).
//!
//! A message is an `ai-chat-message` node, a child of its chat in conversation
//! order. What a turn did is on edges from its assistant message: `wrote` for
//! each node a write tool call landed on, `resolved` for each node a read
//! surfaced, and `pending_delete` for each delete held until the user answers.
//!
//! [`load_messages`] reads the whole conversation back as [`StoredMessage`]s
//! and [`append_message`] writes one: the node and its edges in a single
//! transaction. Nothing rewrites the conversation.

use std::collections::HashMap;

use nodespace_agent::local_agent::deletion_confirmation::{self, PendingDeletion};
use nodespace_agent::local_agent::tools::{written_node, WrittenNode};
use nodespace_core::models::{
    AiChatMessageNode, AiChatMessageRole, AiChatPendingDeleteEdge, AiChatResolvedEdge,
    AiChatTurnOutcome, AiChatWrite, AiChatWroteEdge, CoreNodeType, Node, AI_CHAT_PENDING_DELETE,
    AI_CHAT_RESOLVED, AI_CHAT_WROTE,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NewRelationship, NodeService, NodeServiceError,
};

/// Longest label kept for a written or looked-up node.
pub(crate) const SUMMARY_MAX_CHARS: usize = 120;

/// Clip an evidence label, marking it when clipped so a truncated summary is
/// not mistaken for a complete one.
pub(crate) fn clip_summary(s: &str) -> String {
    let flat = flatten_label(s);
    if flat.chars().count() > SUMMARY_MAX_CHARS {
        let head: String = flat.chars().take(SUMMARY_MAX_CHARS).collect();
        format!("{head}…")
    } else {
        flat
    }
}

/// Newlines would let user-supplied content shape the evidence block's line
/// structure; a label is a single line by construction.
pub(crate) fn flatten_label(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// A graph write completed during an assistant turn.
///
/// Only successful, state-changing tool calls are recorded. This is the
/// durable evidence that a turn's write actually happened: the agent session
/// is rebuilt from scratch on every turn, so without it the next turn sees the
/// user's original instruction alongside a prose claim of completion and no
/// proof the write occurred, and may repeat it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedWrite {
    /// Name of the tool that performed the write (e.g. `"create_node"`).
    pub tool: String,

    /// The node the tool reported it produced or changed, in the form the
    /// model reads it: a `nodespace://` URI, or a schema's id. `None` for a
    /// write that reports no node of its own (a relationship, a merge, an
    /// import).
    pub node_id: Option<String>,

    /// Short human-readable label for what was written, when available.
    pub summary: Option<String>,

    /// Edges the write evicted as a side effect, each rendered the way
    /// `summary` renders a relationship (`"from -[type]-> to"`).
    pub replaced: Vec<String>,

    /// The call's arguments, canonicalised: with `tool`, the write's identity
    /// for the cross-turn duplicate guard.
    pub canonical_args: String,

    /// Bare id of the node the write's `wrote` edge points at: `node_id`'s
    /// node, or for a write without one the node its record is kept on (the
    /// source of a relationship, the survivor of a merge, the root of an
    /// import). A write with no target is not stored.
    pub target: Option<String>,
}

/// A concrete graph entity a read-only tool call surfaced during an assistant
/// turn: the read-side counterpart of [`CompletedWrite`], so a follow-up like
/// "update that" has an id to resolve "that" against. Identity only: the
/// title and type are read from the node when the conversation is loaded, so
/// they cannot go stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntity {
    /// The node, as a `nodespace://` URI.
    pub node_id: String,

    /// Short human-readable title for the node, when available.
    pub title: Option<String>,

    /// The node's type (e.g. `"task"`), when available.
    pub node_type: Option<String>,

    /// Name of the read tool that surfaced the node.
    pub tool: String,
}

/// One message of a conversation, with what its turn recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    /// The message node's id. Empty for a message not yet stored.
    pub id: String,

    /// Who sent the message.
    pub role: AiChatMessageRole,

    /// Message text. A clarifying question is the text.
    pub content: String,

    /// When the message was created (RFC3339), when known.
    pub timestamp: Option<String>,

    /// Model chain-of-thought reasoning toward the answer, when captured.
    pub reasoning: Option<String>,

    /// Graph writes this assistant turn completed.
    pub completed_writes: Vec<CompletedWrite>,

    /// Graph entities this assistant turn's reads surfaced.
    pub resolved_entities: Vec<ResolvedEntity>,

    /// The choices offered with a clarifying question.
    pub options: Vec<String>,

    /// Deletes this assistant turn is asking the user to confirm. Only the
    /// user's next message can confirm them.
    pub pending_deletions: Vec<PendingDeletion>,

    /// How this assistant turn ended, when an agent turn produced it.
    pub outcome: Option<AiChatTurnOutcome>,
}

impl StoredMessage {
    /// A message with only its role and text.
    pub fn text(role: AiChatMessageRole, content: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            role,
            content: content.into(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            options: Vec::new(),
            pending_deletions: Vec::new(),
            outcome: None,
        }
    }

    /// Whether the message put a composed question to the user: a
    /// clarification, with or without choices, or a delete confirmation.
    pub fn asks(&self) -> bool {
        self.outcome == Some(AiChatTurnOutcome::Clarified)
            || !self.options.is_empty()
            || !self.pending_deletions.is_empty()
    }
}

fn node_uri(id: &str) -> String {
    format!("nodespace://{id}")
}

fn bare_id(id: &str) -> &str {
    id.strip_prefix("nodespace://").unwrap_or(id)
}

/// The bare id of the node a successful write landed on, from the tool's
/// result. See [`CompletedWrite::target`].
pub fn write_target(tool: &str, result: &serde_json::Value) -> Option<String> {
    let key = written_node(tool)?.key();
    result
        .get(key)
        .and_then(|v| v.as_str())
        .map(|id| bare_id(id).to_string())
        .filter(|id| !id.is_empty())
}

/// The messages of a chat, in conversation order. A chat may hold other
/// children; they are not part of the conversation.
async fn message_nodes(
    node_service: &NodeService,
    chat_id: &str,
) -> Result<Vec<AiChatMessageNode>, NodeServiceError> {
    Ok(node_service
        .get_children(chat_id)
        .await?
        .into_iter()
        .filter(|child| CoreNodeType::AiChatMessage.is_exactly(&child.node_type))
        .filter_map(|child| AiChatMessageNode::from_node(child).ok())
        .collect())
}

/// The role of a chat's latest message, or `None` when it has none. One
/// children read, with no edges: what deciding whether a turn is owed needs.
pub async fn last_message_role(
    node_service: &NodeService,
    chat_id: &str,
) -> Result<Option<AiChatMessageRole>, NodeServiceError> {
    Ok(message_nodes(node_service, chat_id)
        .await?
        .last()
        .map(|message| message.role))
}

/// Load a chat's conversation: its message nodes in order, each with the
/// writes, looked-up entities and held deletes its edges record.
///
/// Two reads whatever the conversation's length: the chat's children, and the
/// edges leaving them. An edge whose fields cannot be read is logged and left
/// out; the message and the rest of its records are kept.
pub async fn load_messages(
    node_service: &NodeService,
    chat_id: &str,
) -> Result<Vec<StoredMessage>, NodeServiceError> {
    let nodes = message_nodes(node_service, chat_id).await?;
    let edges = node_service
        .get_child_edges(
            chat_id,
            &[AI_CHAT_WROTE, AI_CHAT_RESOLVED, AI_CHAT_PENDING_DELETE],
        )
        .await?;

    let mut writes: HashMap<String, Vec<(u32, CompletedWrite)>> = HashMap::new();
    let mut resolved: HashMap<String, Vec<ResolvedEntity>> = HashMap::new();
    let mut pending: HashMap<String, Vec<PendingDeletion>> = HashMap::new();

    for (message_id, relationship, target, fields) in edges {
        if relationship == AI_CHAT_WROTE {
            match serde_json::from_value::<AiChatWroteEdge>(fields) {
                Ok(edge) => writes.entry(message_id).or_default().extend(
                    edge.writes
                        .into_iter()
                        .map(|write| (write.seq, completed_write(write, &target))),
                ),
                Err(e) => unreadable_edge(&message_id, &relationship, &target, &e),
            }
        } else if relationship == AI_CHAT_RESOLVED {
            match serde_json::from_value::<AiChatResolvedEdge>(fields) {
                Ok(edge) => resolved
                    .entry(message_id)
                    .or_default()
                    .push(ResolvedEntity {
                        node_id: node_uri(&target.id),
                        title: display_title(&target).map(|t| clip_summary(&t)),
                        node_type: Some(target.node_type.clone()),
                        tool: edge.tool,
                    }),
                Err(e) => unreadable_edge(&message_id, &relationship, &target, &e),
            }
        } else if relationship == AI_CHAT_PENDING_DELETE {
            match serde_json::from_value::<AiChatPendingDeleteEdge>(fields) {
                Ok(edge) => {
                    // The node is named as it is now; the version and the
                    // nested-node count are the ones the user was shown, which
                    // is what a yes is checked against.
                    if let Some(preview) =
                        deletion_confirmation::preview_deletion(node_service, &target.id).await?
                    {
                        pending
                            .entry(message_id)
                            .or_default()
                            .push(PendingDeletion {
                                version: edge.version,
                                descendant_count: edge.descendant_count,
                                ..preview
                            });
                    }
                }
                Err(e) => unreadable_edge(&message_id, &relationship, &target, &e),
            }
        }
    }

    Ok(nodes
        .into_iter()
        .map(|node| {
            let id = node.envelope.id;
            let mut completed = writes.remove(&id).unwrap_or_default();
            completed.sort_by_key(|(seq, _)| *seq);
            StoredMessage {
                completed_writes: completed.into_iter().map(|(_, write)| write).collect(),
                resolved_entities: resolved.remove(&id).unwrap_or_default(),
                pending_deletions: pending.remove(&id).unwrap_or_default(),
                id,
                role: node.role,
                content: node.envelope.content,
                timestamp: node.timestamp,
                reasoning: node.reasoning,
                options: node.options,
                outcome: node.outcome,
            }
        })
        .collect())
}

fn unreadable_edge(message_id: &str, relationship: &str, target: &Node, error: &serde_json::Error) {
    tracing::error!(
        message_id,
        relationship,
        target = %target.id,
        %error,
        "leaving out an ai-chat-message edge whose fields cannot be read"
    );
}

/// A node's name, as a reader would give it: its title, else the first line
/// of its content.
fn display_title(node: &Node) -> Option<String> {
    node.title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| node.content.lines().next().filter(|l| !l.trim().is_empty()))
        .map(str::to_string)
}

/// Rebuild a write's record from its edge. The node the model was shown as
/// the write's own is the edge's target for a tool that reports one, in the
/// form that tool reports it: a schema by its id, any other node by its URI.
fn completed_write(write: AiChatWrite, target: &Node) -> CompletedWrite {
    let node_id = match written_node(&write.tool) {
        Some(WrittenNode::Reported(_)) if CoreNodeType::Schema.is_exactly(&target.node_type) => {
            Some(target.id.clone())
        }
        Some(WrittenNode::Reported(_)) => Some(node_uri(&target.id)),
        Some(WrittenNode::Anchor(_)) | None => None,
    };
    CompletedWrite {
        tool: write.tool,
        node_id,
        summary: write.summary,
        replaced: write.replaced,
        canonical_args: write.canonical_args,
        target: Some(target.id.clone()),
    }
}

/// A message to append to a chat.
#[derive(Debug, Clone)]
pub struct NewMessage<'a> {
    pub role: AiChatMessageRole,
    pub content: &'a str,
    pub reasoning: Option<&'a str>,
    pub outcome: Option<AiChatTurnOutcome>,
    pub options: &'a [String],
    pub completed_writes: &'a [CompletedWrite],
    pub resolved_entities: &'a [ResolvedEntity],
    pub pending_deletions: &'a [PendingDeletion],
}

impl<'a> NewMessage<'a> {
    /// A message with only its role and text.
    pub fn text(role: AiChatMessageRole, content: &'a str) -> Self {
        Self {
            role,
            content,
            reasoning: None,
            outcome: None,
            options: &[],
            completed_writes: &[],
            resolved_entities: &[],
            pending_deletions: &[],
        }
    }
}

/// Append a message to a chat: one node, created as the chat's last child,
/// with the edges recording what its turn did. Returns the message's id.
///
/// The node and its edges are one transaction, so a reply never exists
/// without the record of its writes: that record is what keeps the next turn
/// from repeating them. An edge whose node is already gone is left out (a
/// node deleted later in the same turn has nothing to point at).
///
/// If the edges cannot be written at all, the message is still stored, on its
/// own, and the failure is logged: a reply the user never sees is worse than
/// one whose records are missing.
pub async fn append_message(
    node_service: &NodeService,
    chat_id: &str,
    message: NewMessage<'_>,
) -> Result<String, NodeServiceError> {
    let mut properties = serde_json::json!({
        "role": message.role,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    // Reasoning is kept only when the model produced some.
    if let Some(reasoning) = message.reasoning.filter(|r| !r.trim().is_empty()) {
        properties["reasoning"] = serde_json::json!(reasoning);
    }
    if let Some(outcome) = message.outcome {
        properties["outcome"] = serde_json::json!(outcome);
    }
    if !message.options.is_empty() {
        properties["options"] = serde_json::json!(message.options);
    }
    let params = CreateNodeParams {
        id: None,
        node_type: CoreNodeType::AiChatMessage.as_str().to_string(),
        content: message.content.to_string(),
        parent_id: Some(chat_id.to_string()),
        position: InsertPositionOwned::End,
        properties,
        lifecycle_status: None,
    };

    let relationships = message_edges(&message);
    if relationships.is_empty() {
        return node_service.create_node_with_parent(params).await;
    }

    match node_service
        .create_node_with_relationships(params.clone(), relationships.clone())
        .await
    {
        Ok((message_id, skipped)) => {
            for index in skipped {
                let edge = &relationships[index];
                tracing::debug!(
                    message_id,
                    relationship = %edge.name,
                    target_id = %edge.target_id,
                    "an ai-chat-message edge was not recorded: its node no longer exists"
                );
            }
            Ok(message_id)
        }
        Err(error) => {
            tracing::warn!(
                chat_id,
                %error,
                edges = relationships.len(),
                "an ai-chat message could not be stored with its edges; storing it without \
                 them, so the record of what its turn did is lost"
            );
            node_service.create_node_with_parent(params).await
        }
    }
}

/// The edges a message is created with: what its turn wrote, looked up and
/// asked to delete.
fn message_edges(message: &NewMessage<'_>) -> Vec<NewRelationship> {
    let wrote = wrote_edges(message.completed_writes)
        .into_iter()
        .map(|(target_id, edge)| NewRelationship {
            name: AI_CHAT_WROTE.to_string(),
            target_id,
            edge_data: serde_json::json!(edge),
        });
    // One edge joins a message and a node. The looked-up list is deduplicated
    // by node already; a repeat would be refused as a duplicate edge.
    let resolved = message
        .resolved_entities
        .iter()
        .map(|entity| NewRelationship {
            name: AI_CHAT_RESOLVED.to_string(),
            target_id: bare_id(&entity.node_id).to_string(),
            edge_data: serde_json::json!(AiChatResolvedEdge {
                tool: entity.tool.clone(),
            }),
        });
    let pending = message
        .pending_deletions
        .iter()
        .map(|held| NewRelationship {
            name: AI_CHAT_PENDING_DELETE.to_string(),
            target_id: held.node_id.clone(),
            edge_data: serde_json::json!(AiChatPendingDeleteEdge {
                version: held.version,
                descendant_count: held.descendant_count,
            }),
        });
    wrote.chain(resolved).chain(pending).collect()
}

/// A turn's writes as its `wrote` edges: one per node written, holding that
/// node's writes in call order, each with its position among all of the
/// turn's writes. A write with no node to point at is left out.
fn wrote_edges(writes: &[CompletedWrite]) -> Vec<(String, AiChatWroteEdge)> {
    let mut edges: Vec<(String, AiChatWroteEdge)> = Vec::new();
    for (seq, write) in writes.iter().enumerate() {
        let Some(target) = write.target.as_deref() else {
            tracing::debug!(
                tool = %write.tool,
                "a write with no node to point at is not recorded on the message"
            );
            continue;
        };
        let entry = AiChatWrite {
            seq: seq as u32,
            tool: write.tool.clone(),
            summary: write.summary.clone(),
            canonical_args: write.canonical_args.clone(),
            replaced: write.replaced.clone(),
        };
        match edges.iter_mut().find(|(id, _)| id == target) {
            Some((_, edge)) => edge.writes.push(entry),
            None => edges.push((
                target.to_string(),
                AiChatWroteEdge {
                    writes: vec![entry],
                },
            )),
        }
    }
    edges
}

/// Remove a message's held deletes: the user answered, or moved on. An edge
/// whose node the delete removed went with it and is not an error here.
pub async fn clear_pending_deletions(
    node_service: &NodeService,
    message_id: &str,
    held: &[PendingDeletion],
) {
    for target in held {
        if let Err(error) = node_service
            .delete_relationship(message_id, AI_CHAT_PENDING_DELETE, &target.node_id)
            .await
        {
            tracing::debug!(
                message_id,
                target_id = %target.node_id,
                %error,
                "a held delete's edge was already gone"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::SqliteStore;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_service() -> (Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let mut store = Arc::new(
            SqliteStore::new(temp_dir.path().join("test.db"))
                .await
                .unwrap(),
        );
        let service = Arc::new(NodeService::new(&mut store).await.unwrap());
        (service, temp_dir)
    }

    async fn create(svc: &NodeService, node_type: &str, content: &str) -> String {
        let properties = if node_type == "ai-chat-native" {
            json!({ "agent": "nodespace" })
        } else {
            json!({})
        };
        svc.create_node(Node::new(
            node_type.to_string(),
            content.to_string(),
            properties,
        ))
        .await
        .unwrap()
    }

    fn write(tool: &str, target: Option<&str>, args: &str) -> CompletedWrite {
        CompletedWrite {
            tool: tool.to_string(),
            node_id: None,
            summary: Some(format!("{tool} summary")),
            replaced: Vec::new(),
            canonical_args: args.to_string(),
            target: target.map(str::to_string),
        }
    }

    #[test]
    fn a_writes_target_is_the_node_its_tool_reports() {
        assert_eq!(
            write_target("create_node", &json!({ "id": "nodespace://n1" })).as_deref(),
            Some("n1")
        );
        assert_eq!(
            write_target("create_schema", &json!({ "schemaId": "invoice" })).as_deref(),
            Some("invoice")
        );
        assert_eq!(
            write_target(
                "create_relationship",
                &json!({ "from_id": "nodespace://a", "to_id": "nodespace://b", "type": "t" })
            )
            .as_deref(),
            Some("a")
        );
        assert_eq!(
            write_target("create_nodes_from_markdown", &json!({ "root_id": "r1" })).as_deref(),
            Some("r1")
        );
        assert_eq!(
            write_target(
                "merge_conflict",
                &json!({ "survivor_id": "nodespace://s", "loser_id": "nodespace://l" })
            )
            .as_deref(),
            Some("s")
        );
        // A delete leaves nothing to point at, and a read wrote nothing.
        assert_eq!(write_target("delete_node", &json!({ "id": "n1" })), None);
        assert_eq!(write_target("get_node", &json!({ "id": "n1" })), None);
        assert_eq!(write_target("create_node", &json!({})), None);
    }

    /// Two writes to one node share its edge; their order across the turn's
    /// edges is kept.
    #[test]
    fn a_turns_writes_group_by_node_and_keep_their_order() {
        let edges = wrote_edges(&[
            write("create_node", Some("a"), "{\"n\":1}"),
            write("create_node", Some("b"), "{\"n\":2}"),
            write("create_relationship", Some("a"), "{\"n\":3}"),
            write("dismiss_conflict", None, "{\"n\":4}"),
        ]);
        let shape: Vec<(&str, Vec<(u32, &str)>)> = edges
            .iter()
            .map(|(target, edge)| {
                (
                    target.as_str(),
                    edge.writes
                        .iter()
                        .map(|w| (w.seq, w.tool.as_str()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("a", vec![(0, "create_node"), (2, "create_relationship")]),
                ("b", vec![(1, "create_node")]),
            ]
        );
    }

    #[tokio::test]
    async fn a_message_round_trips_with_what_its_turn_recorded() {
        let (svc, _tmp) = test_service().await;
        let chat = create(&svc, "ai-chat-native", "Untitled").await;
        let task = create(&svc, "task", "Buy milk").await;
        let page = create(&svc, "text", "Shopping").await;

        append_message(
            &svc,
            &chat,
            NewMessage::text(AiChatMessageRole::User, "add a task and link it"),
        )
        .await
        .unwrap();
        let version = svc.get_node(&task).await.unwrap().unwrap().version;
        let reply = append_message(
            &svc,
            &chat,
            NewMessage {
                reasoning: Some("The user wants a task."),
                outcome: Some(AiChatTurnOutcome::Acted),
                options: &["Yes, delete".to_string(), "No, keep".to_string()],
                completed_writes: &[
                    write("create_node", Some(&task), "{\"content\":\"Buy milk\"}"),
                    write("create_node", Some(&page), "{\"content\":\"Shopping\"}"),
                    write("create_relationship", Some(&task), "{\"from\":\"t\"}"),
                ],
                resolved_entities: &[ResolvedEntity {
                    node_id: node_uri(&page),
                    title: None,
                    node_type: None,
                    tool: "search_nodes".to_string(),
                }],
                pending_deletions: &[PendingDeletion {
                    node_id: task.clone(),
                    title: "Buy milk".to_string(),
                    node_type: "task".to_string(),
                    version,
                    descendant_count: 3,
                }],
                ..NewMessage::text(AiChatMessageRole::Assistant, "Delete \"Buy milk\" (task)?")
            },
        )
        .await
        .unwrap();

        let messages = load_messages(&svc, &chat).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, AiChatMessageRole::User);
        assert_eq!(messages[0].content, "add a task and link it");
        assert!(messages[0].timestamp.is_some());
        assert!(messages[0].completed_writes.is_empty());

        let stored = &messages[1];
        assert_eq!(stored.id, reply);
        assert_eq!(stored.role, AiChatMessageRole::Assistant);
        assert_eq!(stored.reasoning.as_deref(), Some("The user wants a task."));
        assert_eq!(stored.outcome, Some(AiChatTurnOutcome::Acted));
        assert_eq!(stored.options, ["Yes, delete", "No, keep"]);
        assert!(stored.asks());

        // The writes come back in call order, across both nodes' edges. A
        // tool that reports its node names it by URI; a relationship names
        // none.
        let writes: Vec<(&str, Option<String>, &str)> = stored
            .completed_writes
            .iter()
            .map(|w| {
                (
                    w.tool.as_str(),
                    w.node_id.clone(),
                    w.canonical_args.as_str(),
                )
            })
            .collect();
        assert_eq!(
            writes,
            [
                (
                    "create_node",
                    Some(node_uri(&task)),
                    "{\"content\":\"Buy milk\"}"
                ),
                (
                    "create_node",
                    Some(node_uri(&page)),
                    "{\"content\":\"Shopping\"}"
                ),
                ("create_relationship", None, "{\"from\":\"t\"}"),
            ]
        );
        assert_eq!(stored.completed_writes[2].target.as_deref(), Some(&*task));

        // A looked-up entity is read from the node as it is now.
        assert_eq!(
            stored.resolved_entities,
            [ResolvedEntity {
                node_id: node_uri(&page),
                title: Some("Shopping".to_string()),
                node_type: Some("text".to_string()),
                tool: "search_nodes".to_string(),
            }]
        );

        // The held delete keeps what the user was shown: the version and the
        // nested-node count, not the ones the node has now.
        assert_eq!(stored.pending_deletions.len(), 1);
        assert_eq!(stored.pending_deletions[0].node_id, task);
        assert_eq!(stored.pending_deletions[0].version, version);
        assert_eq!(stored.pending_deletions[0].descendant_count, 3);
        assert_eq!(stored.pending_deletions[0].title, "Buy milk");

        // Answering removes the edge, and the message no longer asks.
        clear_pending_deletions(&svc, &reply, &stored.pending_deletions).await;
        let messages = load_messages(&svc, &chat).await.unwrap();
        assert!(messages[1].pending_deletions.is_empty());
        // It is still the question it was: its choices stay on it.
        assert!(messages[1].asks());

        assert_eq!(
            last_message_role(&svc, &chat).await.unwrap(),
            Some(AiChatMessageRole::Assistant)
        );
    }

    /// A schema is named to the model by its id, not a URI.
    #[tokio::test]
    async fn a_schema_write_reads_back_under_the_schemas_id() {
        let (svc, _tmp) = test_service().await;
        let chat = create(&svc, "ai-chat-native", "Untitled").await;
        nodespace_core::schema::handle_create_schema(
            &svc,
            json!({ "name": "Invoice", "fields": [] }),
        )
        .await
        .unwrap();

        append_message(
            &svc,
            &chat,
            NewMessage {
                completed_writes: &[write("create_schema", Some("invoice"), "{}")],
                ..NewMessage::text(AiChatMessageRole::Assistant, "Created.")
            },
        )
        .await
        .unwrap();

        let messages = load_messages(&svc, &chat).await.unwrap();
        assert_eq!(
            messages[0].completed_writes[0].node_id.as_deref(),
            Some("invoice")
        );
    }

    /// An edge whose node is gone does not cost the message or its other
    /// records. A chat is a legal target of a message's own edges: a turn that
    /// looked one up records it.
    #[tokio::test]
    async fn an_edge_to_a_missing_node_is_left_out_and_the_rest_are_kept() {
        let (svc, _tmp) = test_service().await;
        let chat = create(&svc, "ai-chat-native", "Untitled").await;
        let other_chat = create(&svc, "ai-chat-native", "Other").await;

        let id = append_message(
            &svc,
            &chat,
            NewMessage {
                completed_writes: &[write("update_node", Some("no-such-node"), "{}")],
                resolved_entities: &[ResolvedEntity {
                    node_id: node_uri(&other_chat),
                    title: None,
                    node_type: None,
                    tool: "get_node".to_string(),
                }],
                ..NewMessage::text(AiChatMessageRole::Assistant, "Done.")
            },
        )
        .await
        .expect("the message is stored");

        let messages = load_messages(&svc, &chat).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, id);
        assert!(messages[0].completed_writes.is_empty());
        let looked_up: Vec<&str> = messages[0]
            .resolved_entities
            .iter()
            .map(|e| e.node_id.as_str())
            .collect();
        assert_eq!(looked_up, [node_uri(&other_chat)]);
    }

    /// A delete of a chat can be held and found again: the edge from the
    /// confirmation to the chat is the message's own record, not a reference
    /// to the chat from outside.
    #[tokio::test]
    async fn a_held_delete_of_a_chat_is_recorded() {
        let (svc, _tmp) = test_service().await;
        let chat = create(&svc, "ai-chat-native", "Untitled").await;
        let doomed = create(&svc, "ai-chat-native", "Old chat").await;
        let held = deletion_confirmation::preview_deletion(&svc, &doomed)
            .await
            .unwrap()
            .unwrap();

        append_message(
            &svc,
            &chat,
            NewMessage {
                outcome: Some(AiChatTurnOutcome::Acted),
                pending_deletions: std::slice::from_ref(&held),
                ..NewMessage::text(AiChatMessageRole::Assistant, "Delete \"Old chat\"?")
            },
        )
        .await
        .unwrap();

        let messages = load_messages(&svc, &chat).await.unwrap();
        assert_eq!(messages[0].pending_deletions, [held]);
    }

    /// A chat's other children are not part of its conversation.
    #[tokio::test]
    async fn only_message_children_are_the_conversation() {
        let (svc, _tmp) = test_service().await;
        let chat = create(&svc, "ai-chat-native", "Untitled").await;
        svc.create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "a note kept under the chat".to_string(),
            parent_id: Some(chat.clone()),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await
        .unwrap();
        append_message(&svc, &chat, NewMessage::text(AiChatMessageRole::User, "hi"))
            .await
            .unwrap();

        let messages = load_messages(&svc, &chat).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hi");
    }
}
