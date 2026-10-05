//! What a native chat pins, as a turn uses it (ADR-090 §4, §5).
//!
//! A chat's `pins` edges name the nodes it is bound to. A pinned skill is a
//! Stage-2 candidate on every routed turn; any other pinned node is in every
//! turn's entity list. Both are read from the edges each turn, so a turn sees
//! the nodes as they are now.

use nodespace_agent::agent_types::{ChatMessage, Role, SkillCandidate};
use nodespace_agent::local_agent::tools::pinned_skill_candidates;
use nodespace_core::governance;
use nodespace_core::models::AI_CHAT_PINS;
use nodespace_core::services::NodeService;

use crate::services::chat_messages::{clip_summary, display_title, node_uri};

/// A pinned node that is not a skill: what the conversation is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedNode {
    /// The node, as a `nodespace://` URI.
    pub node_id: String,
    /// The node's name, when it has one.
    pub title: Option<String>,
    /// The node's type.
    pub node_type: String,
}

/// What a chat pins, split by what a turn does with it.
#[derive(Debug, Clone, Default)]
pub struct ChatPins {
    /// The pinned skills, as routing candidates marked `pinned`.
    pub skills: Vec<SkillCandidate>,
    /// Every other pinned node, in the order it was pinned.
    pub nodes: Vec<PinnedNode>,
}

/// Read what `chat_id` pins.
///
/// A pin whose node is gone or no longer participates is left out. A read
/// that fails is logged and yields no pins: the turn then runs as one in a
/// chat that pins nothing, which costs it the binding and not the reply.
pub async fn load_chat_pins(node_service: &NodeService, chat_id: &str) -> ChatPins {
    let targets = match node_service
        .store()
        .get_edge_targets_by_source(&[chat_id.to_string()], AI_CHAT_PINS)
        .await
    {
        Ok(mut targets) => targets.remove(chat_id).unwrap_or_default(),
        Err(error) => {
            tracing::warn!(chat_id, %error, "failed to read an ai-chat's pins");
            return ChatPins::default();
        }
    };
    if targets.is_empty() {
        return ChatPins::default();
    }

    let skills = match pinned_skill_candidates(node_service, &targets).await {
        Ok(skills) => skills,
        Err(error) => {
            tracing::warn!(chat_id, %error, "failed to read an ai-chat's pinned skills");
            Vec::new()
        }
    };

    let mut nodes = Vec::new();
    for id in &targets {
        if skills.iter().any(|skill| &skill.id == id) {
            continue;
        }
        match node_service.get_node(id).await {
            Ok(Some(node)) if governance::participates(&node) => {
                // A skill that is not offered (one that does not decode, say)
                // is not what the conversation is about either.
                if is_skill(node_service, &node.node_type).await {
                    continue;
                }
                nodes.push(PinnedNode {
                    node_id: node_uri(&node.id),
                    title: display_title(&node).map(|title| clip_summary(&title)),
                    node_type: node.node_type,
                });
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(chat_id, pinned = %id, %error, "failed to read a pinned node");
            }
        }
    }

    ChatPins { skills, nodes }
}

async fn is_skill(node_service: &NodeService, node_type: &str) -> bool {
    node_service
        .type_is_a(node_type, nodespace_core::models::CoreNodeType::Skill)
        .await
        .unwrap_or(false)
}

/// The pinned nodes as the turn's record of what the conversation is about.
///
/// Identity and title only, like the record of what an earlier turn looked
/// up: a tool reads the node before acting on it, so nothing here can be
/// stale. Unlike that record it is rebuilt from the pins on every turn and
/// is not bounded by how recently a node was referenced.
///
/// `Role::System`, as that record is: it is the system's own text, which is
/// also what lets a reply link the ids in it.
pub fn pinned_nodes_message(nodes: &[PinnedNode]) -> Option<ChatMessage> {
    if nodes.is_empty() {
        return None;
    }
    let mut lines = String::from(
        "Nodes pinned to this conversation. The conversation is about them: \
         if the user's message refers to one by pronoun or description \
         (\"it\", \"this\", \"the play\"), use the matching id below:\n",
    );
    for node in nodes {
        lines.push_str(&format!("- {}", node.node_id));
        if let Some(title) = &node.title {
            lines.push_str(&format!(" \"{title}\""));
        }
        lines.push_str(&format!(" ({})\n", node.node_type));
    }
    Some(ChatMessage::text(
        Role::System,
        lines.trim_end().to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::Node;
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

    async fn create(
        svc: &NodeService,
        node_type: &str,
        content: &str,
        properties: serde_json::Value,
    ) -> String {
        svc.create_node(Node::new(
            node_type.to_string(),
            content.to_string(),
            properties,
        ))
        .await
        .unwrap()
    }

    async fn chat(svc: &NodeService) -> String {
        create(
            svc,
            "ai-chat-native",
            "A chat",
            json!({ "agent": "nodespace" }),
        )
        .await
    }

    async fn skill(svc: &NodeService, name: &str) -> String {
        let fields = nodespace_core::models::SkillFields::new(
            format!("What {name} is for"),
            &["search_nodes"],
            3,
        );
        create(svc, "skill", name, fields.properties()).await
    }

    async fn pin(svc: &NodeService, chat_id: &str, target: &str) {
        svc.create_relationship(chat_id, AI_CHAT_PINS, target, json!({}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_chat_with_no_pins_has_none() {
        let (svc, _dir) = test_service().await;
        let chat_id = chat(&svc).await;

        let pins = load_chat_pins(&svc, &chat_id).await;

        assert!(pins.skills.is_empty() && pins.nodes.is_empty());
        assert!(pinned_nodes_message(&pins.nodes).is_none());
    }

    /// A pinned skill is a routing candidate; a pinned node of any other type
    /// is an entity. Neither appears as the other.
    #[tokio::test]
    async fn pins_split_into_skills_and_the_nodes_the_chat_is_about() {
        let (svc, _dir) = test_service().await;
        let chat_id = chat(&svc).await;
        let skill_id = skill(&svc, "Authoring").await;
        let note_id = create(&svc, "text", "Release checklist", json!({})).await;
        pin(&svc, &chat_id, &skill_id).await;
        pin(&svc, &chat_id, &note_id).await;

        let pins = load_chat_pins(&svc, &chat_id).await;

        assert_eq!(pins.skills.len(), 1);
        let candidate = &pins.skills[0];
        assert_eq!(candidate.id, skill_id);
        assert_eq!(candidate.name, "Authoring");
        assert_eq!(candidate.tools, ["search_nodes"]);
        assert!(candidate.pinned);

        assert_eq!(
            pins.nodes,
            [PinnedNode {
                node_id: format!("nodespace://{note_id}"),
                title: Some("Release checklist".to_string()),
                node_type: "text".to_string(),
            }]
        );
    }

    /// The record names each pinned node by id, title and type, and nothing
    /// that could go stale.
    #[tokio::test]
    async fn the_pinned_nodes_record_lists_identity_and_title() {
        let record = pinned_nodes_message(&[PinnedNode {
            node_id: "nodespace://p1".to_string(),
            title: Some("Close finished parents".to_string()),
            node_type: "play".to_string(),
        }])
        .expect("a record of the pinned node");

        assert_eq!(record.role, Role::System);
        assert!(
            record
                .content
                .ends_with("- nodespace://p1 \"Close finished parents\" (play)"),
            "{}",
            record.content
        );
    }

    /// A pin goes with its node: deleting the node leaves nothing to list.
    #[tokio::test]
    async fn a_deleted_pinned_node_is_not_listed() {
        let (svc, _dir) = test_service().await;
        let chat_id = chat(&svc).await;
        let note_id = create(&svc, "text", "Gone soon", json!({})).await;
        pin(&svc, &chat_id, &note_id).await;
        let version = svc.get_node(&note_id).await.unwrap().unwrap().version;
        svc.delete_node(&note_id, version).await.unwrap();

        let pins = load_chat_pins(&svc, &chat_id).await;

        assert!(pins.nodes.is_empty());
    }
}
