//! The chat a play is edited through (ADR-090 §3).
//!
//! A play's rules are changed by asking for the change in a chat bound to
//! the play. [`create_play_edit_chat`] creates that chat: a native chat that
//! pins the play-authoring skill and the play, opened by a message the system
//! writes. No model is called, so the chat is there at once, whether or not a
//! model is loaded.

use nodespace_agent::skill_pipeline::PLAY_AUTHORING_SKILL_ID;
use nodespace_core::models::{
    AiChatMessageRole, AiChatTurnOutcome, CoreNodeType, AI_CHAT_PINS, NODESPACE_AGENT,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NewRelationship, NodeService, NodeServiceError,
};

use crate::services::chat_messages::{append_message, display_title, NewMessage};

/// What the opening message offers to change, in the order shown.
pub const EDIT_OPTIONS: [&str; 5] = [
    "Change when it runs",
    "Change a condition",
    "Change an action",
    "Add a rule",
    "Remove a rule",
];

/// The model a new chat starts on, when the client has a default.
#[derive(Debug, Clone, Default)]
pub struct ChatModel {
    /// Where inference runs (`native`, `openai-compat`).
    pub provider: Option<String>,
    /// The model's id.
    pub model: Option<String>,
}

/// Why an edit chat was not created.
#[derive(Debug)]
pub enum PlayEditChatError {
    PlayNotFound(String),
    NotAPlay { id: String, node_type: String },
    Service(NodeServiceError),
}

impl std::fmt::Display for PlayEditChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlayNotFound(id) => write!(f, "play {id} not found"),
            Self::NotAPlay { id, node_type } => {
                write!(f, "node {id} is a {node_type}, not a play")
            }
            Self::Service(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PlayEditChatError {}

impl From<NodeServiceError> for PlayEditChatError {
    fn from(error: NodeServiceError) -> Self {
        Self::Service(error)
    }
}

/// Create the chat that edits `play_id`, and return its id.
///
/// The chat, titled `Edit <play title>`, is created with its pins to the
/// play-authoring skill and the play in one transaction. Its opening message
/// follows: an assistant message that asks what to change and links the
/// play, with `outcome: clarified` and [`EDIT_OPTIONS`]. The link records a
/// `mentions` edge from the message to the play, so the play's backlinks
/// lead to the chat. A chat whose opening message cannot be written is
/// removed, so the caller gets the whole chat or none.
///
/// A database with no play-authoring skill still gets the chat, pinned to
/// the play alone: retrieval offers whatever skills it has.
pub async fn create_play_edit_chat(
    node_service: &NodeService,
    play_id: &str,
    model: ChatModel,
) -> Result<String, PlayEditChatError> {
    let play = node_service
        .get_node(play_id)
        .await?
        .ok_or_else(|| PlayEditChatError::PlayNotFound(play_id.to_string()))?;
    if !node_service
        .type_is_a(&play.node_type, CoreNodeType::Play)
        .await?
    {
        return Err(PlayEditChatError::NotAPlay {
            id: play.id,
            node_type: play.node_type,
        });
    }
    let title = display_title(&play).unwrap_or_else(|| "Untitled play".to_string());

    let mut properties = serde_json::json!({ "agent": NODESPACE_AGENT });
    if let Some(provider) = model.provider {
        properties["provider"] = serde_json::json!(provider);
    }
    if let Some(model) = model.model {
        properties["model"] = serde_json::json!(model);
    }
    let chat = CreateNodeParams {
        id: None,
        node_type: CoreNodeType::AiChatNative.as_str().to_string(),
        content: format!("Edit {title}"),
        parent_id: None,
        position: InsertPositionOwned::End,
        properties,
        lifecycle_status: None,
    };
    let pins = [PLAY_AUTHORING_SKILL_ID, play.id.as_str()]
        .into_iter()
        .map(|target| NewRelationship {
            name: AI_CHAT_PINS.to_string(),
            target_id: target.to_string(),
            edge_data: serde_json::json!({}),
        })
        .collect();

    let (chat_id, unpinned) = node_service
        .create_node_with_relationships(chat, pins)
        .await?;
    if !unpinned.is_empty() {
        tracing::warn!(
            chat_id,
            play_id,
            "a play's edit chat was created without its skill pin: the play-authoring skill is missing"
        );
    }

    let options: Vec<String> = EDIT_OPTIONS.iter().map(|o| o.to_string()).collect();
    let content = opening_message(&title, &play.id);
    let opening = NewMessage {
        outcome: Some(AiChatTurnOutcome::Clarified),
        options: &options,
        ..NewMessage::text(AiChatMessageRole::Assistant, &content)
    };
    if let Err(error) = append_message(node_service, &chat_id, opening).await {
        // A message's parent has to exist before the message is created, so
        // the two are not one transaction. A chat that opens on nothing is
        // not the chat that was asked for: take it back out.
        remove_chat(node_service, &chat_id).await;
        return Err(error.into());
    }
    Ok(chat_id)
}

/// Delete a chat whose opening message could not be written. Best-effort: a
/// failure here leaves an empty chat the user can delete.
async fn remove_chat(node_service: &NodeService, chat_id: &str) {
    let version = match node_service.get_node(chat_id).await {
        Ok(Some(chat)) => chat.version,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(chat_id, %error, "could not read a play edit chat to remove it");
            return;
        }
    };
    if let Err(error) = node_service.delete_node(chat_id, version).await {
        tracing::warn!(chat_id, %error, "could not remove a play edit chat left without its opening message");
    }
}

/// The opening message: what to change, with a link to the play. The link
/// text is the play's title with the characters that would end it removed.
fn opening_message(title: &str, play_id: &str) -> String {
    let label: String = title
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\n' | '\r'))
        .collect();
    let label = if label.trim().is_empty() {
        "this play"
    } else {
        label.trim()
    };
    format!("What would you like to change in [{label}](nodespace://{play_id})?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chat_messages::load_messages;
    use crate::services::chat_pins::load_chat_pins;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::{AiChatNativeNode, Node, SkillFields};
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

    async fn play(svc: &NodeService, title: &str) -> String {
        svc.create_node(Node::new(
            "play".to_string(),
            title.to_string(),
            json!({ "rules": [] }),
        ))
        .await
        .unwrap()
    }

    /// The play-authoring skill, as seeding leaves it.
    async fn seed_authoring_skill(svc: &NodeService) {
        let fields = SkillFields::new("Change a play", &["update_play"], 5);
        svc.create_node(Node::new_with_id(
            PLAY_AUTHORING_SKILL_ID.to_string(),
            "skill".to_string(),
            "Play Authoring".to_string(),
            fields.properties(),
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_edit_chat_is_titled_for_its_play_and_pins_the_skill_and_the_play() {
        let (svc, _dir) = test_service().await;
        seed_authoring_skill(&svc).await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let chat = svc.get_node(&chat_id).await.unwrap().unwrap();
        assert_eq!(chat.content, "Edit Close finished parents");
        let chat = AiChatNativeNode::from_node(chat).expect("a native chat");
        assert_eq!(chat.base.agent, NODESPACE_AGENT);

        let pins = load_chat_pins(&svc, &chat_id).await;
        assert_eq!(
            pins.skills
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            [PLAY_AUTHORING_SKILL_ID]
        );
        assert_eq!(
            pins.nodes
                .iter()
                .map(|n| (n.node_id.as_str(), n.node_type.as_str()))
                .collect::<Vec<_>>(),
            [(format!("nodespace://{play_id}").as_str(), "play")]
        );
    }

    #[tokio::test]
    async fn the_opening_message_asks_what_to_change_and_offers_the_options() {
        let (svc, _dir) = test_service().await;
        seed_authoring_skill(&svc).await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let messages = load_messages(&svc, &chat_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        let opening = &messages[0];
        assert_eq!(opening.role, AiChatMessageRole::Assistant);
        assert_eq!(opening.outcome, Some(AiChatTurnOutcome::Clarified));
        assert_eq!(opening.options, EDIT_OPTIONS);
        assert_eq!(
            opening.content,
            format!(
                "What would you like to change in [Close finished parents](nodespace://{play_id})?"
            )
        );
        assert!(opening.reasoning.is_none());
        assert!(opening.completed_writes.is_empty() && opening.resolved_entities.is_empty());
    }

    /// The opening message's link is a `mentions` edge from the message to
    /// the play, so the play's backlinks list the message, whose parent is
    /// the chat.
    #[tokio::test]
    async fn the_opening_message_mentions_the_play() {
        let (svc, _dir) = test_service().await;
        seed_authoring_skill(&svc).await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let message_id = load_messages(&svc, &chat_id).await.unwrap()[0].id.clone();
        let mentioned = svc
            .store()
            .get_edge_targets_by_source(std::slice::from_ref(&message_id), "mentions")
            .await
            .unwrap();
        assert_eq!(mentioned.get(&message_id), Some(&vec![play_id.clone()]));
    }

    #[tokio::test]
    async fn the_chat_starts_on_the_model_it_is_given() {
        let (svc, _dir) = test_service().await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(
            &svc,
            &play_id,
            ChatModel {
                provider: Some("native".to_string()),
                model: Some("gemma-4-e4b".to_string()),
            },
        )
        .await
        .unwrap();

        let chat =
            AiChatNativeNode::from_node(svc.get_node(&chat_id).await.unwrap().unwrap()).unwrap();
        assert_eq!(chat.base.model.as_deref(), Some("gemma-4-e4b"));
    }

    /// With no play-authoring skill in the database the chat is still
    /// created, bound to the play.
    #[tokio::test]
    async fn a_missing_skill_does_not_cost_the_chat() {
        let (svc, _dir) = test_service().await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let pins = load_chat_pins(&svc, &chat_id).await;
        assert!(pins.skills.is_empty());
        assert_eq!(pins.nodes.len(), 1);
    }

    #[tokio::test]
    async fn only_a_play_gets_an_edit_chat() {
        let (svc, _dir) = test_service().await;
        let note_id = svc
            .create_node(Node::new(
                "text".to_string(),
                "A note".to_string(),
                json!({}),
            ))
            .await
            .unwrap();

        assert!(matches!(
            create_play_edit_chat(&svc, &note_id, ChatModel::default()).await,
            Err(PlayEditChatError::NotAPlay { .. })
        ));
        assert!(matches!(
            create_play_edit_chat(&svc, "no-such-node", ChatModel::default()).await,
            Err(PlayEditChatError::PlayNotFound(_))
        ));
    }

    /// A title cannot break the link that records the mention.
    #[test]
    fn the_link_text_drops_the_characters_that_would_end_it() {
        assert_eq!(
            opening_message("Close [done] parents", "p1"),
            "What would you like to change in [Close done parents](nodespace://p1)?"
        );
        assert_eq!(
            opening_message("  ", "p1"),
            "What would you like to change in [this play](nodespace://p1)?"
        );
    }
}
