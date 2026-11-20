//! The chat card for a remote-tool confirmation: a message the user answers
//! and that stays in the history as a record of what was asked and answered.
//!
//! [`ChatCardUi`] is the [`ConfirmationUi`] the daemon installs. `present` writes
//! the card as a new `ai-chat-message` (an assistant message, `part_kind` =
//! `confirmation`, status `pending`) at the end of the chat, so it sits after
//! the user's message and before the reply the turn writes when it ends.
//! `resolved` changes its status once to how the question ended. The card is
//! never part of the conversation the model sees: [`chat_messages`] leaves
//! confirmation parts out of the history and out of "whose turn is it".
//!
//! The id and the digest are stored on the card so this device's UI can answer;
//! both fields are device-local, so another device sees the question as a
//! record only and cannot answer it. The status written here is for display:
//! nothing decides a run from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nodespace_core::models::{
    AiChatConfirmationStatus, AiChatMessageNode, AiChatMessageRole, AiChatPartKind, CoreNodeType,
    NodeUpdate,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NodeService, NodeServiceError,
};
use serde_json::json;

use crate::services::remote_confirmation::{ConfirmationAsk, ConfirmationOutcome, ConfirmationUi};

/// Writes and updates the cards of one database.
pub struct ChatCardUi {
    node_service: Arc<NodeService>,
    /// Question id -> the card's node id, for the update when it ends.
    cards: Mutex<HashMap<String, String>>,
}

impl ChatCardUi {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            cards: Mutex::new(HashMap::new()),
        }
    }

    fn status_of(outcome: ConfirmationOutcome) -> AiChatConfirmationStatus {
        match outcome {
            ConfirmationOutcome::Allowed => AiChatConfirmationStatus::Allowed,
            ConfirmationOutcome::Denied => AiChatConfirmationStatus::Denied,
            ConfirmationOutcome::Expired => AiChatConfirmationStatus::Expired,
            ConfirmationOutcome::Cancelled => AiChatConfirmationStatus::Cancelled,
        }
    }

    /// Sets a card's status, once: a card that is no longer pending is left as
    /// it is. Retries on a version conflict (the viewer may write the node).
    async fn set_status(&self, card_id: &str, status: AiChatConfirmationStatus) {
        for _ in 0..3 {
            let Ok(Some(node)) = self.node_service.get_node(card_id).await else {
                return;
            };
            let Ok(card) = AiChatMessageNode::from_node(node.clone()) else {
                return;
            };
            if card.confirmation_status != Some(AiChatConfirmationStatus::Pending) {
                return;
            }
            let update = NodeUpdate::new().with_properties(json!({
                "confirmation_status": status,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }));
            match self
                .node_service
                .update_node(card_id, node.version, update)
                .await
            {
                Ok(_) => return,
                Err(NodeServiceError::VersionConflict { .. }) => continue,
                Err(e) => {
                    tracing::warn!(card_id, error = %e, "a confirmation card's status was not written");
                    return;
                }
            }
        }
    }

    /// Ends every card of `chat_id` that is still pending as cancelled: the
    /// turn stopped without answering them (a restart, a crash).
    pub async fn cancel_pending_in(&self, chat_id: &str) {
        let Ok(children) = self.node_service.get_children(chat_id).await else {
            return;
        };
        for child in children {
            if !CoreNodeType::AiChatMessage.is_exactly(&child.node_type) {
                continue;
            }
            let id = child.id.clone();
            let Ok(card) = AiChatMessageNode::from_node(child) else {
                continue;
            };
            if card.part_kind == Some(AiChatPartKind::Confirmation)
                && card.confirmation_status == Some(AiChatConfirmationStatus::Pending)
            {
                self.set_status(&id, AiChatConfirmationStatus::Cancelled)
                    .await;
            }
        }
    }
}

#[async_trait]
impl ConfirmationUi for ChatCardUi {
    async fn present(&self, ask: &ConfirmationAsk) -> bool {
        let params = CreateNodeParams {
            id: None,
            node_type: CoreNodeType::AiChatMessage.as_str().to_string(),
            // The summary: credentials first, then where the call goes.
            content: ask.summary.clone(),
            parent_id: Some(ask.chat_node_id.clone()),
            position: InsertPositionOwned::End,
            properties: json!({
                "role": AiChatMessageRole::Assistant,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "part_kind": AiChatPartKind::Confirmation,
                "confirmation_id": ask.request_id,
                "confirmation_digest": ask.digest,
                "confirmation_status": AiChatConfirmationStatus::Pending,
                "confirmation_arguments": ask.arguments_json,
                "confirmation_credentials": ask.credentials,
            }),
            lifecycle_status: None,
        };
        match self.node_service.create_node_with_parent(params).await {
            Ok(card_id) => {
                self.cards
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(ask.request_id.clone(), card_id);
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "a confirmation card could not be written; the call is not run");
                false
            }
        }
    }

    async fn resolved(&self, ask: &ConfirmationAsk, outcome: ConfirmationOutcome) {
        let card_id = self
            .cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&ask.request_id);
        if let Some(card_id) = card_id {
            self.set_status(&card_id, Self::status_of(outcome)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chat_messages::{
        append_message, last_message_role, load_messages, NewMessage,
    };
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::{AiChatConfirmationStatus, Node};
    use tempfile::TempDir;

    async fn rig() -> (Arc<NodeService>, String, ChatCardUi, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut store: Arc<SqliteStore> =
            Arc::new(SqliteStore::new(tmp.path().join("t.db")).await.unwrap());
        let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
        let chat = ns
            .create_node(Node::new(
                "ai-chat-native".to_string(),
                "Chat".to_string(),
                json!({"agent": "nodespace", "turn_status": "idle"}),
            ))
            .await
            .unwrap();
        let ui = ChatCardUi::new(ns.clone());
        (ns, chat, ui, tmp)
    }

    fn ask(chat: &str, id: &str) -> ConfirmationAsk {
        ConfirmationAsk {
            request_id: id.to_string(),
            chat_node_id: chat.to_string(),
            turn_id: "turn".into(),
            tool_name: "c_tool".into(),
            kind: "tool-cli".into(),
            summary: "run `git`; credentials in the environment: T from keychain:t".into(),
            arguments_json: r#"{"q":"x"}"#.into(),
            arguments_bytes: 9,
            credentials: vec!["T from keychain:t".into()],
            digest: "d".repeat(64),
        }
    }

    async fn card(ns: &NodeService, id: &str) -> AiChatMessageNode {
        AiChatMessageNode::from_node(ns.get_node(id).await.unwrap().unwrap()).unwrap()
    }

    async fn the_card_of(ns: &NodeService, chat: &str) -> (String, AiChatMessageNode) {
        let all = ns.get_children(chat).await.unwrap();
        let node = all
            .into_iter()
            .find(|n| n.properties.to_string().contains("confirmation"))
            .expect("a card");
        let id = node.id.clone();
        (id, AiChatMessageNode::from_node(node).unwrap())
    }

    #[tokio::test]
    async fn the_card_records_the_question_with_its_credentials_arguments_and_digest() {
        let (ns, chat, ui, _tmp) = rig().await;
        assert!(ui.present(&ask(&chat, "req-1")).await);
        let (_, card) = the_card_of(&ns, &chat).await;
        assert_eq!(card.part_kind, Some(AiChatPartKind::Confirmation));
        assert_eq!(card.role, AiChatMessageRole::Assistant);
        assert_eq!(
            card.confirmation_status,
            Some(AiChatConfirmationStatus::Pending)
        );
        assert_eq!(card.confirmation_id.as_deref(), Some("req-1"));
        assert_eq!(card.confirmation_digest.as_deref(), Some(&*"d".repeat(64)));
        assert_eq!(card.confirmation_arguments.as_deref(), Some(r#"{"q":"x"}"#));
        assert_eq!(card.confirmation_credentials, ["T from keychain:t"]);
        assert!(card.envelope.content.contains("run `git`"));
    }

    #[tokio::test]
    async fn the_status_changes_once_and_stays() {
        let (ns, chat, ui, _tmp) = rig().await;
        let a = ask(&chat, "req-1");
        ui.present(&a).await;
        ui.resolved(&a, ConfirmationOutcome::Allowed).await;
        let (id, card) = the_card_of(&ns, &chat).await;
        assert_eq!(
            card.confirmation_status,
            Some(AiChatConfirmationStatus::Allowed)
        );
        // A later, different ending does not rewrite the record.
        ui.set_status(&id, AiChatConfirmationStatus::Denied).await;
        assert_eq!(
            card_status(&ns, &id).await,
            Some(AiChatConfirmationStatus::Allowed)
        );
    }

    async fn card_status(ns: &NodeService, id: &str) -> Option<AiChatConfirmationStatus> {
        card(ns, id).await.confirmation_status
    }

    #[tokio::test]
    async fn each_ending_is_recorded() {
        for (outcome, status) in [
            (
                ConfirmationOutcome::Denied,
                AiChatConfirmationStatus::Denied,
            ),
            (
                ConfirmationOutcome::Expired,
                AiChatConfirmationStatus::Expired,
            ),
            (
                ConfirmationOutcome::Cancelled,
                AiChatConfirmationStatus::Cancelled,
            ),
        ] {
            let (ns, chat, ui, _tmp) = rig().await;
            let a = ask(&chat, "r");
            ui.present(&a).await;
            ui.resolved(&a, outcome).await;
            let (_, card) = the_card_of(&ns, &chat).await;
            assert_eq!(card.confirmation_status, Some(status));
        }
    }

    #[tokio::test]
    async fn a_card_for_a_chat_that_does_not_exist_is_not_shown() {
        let (_ns, _chat, ui, _tmp) = rig().await;
        assert!(!ui.present(&ask("no-such-chat", "r")).await);
    }

    #[tokio::test]
    async fn the_card_is_after_the_users_message_before_the_reply_and_not_part_of_the_conversation()
    {
        let (ns, chat, ui, _tmp) = rig().await;
        append_message(
            &ns,
            &chat,
            NewMessage::text(AiChatMessageRole::User, "do it"),
        )
        .await
        .unwrap();
        ui.present(&ask(&chat, "r")).await;
        append_message(
            &ns,
            &chat,
            NewMessage::text(AiChatMessageRole::Assistant, "done"),
        )
        .await
        .unwrap();
        let order: Vec<String> = ns
            .get_children(&chat)
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.content)
            .collect();
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], "do it");
        assert!(order[1].contains("run `git`"), "{order:?}");
        assert_eq!(order[2], "done");
        // The model's history and the "whose turn" read leave the card out.
        let history = load_messages(&ns, &chat).await.unwrap();
        assert_eq!(
            history
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            ["do it", "done"]
        );
    }

    #[tokio::test]
    async fn a_pending_card_as_the_last_message_does_not_hide_the_users_message_from_recovery() {
        let (ns, chat, ui, _tmp) = rig().await;
        append_message(
            &ns,
            &chat,
            NewMessage::text(AiChatMessageRole::User, "do it"),
        )
        .await
        .unwrap();
        ui.present(&ask(&chat, "r")).await;
        assert_eq!(
            last_message_role(&ns, &chat).await.unwrap(),
            Some(AiChatMessageRole::User)
        );
    }

    #[tokio::test]
    async fn cancelling_pending_cards_ends_only_the_pending_ones() {
        let (ns, chat, ui, _tmp) = rig().await;
        let done = ask(&chat, "done");
        ui.present(&done).await;
        ui.resolved(&done, ConfirmationOutcome::Allowed).await;
        ui.present(&ask(&chat, "open")).await;
        ui.cancel_pending_in(&chat).await;
        let mut statuses = Vec::new();
        for n in ns.get_children(&chat).await.unwrap() {
            statuses.push(card(&ns, &n.id).await.confirmation_status.unwrap());
        }
        statuses.sort_by_key(|s| s.as_str());
        assert_eq!(
            statuses,
            [
                AiChatConfirmationStatus::Allowed,
                AiChatConfirmationStatus::Cancelled
            ]
        );
    }

    #[tokio::test]
    async fn a_question_asked_through_the_confirmer_is_answered_with_what_the_card_carries() {
        use crate::services::remote_confirmation::{
            Answer, AnswerResult, DaemonCallConfirmer, PendingConfirmations,
        };
        use nodespace_agent::local_agent::remote_tools::{
            confirmation_digest, CallConfirmer, ConfirmationRequest, RemoteToolKind,
        };
        use tokio_util::sync::CancellationToken;
        let (ns, chat, ui, _tmp) = rig().await;
        let pending = PendingConfirmations::default();
        let confirmer = Arc::new(DaemonCallConfirmer::new(
            pending.clone(),
            Arc::new(ui),
            chat.clone(),
            "turn".into(),
            CancellationToken::new(),
            CancellationToken::new(),
            std::time::Duration::from_secs(30),
        ));
        let arguments_json = r#"{"q":"x"}"#.to_string();
        let destination = "run `git`".to_string();
        let request = ConfirmationRequest {
            tool_name: "c_tool".into(),
            kind: RemoteToolKind::Cli,
            digest: confirmation_digest(&destination, &arguments_json),
            destination,
            arguments: json!({"q": "x"}),
            arguments_bytes: arguments_json.len(),
            arguments_json,
            credentials: vec![],
        };
        let task = tokio::spawn({
            let confirmer = confirmer.clone();
            async move { confirmer.confirm(&request).await }
        });
        // The UI reads the card and answers with the id and digest ON IT.
        let mut found = None;
        for _ in 0..200 {
            let children = ns.get_children(&chat).await.unwrap();
            if let Some(n) = children.into_iter().next() {
                found = AiChatMessageNode::from_node(n).ok();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let card = found.expect("the card was written");
        let result = pending.answer(&Answer {
            request_id: card.confirmation_id.clone().unwrap(),
            chat_node_id: chat.clone(),
            digest: card.confirmation_digest.clone().unwrap(),
            allow: true,
        });
        assert_eq!(result, AnswerResult::Accepted);
        assert!(task.await.unwrap());
        let (_, card) = the_card_of(&ns, &chat).await;
        assert_eq!(
            card.confirmation_status,
            Some(AiChatConfirmationStatus::Allowed)
        );
    }
}
