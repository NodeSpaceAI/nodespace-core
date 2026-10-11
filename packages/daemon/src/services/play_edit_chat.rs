//! The chat a play is edited through (ADR-090 §3).
//!
//! A play's rules are changed by asking for the change in a chat bound to
//! the play. [`create_play_edit_chat`] creates that chat: a native chat that
//! pins the play-authoring skill and the play, opened by a message the system
//! writes. No model is called, so the chat is there at once, whether or not a
//! model is loaded.

use nodespace_agent::skill_pipeline::PLAY_AUTHORING_SKILL_ID;
use nodespace_core::models::{
    AiChatMessageRole, AiChatTurnOutcome, CoreNodeType, Node, PlayFields, AI_CHAT_PINS,
    NODESPACE_AGENT,
};
use nodespace_core::playbook::types::{
    GraphEventType, RuleClass, RuleDefinition, Selector, Trigger,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NewRelationship, NodeService, NodeServiceError,
};

use crate::services::chat_messages::{append_message, display_title, NewMessage};

/// What the opening message offers, in the order shown. The first is not a
/// change: it is there so the user knows the play can be asked about as well
/// as changed (ADR-090 §3).
pub const EDIT_OPTIONS: [&str; 6] = [
    "Explain this play",
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
/// follows: an assistant message that links the play, describes it from
/// what it stores ([`opening_message`]) and asks what to change, with
/// `outcome: clarified` and [`EDIT_OPTIONS`]. The link records a `mentions`
/// edge from the message to the play, so the play's backlinks lead to the
/// chat. A chat whose opening message cannot be written is removed, so the
/// caller gets the whole chat or none.
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
    // A blank value is no value: the schema's default stands.
    let given = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    if let Some(provider) = given(model.provider) {
        properties["provider"] = serde_json::json!(provider);
    }
    if let Some(model) = given(model.model) {
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
    let pinned = [PLAY_AUTHORING_SKILL_ID, play.id.as_str()];
    let pins = pinned
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
        let missing: Vec<&str> = unpinned.iter().map(|index| pinned[*index]).collect();
        tracing::warn!(
            chat_id,
            play_id,
            ?missing,
            "a play's edit chat was created without a pin: its node does not exist"
        );
    }

    let options: Vec<String> = EDIT_OPTIONS.iter().map(|o| o.to_string()).collect();
    let content = opening_message(&title, &play);
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
///
/// No test reaches this: a message create that fails just after its chat's
/// create succeeded needs a store that fails on demand, which the service
/// has no seam for.
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

/// The opening message: a link to the play, what the play does, and the
/// question of what to change.
///
/// The description is built from what the play stores, with no model call:
/// the play's own description, then each rule with what triggers it, what it
/// checks and what it does, in the words of the rule's, conditions' and
/// actions' required descriptions (ADR-090 §1). A trigger has no description
/// of its own, so it is described from its fields ([`describe_trigger`]).
///
/// The message is stored content: it describes the play as it was when the
/// chat was opened, and a play changed later, in this chat or elsewhere, is
/// not described again. The agent reads the play as it is before it answers.
///
/// The link text is the play's title with the characters that would end it
/// removed.
fn opening_message(title: &str, play: &Node) -> String {
    let label: String = title
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\n' | '\r'))
        .collect();
    let label = if label.trim().is_empty() {
        "this play"
    } else {
        label.trim()
    };
    let mut message = format!("Here is what [{label}](nodespace://{}) does.", play.id);

    // A play whose rules do not decode still has a description to show.
    let (fields, rules_readable) = match PlayFields::from_properties(&play.properties) {
        Ok(fields) => (fields, true),
        Err(_) => (
            PlayFields::readable_from_properties(&play.properties),
            false,
        ),
    };
    if let Some(description) = fields.description.as_deref().map(one_line) {
        if !description.is_empty() {
            message.push(' ');
            message.push_str(&description);
        }
    }

    message.push_str("\n\n");
    message.push_str(&match (rules_readable, fields.rules.len()) {
        (false, _) => "Its rules can't be read as they are stored.".to_string(),
        (true, 0) => "It has no rules yet.".to_string(),
        (true, 1) => "It has one rule.".to_string(),
        (true, count) => format!("It has {count} rules."),
    });
    for rule in &fields.rules {
        message.push_str("\n\n");
        message.push_str(&describe_rule(rule));
    }

    message.push_str("\n\nWhat would you like to change?");
    message
}

/// One rule as the opening message shows it: its name and description, then
/// a line each for its trigger, its conditions and its actions.
fn describe_rule(rule: &RuleDefinition) -> String {
    let mut lines = vec![format!(
        "**{}**: {}",
        one_line(&rule.name),
        one_line(&rule.description)
    )];
    let trigger = describe_trigger(&rule.trigger);
    lines.push(match rule.class {
        RuleClass::Reactive => format!("- {trigger}"),
        // An invariant rule runs inside the write that triggered it and can
        // refuse it (ADR-060), which is what a user changing it needs to know.
        RuleClass::Invariant => format!("- {trigger}, before that change is saved"),
    });
    for condition in &rule.conditions {
        lines.push(format!("- Only if: {}", one_line(&condition.description)));
    }
    for action in &rule.actions {
        let step = one_line(action.description());
        lines.push(match action.for_each() {
            Some(collection) => format!("- Then, for each of `{collection}`: {step}"),
            None => format!("- Then: {step}"),
        });
    }
    lines.join("\n")
}

/// A trigger in words, from its fields: it carries no authored description
/// (ADR-090 §1). The wording is the play viewer's (`describeTrigger` in
/// `play-node-model.ts`), so the chat and the view beside it say the same
/// thing about the same rule.
fn describe_trigger(trigger: &Trigger) -> String {
    let noun = selector_noun(trigger.selector());
    match trigger {
        Trigger::Scheduled { cron, .. } => format!("On the schedule {cron}, for every {noun}"),
        Trigger::GraphEvent {
            on, property_key, ..
        } => {
            let target = with_article(&noun);
            match on {
                GraphEventType::NodeCreated => format!("When {target} is created"),
                GraphEventType::PropertyChanged => match property_key {
                    // The field a `<type>.<field>` key names.
                    Some(key) => format!(
                        "When {} changes on {target}",
                        key.rsplit('.').next().unwrap_or(key)
                    ),
                    None => format!("When any property changes on {target}"),
                },
                GraphEventType::RelationshipAdded => {
                    format!("When a relationship is added from {target}")
                }
                GraphEventType::RelationshipRemoved => {
                    format!("When a relationship is removed from {target}")
                }
            }
        }
    }
}

/// The noun a selector's nodes are named by: `task`, `node the saved query
/// selects`.
fn selector_noun(select: &Selector) -> String {
    match select {
        Selector::Query(_) => "node the saved query selects".to_string(),
        Selector::Inline(inline) => {
            let noun = if inline.target_type == "*" {
                "node"
            } else {
                inline.target_type.as_str()
            };
            match inline.filters.len() {
                0 => noun.to_string(),
                1 => format!("{noun} matching 1 filter"),
                count => format!("{noun} matching {count} filters"),
            }
        }
    }
}

fn with_article(noun: &str) -> String {
    let vowel = noun
        .chars()
        .next()
        .is_some_and(|c| "aeiou".contains(c.to_ascii_lowercase()));
    format!("{} {noun}", if vowel { "an" } else { "a" })
}

/// Author text on one line, so it cannot break the list it is shown in.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chat_messages::load_messages;
    use crate::services::chat_pins::load_chat_pins;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::{AiChatNativeNode, SkillFields};
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

    /// A play with `rules`, described as "Closes a parent once its sub-tasks
    /// are done."
    async fn play_with_rules(svc: &NodeService, rules: serde_json::Value) -> String {
        svc.create_node(Node::new(
            "play".to_string(),
            "Close finished parents".to_string(),
            json!({
                "description": "Closes a parent once its sub-tasks are done.",
                "rules": rules,
            }),
        ))
        .await
        .unwrap()
    }

    fn close_parent_rule() -> serde_json::Value {
        json!({
            "name": "close parent",
            "description": "Marks a parent task done when its last sub-task is.",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "task" },
                "property_key": "task.status"
            },
            "conditions": [
                { "expr": "node.status == 'done'", "description": "The task was just finished" },
                {
                    "expr": "node.child_of.has_child.all(status == 'done')",
                    "description": "Every other sub-task of its parent is done"
                }
            ],
            "actions": [{
                "action_type": "update_node",
                "description": "Set the parent's status to done",
                "params": { "node_id": "{trigger.node.child_of.id}", "properties": { "status": "done" } }
            }]
        })
    }

    async fn opening_content(svc: &NodeService, play_id: &str) -> String {
        let chat_id = create_play_edit_chat(svc, play_id, ChatModel::default())
            .await
            .unwrap();
        load_messages(svc, &chat_id)
            .await
            .unwrap()
            .remove(0)
            .content
    }

    /// The opening message describes the play before it asks: the play's
    /// description, then the rule's trigger, conditions and actions in their
    /// authors' words.
    #[tokio::test]
    async fn the_opening_message_describes_a_play_with_one_rule() {
        let (svc, _dir) = test_service().await;
        let play_id = play_with_rules(&svc, json!([close_parent_rule()])).await;

        assert_eq!(
            opening_content(&svc, &play_id).await,
            format!(
                "Here is what [Close finished parents](nodespace://{play_id}) does. \
                 Closes a parent once its sub-tasks are done.\n\
                 \n\
                 It has one rule.\n\
                 \n\
                 **close parent**: Marks a parent task done when its last sub-task is.\n\
                 - When status changes on a task\n\
                 - Only if: The task was just finished\n\
                 - Only if: Every other sub-task of its parent is done\n\
                 - Then: Set the parent's status to done\n\
                 \n\
                 What would you like to change?"
            )
        );
    }

    /// Each rule of a play with several is described, in rule order: a
    /// scheduled trigger, an action that runs over a collection, and an
    /// invariant rule, which says it runs before the change is saved.
    #[test]
    fn the_opening_message_describes_each_rule_of_a_play_with_several() {
        let roll_over = json!({
            "name": "roll over",
            "description": "Moves unfinished tasks to the next cycle.",
            "trigger": {
                "type": "scheduled",
                "cron": "0 5 0 * * * *",
                "select": {
                    "target_type": "cycle",
                    "filters": [
                        { "type": "property", "operator": "equals", "property": "state", "value": "active" }
                    ]
                }
            },
            "actions": [{
                "action_type": "add_relationship",
                "description": "Link the task\nto the next cycle",
                "for_each": "trigger.node.tasks",
                "params": { "source_id": "{item.id}", "relationship_type": "cycle", "target_id": "next" }
            }]
        });
        let guard = json!({
            "name": "no empty epic",
            "description": "Refuses an epic with no owner.",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "epic" } },
            "conditions": [{ "expr": "!has(node.owner)", "description": "The epic has no owner" }],
            "actions": [{
                "action_type": "reject",
                "description": "Refuse the write",
                "params": { "message": "An epic needs an owner" }
            }]
        });
        // Built in memory: the message is made from the rules as stored, and
        // these name types this database does not define.
        let play = Node::new(
            "play".to_string(),
            "Close finished parents".to_string(),
            json!({ "rules": [close_parent_rule(), roll_over, guard] }),
        );

        let content = opening_message("Close finished parents", &play);
        assert!(content.contains("It has 3 rules."), "{content}");
        let rules: Vec<usize> = ["**close parent**", "**roll over**", "**no empty epic**"]
            .iter()
            .map(|name| {
                content
                    .find(name)
                    .unwrap_or_else(|| panic!("{name}: {content}"))
            })
            .collect();
        assert!(rules.is_sorted(), "{content}");
        assert!(
            content.contains(
                "**roll over**: Moves unfinished tasks to the next cycle.\n\
                 - On the schedule 0 5 0 * * * *, for every cycle matching 1 filter\n\
                 - Then, for each of `trigger.node.tasks`: Link the task to the next cycle"
            ),
            "{content}"
        );
        assert!(
            content.contains(
                "**no empty epic**: Refuses an epic with no owner.\n\
                 - When an epic is created, before that change is saved\n\
                 - Only if: The epic has no owner\n\
                 - Then: Refuse the write\n\
                 \n\
                 What would you like to change?"
            ),
            "{content}"
        );
    }

    /// A play with no rules, and one whose stored rules do not decode, still
    /// get an opening message that says so.
    #[tokio::test]
    async fn the_opening_message_says_when_there_are_no_rules_to_describe() {
        let (svc, _dir) = test_service().await;
        let empty = play(&svc, "Close finished parents").await;
        assert_eq!(
            opening_content(&svc, &empty).await,
            format!(
                "Here is what [Close finished parents](nodespace://{empty}) does.\n\
                 \n\
                 It has no rules yet.\n\
                 \n\
                 What would you like to change?"
            )
        );

        let unreadable = Node::new(
            "play".to_string(),
            "Broken".to_string(),
            json!({ "description": "Does something.", "rules": "nope" }),
        );
        let content = opening_message("Broken", &unreadable);
        assert!(
            content
                .contains("does. Does something.\n\nIts rules can't be read as they are stored."),
            "{content}"
        );
    }

    #[test]
    fn a_trigger_is_described_from_its_fields() {
        let described = |trigger: serde_json::Value| {
            describe_trigger(&serde_json::from_value(trigger).unwrap())
        };
        let event = |on: &str, select: serde_json::Value| json!({ "type": "graph_event", "on": on, "select": select });
        assert_eq!(
            described(event("property_changed", json!({ "target_type": "*" }))),
            "When any property changes on a node"
        );
        assert_eq!(
            described(event(
                "relationship_added",
                json!({ "target_type": "invoice" })
            )),
            "When a relationship is added from an invoice"
        );
        assert_eq!(
            described(event("relationship_removed", json!({ "query_id": "q-1" }))),
            "When a relationship is removed from a node the saved query selects"
        );
    }

    #[tokio::test]
    async fn the_opening_message_offers_the_options_explaining_first() {
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
        assert_eq!(opening.options.len(), 6);
        assert_eq!(opening.options[0], "Explain this play");
        assert!(
            opening.content.starts_with(&format!(
                "Here is what [Close finished parents](nodespace://{play_id}) does."
            )),
            "{}",
            opening.content
        );
        assert!(
            opening.content.ends_with("What would you like to change?"),
            "{}",
            opening.content
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

    /// A blank provider or model is none given: the chat gets the default.
    #[tokio::test]
    async fn a_blank_model_is_no_model() {
        let (svc, _dir) = test_service().await;
        let play_id = play(&svc, "Close finished parents").await;

        let chat_id = create_play_edit_chat(
            &svc,
            &play_id,
            ChatModel {
                provider: Some(String::new()),
                model: Some("  ".to_string()),
            },
        )
        .await
        .unwrap();

        let chat =
            AiChatNativeNode::from_node(svc.get_node(&chat_id).await.unwrap().unwrap()).unwrap();
        assert_eq!(chat.base.model, None);
        assert_eq!(chat.provider, Default::default());
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
        let play = Node::new_with_id(
            "p1".to_string(),
            "play".to_string(),
            String::new(),
            json!({ "rules": [] }),
        );
        assert!(opening_message("Close [done] parents", &play)
            .starts_with("Here is what [Close done parents](nodespace://p1) does."));
        assert!(opening_message("  ", &play)
            .starts_with("Here is what [this play](nodespace://p1) does."));
    }
}
