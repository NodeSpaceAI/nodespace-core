//! The chat a node is asked about and changed through (ADR-100 §1, ADR-090 §3).
//!
//! A node of a type that takes part is opened in a chat bound to it.
//! [`open_bound_chat`] creates that chat: a native chat that pins the node and
//! the skill for its type, opened by a message the system writes. No model is
//! called, so the chat is there at once, whether or not a model is loaded.
//!
//! A type takes part by supplying a [`BoundChatType`] (ADR-100 §2): a
//! describer, a skill and the options for a change. It is found through the
//! type registry, so a subtype of a type that takes part takes part too.

use nodespace_agent::skill_pipeline::{
    PLAY_AUTHORING_SKILL_ID, QUERY_BUILDING_SKILL_ID, SCHEMA_CHANGING_SKILL_ID,
};
use nodespace_core::models::{
    AiChatMessageRole, AiChatTurnOutcome, CoreNodeType, Node, PlayFields, QueryFields,
    AI_CHAT_PINS, NODESPACE_AGENT, PLAY_RULES_FIELD,
};
use nodespace_core::playbook::types::{
    GraphEventType, RuleClass, RuleDefinition, Selector, Trigger,
};
use nodespace_core::schema::description::{describe_schema, SchemaDescription};
use nodespace_core::services::query_service::description::describe_query;
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NewRelationship, NodeService, NodeServiceError,
};
use std::future::Future;
use std::pin::Pin;

use crate::services::chat_messages::{append_message, display_title, NewMessage};

/// What a describer returns: the text, once whatever it reads has been read.
type Described<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;

/// What a type supplies to take part in a bound chat (ADR-100 §2).
pub struct BoundChatType {
    /// What one is called in the chat's title, link and first option: `play`.
    pub noun: &'static str,
    /// The skill the chat pins: how to explain a node of the type and how to
    /// change one.
    pub skill_id: &'static str,
    /// The kinds of change offered, in the order shown. The option to explain
    /// is not among them: the mechanism puts it first ([`Self::options`]).
    pub changes: &'static [&'static str],
    /// The node in plain language, from what it stores and with no model
    /// call. It is given the node and the markdown link to it, and returns the
    /// text that goes before the question. It may read the graph for what the
    /// node's row does not hold, such as the type a schema extends.
    pub describe: for<'a> fn(&'a NodeService, &'a Node, &'a str) -> Described<'a>,
}

impl BoundChatType {
    /// What the opening message offers: to explain the node, which is not a
    /// change and is first for every type so a person knows the node can be
    /// asked about as well as changed (ADR-100 §3), then the changes.
    pub fn options(&self) -> Vec<String> {
        std::iter::once(format!("Explain this {}", self.noun))
            .chain(self.changes.iter().map(|change| change.to_string()))
            .collect()
    }
}

/// A play is described from its rules' required descriptions, edited with the
/// play-authoring skill, and changed in the five ways a rule can be
/// (ADR-100 §2, ADR-090 §3).
const PLAY: BoundChatType = BoundChatType {
    noun: "play",
    skill_id: PLAY_AUTHORING_SKILL_ID,
    changes: &[
        "Change when it runs",
        "Change a condition",
        "Change an action",
        "Add a rule",
        "Remove a rule",
    ],
    describe: |_, play, link| Box::pin(async move { describe_play(play, link) }),
};

/// A saved query is described from its definition, asked about and changed
/// with the view-building skill, and changed in four ways (ADR-100 §6).
const QUERY: BoundChatType = BoundChatType {
    noun: "view",
    skill_id: QUERY_BUILDING_SKILL_ID,
    changes: &[
        "Add or change a filter",
        "Change the sorting",
        "Change what it lists",
        "Change how it is shown",
    ],
    describe: |_, query, link| Box::pin(async move { describe_query_node(query, link) }),
};

/// A schema is described from its stored definition, asked about and changed
/// with the type-changing skill, and changed in four ways (ADR-100 §5).
const SCHEMA: BoundChatType = BoundChatType {
    noun: "type",
    skill_id: SCHEMA_CHANGING_SKILL_ID,
    changes: &[
        "Add a field",
        "Change a field",
        "Add a value to a list",
        "Remove a field",
    ],
    describe: describe_schema_node,
};

/// The types that take part, each with what it supplies. A type is matched by
/// what it is or extends, so a subtype takes part with its ancestor's.
const PARTICIPANTS: [(CoreNodeType, &BoundChatType); 3] = [
    (CoreNodeType::Play, &PLAY),
    (CoreNodeType::Query, &QUERY),
    (CoreNodeType::Schema, &SCHEMA),
];

/// What `node_type` supplies, or `None` when it does not take part.
async fn bound_chat_type(
    node_service: &NodeService,
    node_type: &str,
) -> Result<Option<&'static BoundChatType>, NodeServiceError> {
    let Some(core) = node_service.core_type_of(node_type).await? else {
        return Ok(None);
    };
    Ok(PARTICIPANTS
        .iter()
        .find(|(base, _)| core.is_a(*base))
        .map(|(_, supplied)| *supplied))
}

/// The model a new chat starts on, when the client has a default.
#[derive(Debug, Clone, Default)]
pub struct ChatModel {
    /// Where inference runs (`native`, `openai-compat`).
    pub provider: Option<String>,
    /// The model's id.
    pub model: Option<String>,
}

/// Why a bound chat was not created.
#[derive(Debug)]
pub enum BoundChatError {
    NodeNotFound(String),
    /// The node's type supplies no describer, skill and options.
    NotSupported {
        id: String,
        node_type: String,
    },
    Service(NodeServiceError),
}

impl std::fmt::Display for BoundChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeNotFound(id) => write!(f, "node {id} not found"),
            Self::NotSupported { id, node_type } => {
                write!(
                    f,
                    "node {id} is a {node_type}, which cannot be opened in a chat"
                )
            }
            Self::Service(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BoundChatError {}

impl From<NodeServiceError> for BoundChatError {
    fn from(error: NodeServiceError) -> Self {
        Self::Service(error)
    }
}

/// Create the chat bound to `node_id`, and return its id.
///
/// The chat, titled `Edit <node title>`, is created with its pins to the
/// type's skill and the node in one transaction. Its opening message follows:
/// an assistant message that the type's describer writes from what the node
/// stores ([`opening_message`]), then asks what to change, with
/// `outcome: clarified` and the type's options ([`BoundChatType::options`]).
/// The describer links the node, which records a `mentions` edge from the
/// message to it, so the node's backlinks lead to the chat. A chat whose
/// opening message cannot be written is removed, so the caller gets the whole
/// chat or none.
///
/// A database with no skill of the type's still gets the chat, pinned to the
/// node alone: retrieval offers whatever skills it has.
pub async fn open_bound_chat(
    node_service: &NodeService,
    node_id: &str,
    model: ChatModel,
) -> Result<String, BoundChatError> {
    let node = node_service
        .get_node(node_id)
        .await?
        .ok_or_else(|| BoundChatError::NodeNotFound(node_id.to_string()))?;
    let Some(supplied) = bound_chat_type(node_service, &node.node_type).await? else {
        return Err(BoundChatError::NotSupported {
            id: node.id,
            node_type: node.node_type,
        });
    };
    let title = display_title(&node).unwrap_or_else(|| format!("Untitled {}", supplied.noun));

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
    let pinned = [supplied.skill_id, node.id.as_str()];
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
            node_id,
            ?missing,
            "a bound chat was created without a pin: its node does not exist"
        );
    }

    let options = supplied.options();
    let content = opening_message(node_service, supplied, &title, &node).await;
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
            tracing::warn!(chat_id, %error, "could not read a bound chat to remove it");
            return;
        }
    };
    if let Err(error) = node_service.delete_node(chat_id, version).await {
        tracing::warn!(chat_id, %error, "could not remove a bound chat left without its opening message");
    }
}

/// The opening message: what the type's describer says about the node, then
/// the question of what to change.
///
/// The message is stored content: it describes the node as it was when the
/// chat was opened, and a node changed later, in this chat or elsewhere, is
/// not described again. The agent reads the node as it is before it answers.
async fn opening_message(
    node_service: &NodeService,
    supplied: &BoundChatType,
    title: &str,
    node: &Node,
) -> String {
    let link = node_link(supplied.noun, title, &node.id);
    format!(
        "{}\n\nWhat would you like to change?",
        (supplied.describe)(node_service, node, &link).await
    )
}

/// A markdown link to the node, whose text is its title with the characters
/// that would end it removed.
fn node_link(noun: &str, title: &str, id: &str) -> String {
    let label: String = title
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\n' | '\r'))
        .collect();
    let label = if label.trim().is_empty() {
        format!("this {noun}")
    } else {
        label.trim().to_string()
    };
    format!("[{label}](nodespace://{id})")
}

/// A type in words, with no model call: what it is for, whether it is built
/// in, what it extends and each field, rendered from its definition
/// ([`describe_schema`]).
///
/// The fields are the effective set, so a type that extends another lists what
/// it inherits, each naming the type that declares it. A type whose definition
/// cannot be read says so, as a saved query does.
fn describe_schema_node<'a>(
    node_service: &'a NodeService,
    node: &'a Node,
    link: &'a str,
) -> Described<'a> {
    Box::pin(async move {
        let unreadable =
            || format!("Here is {link}. Its definition can't be read as it is stored.");
        let Ok(Some(schema)) = node_service.get_schema_node(&node.id).await else {
            return unreadable();
        };
        let Ok((fields, owners, _chain)) = node_service.resolve_field_owners(&node.id).await else {
            return unreadable();
        };
        // A field a built-in schema declares is fixed, here or on a type that
        // extends it.
        let mut built_in = std::collections::HashSet::new();
        for owner in owners.values() {
            if built_in.contains(owner) {
                continue;
            }
            if let Ok(Some(declaring)) = node_service.get_schema_node(owner).await {
                if declaring.is_core {
                    built_in.insert(owner.clone());
                }
            }
        }
        // What the type is for is its description: the schema node's children.
        let purpose = node_service
            .get_children(&node.id)
            .await
            .unwrap_or_default()
            .iter()
            .map(|child| child.content.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        describe_schema(
            link,
            &SchemaDescription {
                schema: &schema,
                purpose: Some(purpose.as_str()),
                fields: &fields,
                owners: &owners,
                built_in: &built_in,
            },
        )
    })
}

/// A saved query in words, with no model call: what it lists, rendered from
/// its definition ([`describe_query`]).
fn describe_query_node(query: &Node, link: &str) -> String {
    match QueryFields::from_properties(&query.properties) {
        Ok(fields) => format!("Here is what {link} shows. {}", describe_query(&fields)),
        Err(_) => format!("Here is {link}. Its definition can't be read as it is stored."),
    }
}

/// A play in words, with no model call: the play's own description, then each
/// rule with what triggers it, what it checks and what it does, in the words
/// of the rule's, conditions' and actions' required descriptions (ADR-090 §1).
/// A trigger has no description of its own, so it is described from its
/// fields ([`describe_trigger`]).
fn describe_play(play: &Node, link: &str) -> String {
    let mut message = format!("Here is what {link} does.");

    // A play with a field that does not decode still shows the ones that
    // do. Its rules are unreadable only when some are stored and none
    // decoded: another field failing leaves them as readable as they were,
    // and a stored empty list is a play with no rules yet.
    let (fields, rules_readable) = match PlayFields::from_properties(&play.properties) {
        Ok(fields) => (fields, true),
        Err(_) => {
            let fields = PlayFields::readable_from_properties(&play.properties);
            let none_stored = PlayFields::stored_field(&play.properties, PLAY_RULES_FIELD)
                .is_none_or(|rules| rules.as_array().is_some_and(Vec::is_empty));
            let readable = none_stored || !fields.rules.is_empty();
            (fields, readable)
        }
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

    /// The opening message of a play built in memory.
    async fn play_opening(svc: &NodeService, title: &str, play: &Node) -> String {
        opening_message(svc, &PLAY, title, play).await
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

        let chat_id = open_bound_chat(&svc, &play_id, ChatModel::default())
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
        let chat_id = open_bound_chat(svc, play_id, ChatModel::default())
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
    #[tokio::test]
    async fn the_opening_message_describes_each_rule_of_a_play_with_several() {
        let (svc, _dir) = test_service().await;
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

        let content = play_opening(&svc, "Close finished parents", &play).await;
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

        // Another field failing to decode does not make the rules unreadable.
        let bad_switch = Node::new(
            "play".to_string(),
            "Half broken".to_string(),
            json!({ "enabled": "yes", "rules": [close_parent_rule()] }),
        );
        let content = play_opening(&svc, "Half broken", &bad_switch).await;
        assert!(
            content.contains("It has one rule.\n\n**close parent**"),
            "{content}"
        );

        let empty_and_bad_switch = Node::new(
            "play".to_string(),
            "Empty".to_string(),
            json!({ "enabled": "yes", "rules": [] }),
        );
        let content = play_opening(&svc, "Empty", &empty_and_bad_switch).await;
        assert!(content.contains("It has no rules yet."), "{content}");

        let unreadable = Node::new(
            "play".to_string(),
            "Broken".to_string(),
            json!({ "description": "Does something.", "rules": "nope" }),
        );
        let content = play_opening(&svc, "Broken", &unreadable).await;
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

        let chat_id = open_bound_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let messages = load_messages(&svc, &chat_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        let opening = &messages[0];
        assert_eq!(opening.role, AiChatMessageRole::Assistant);
        assert_eq!(opening.outcome, Some(AiChatTurnOutcome::Clarified));
        assert_eq!(
            opening.options,
            [
                "Explain this play",
                "Change when it runs",
                "Change a condition",
                "Change an action",
                "Add a rule",
                "Remove a rule",
            ]
        );
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

        let chat_id = open_bound_chat(&svc, &play_id, ChatModel::default())
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

        let chat_id = open_bound_chat(
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

        let chat_id = open_bound_chat(
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

        let chat_id = open_bound_chat(&svc, &play_id, ChatModel::default())
            .await
            .unwrap();

        let pins = load_chat_pins(&svc, &chat_id).await;
        assert!(pins.skills.is_empty());
        assert_eq!(pins.nodes.len(), 1);
    }

    #[tokio::test]
    async fn only_a_type_that_takes_part_gets_a_bound_chat() {
        let (svc, _dir) = test_service().await;
        let note_id = svc
            .create_node(Node::new(
                "text".to_string(),
                "A note".to_string(),
                json!({}),
            ))
            .await
            .unwrap();

        let refusal = open_bound_chat(&svc, &note_id, ChatModel::default())
            .await
            .unwrap_err();
        assert!(matches!(refusal, BoundChatError::NotSupported { .. }));
        assert_eq!(
            refusal.to_string(),
            format!("node {note_id} is a text, which cannot be opened in a chat")
        );
        // The refusal leaves no chat behind.
        assert!(svc
            .query_nodes_by_type("ai-chat-native", true)
            .await
            .unwrap()
            .is_empty());

        assert!(bound_chat_type(&svc, "play").await.unwrap().is_some());
        assert!(bound_chat_type(&svc, "text").await.unwrap().is_none());
        assert!(bound_chat_type(&svc, "no-such-type")
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            open_bound_chat(&svc, "no-such-node", ChatModel::default()).await,
            Err(BoundChatError::NodeNotFound(_))
        ));
    }

    /// A subtype of a type that takes part takes part with its ancestor's
    /// describer, skill and options, and its nodes get the chat.
    #[tokio::test]
    async fn a_subtype_of_a_type_that_takes_part_takes_part() {
        let (svc, _dir) = test_service().await;
        nodespace_core::schema::handle_create_schema(
            &svc,
            json!({ "name": "Nightly play", "extends": "play", "fields": [] }),
        )
        .await
        .expect("create the play subtype");
        let subtype = bound_chat_type(&svc, "nightly-play").await.unwrap();
        assert!(subtype.is_some_and(|supplied| supplied.noun == "play"));

        let id = svc
            .create_node(Node::new(
                "nightly-play".to_string(),
                "Nightly".to_string(),
                json!({ "rules": [] }),
            ))
            .await
            .unwrap();
        let chat_id = open_bound_chat(&svc, &id, ChatModel::default())
            .await
            .unwrap();

        let opening = load_messages(&svc, &chat_id).await.unwrap().remove(0);
        assert_eq!(opening.options[0], "Explain this play");
        assert!(opening.content.contains(&format!("nodespace://{id}")));
    }

    /// A type supplies a describer, a skill and its changes, and the
    /// mechanism does the rest: the option to explain comes first whatever
    /// the type lists, and the question follows the description.
    #[tokio::test]
    async fn the_mechanism_puts_explaining_first_for_any_type() {
        let (svc, _dir) = test_service().await;
        let schema_like = BoundChatType {
            noun: "type",
            skill_id: "skill-1",
            changes: &["Add a field", "Remove a field"],
            describe: |_, node, link| {
                Box::pin(
                    async move { format!("{link} is a type with {} fields.", node.content.len()) },
                )
            },
        };
        assert_eq!(
            schema_like.options(),
            ["Explain this type", "Add a field", "Remove a field"]
        );

        let node = Node::new_with_id(
            "t1".to_string(),
            "schema".to_string(),
            "Invoice".to_string(),
            json!({}),
        );
        assert_eq!(
            opening_message(&svc, &schema_like, "Invoice", &node).await,
            "[Invoice](nodespace://t1) is a type with 7 fields.\n\nWhat would you like to change?"
        );

        let none = BoundChatType {
            changes: &[],
            ..schema_like
        };
        assert_eq!(none.options(), ["Explain this type"]);
    }

    /// A saved query opens on what it lists, then offers to explain it first.
    #[tokio::test]
    async fn a_saved_query_opens_on_what_it_lists() {
        let (svc, _dir) = test_service().await;
        let query_id = svc
            .create_node(Node::new(
                "query".to_string(),
                "Open tasks".to_string(),
                json!({
                    "target_type": "task",
                    "filters": [{
                        "type": "property", "operator": "equals",
                        "property": "status", "value": "open"
                    }],
                    "sorting": [{ "field": "priority", "direction": "asc" }]
                }),
            ))
            .await
            .unwrap();

        let chat_id = open_bound_chat(&svc, &query_id, ChatModel::default())
            .await
            .unwrap();

        let opening = load_messages(&svc, &chat_id).await.unwrap().remove(0);
        assert_eq!(
            opening.content,
            format!(
                "Here is what [Open tasks](nodespace://{query_id}) shows. \
                 Lists tasks whose status is open. Sorted by priority. Shows at most 50.\n\
                 \n\
                 What would you like to change?"
            )
        );
        assert_eq!(
            opening.options,
            [
                "Explain this view",
                "Add or change a filter",
                "Change the sorting",
                "Change what it lists",
                "Change how it is shown",
            ]
        );
        let pins = load_chat_pins(&svc, &chat_id).await;
        assert_eq!(
            pins.nodes
                .iter()
                .map(|n| n.node_type.as_str())
                .collect::<Vec<_>>(),
            ["query"]
        );
    }

    /// A type the user defined opens on what it is for and each of its fields,
    /// says it is open, and offers to explain it first.
    #[tokio::test]
    async fn a_users_type_opens_on_its_fields_and_says_it_is_open() {
        let (svc, _dir) = test_service().await;
        nodespace_core::schema::handle_create_schema(
            &svc,
            json!({
                "name": "Ticket",
                "description": "A customer support request.",
                "fields": [
                    { "name": "summary", "type": "text", "description": "What went wrong." },
                    { "name": "state", "type": "enum", "coreValues": [
                        { "value": "open", "label": "Open" },
                        { "value": "closed", "label": "Closed" }
                    ] }
                ]
            }),
        )
        .await
        .expect("create the type");

        let chat_id = open_bound_chat(&svc, "ticket", ChatModel::default())
            .await
            .unwrap();

        let chat = svc.get_node(&chat_id).await.unwrap().unwrap();
        assert_eq!(chat.content, "Edit Ticket");
        let opening = load_messages(&svc, &chat_id).await.unwrap().remove(0);
        assert_eq!(
            opening.options,
            [
                "Explain this type",
                "Add a field",
                "Change a field",
                "Add a value to a list",
                "Remove a field",
            ]
        );
        for expected in [
            "Here is what [Ticket](nodespace://ticket) is. A customer support request.",
            "It is your own type, so its fields can be added, changed and removed.",
            "It has 2 fields.",
            "- **summary** (text): What went wrong.",
            "- **state** (enum): Values: open, closed.",
        ] {
            assert!(opening.content.contains(expected), "{}", opening.content);
        }
        assert!(
            opening.content.ends_with("What would you like to change?"),
            "{}",
            opening.content
        );
        let pins = load_chat_pins(&svc, &chat_id).await;
        assert_eq!(
            pins.nodes
                .iter()
                .map(|n| (n.node_id.as_str(), n.node_type.as_str()))
                .collect::<Vec<_>>(),
            [("nodespace://ticket", "schema")]
        );
    }

    /// A built-in type says what it fixes and what a person may add, and a
    /// type that extends it lists what it inherits, naming where from.
    #[tokio::test]
    async fn a_built_in_type_says_it_is_closed_and_a_subtype_names_its_parent() {
        let (svc, _dir) = test_service().await;
        nodespace_core::schema::handle_create_schema(
            &svc,
            json!({ "name": "Bug", "extends": "task", "fields": [
                { "name": "severity", "type": "text" }
            ] }),
        )
        .await
        .expect("create the subtype");

        let task = load_messages(
            &svc,
            &open_bound_chat(&svc, "task", ChatModel::default())
                .await
                .unwrap(),
        )
        .await
        .unwrap()
        .remove(0);
        assert!(
            task.content.contains(
                "It is a built-in type, so the fields it declares are fixed and cannot be \
                 changed or removed."
            ),
            "{}",
            task.content
        );
        assert!(
            task.content.contains("under a prefix such as `custom:`"),
            "{}",
            task.content
        );
        // Every field the type declares is fixed, not only the ones marked so.
        let priority = task
            .content
            .lines()
            .find(|line| line.starts_with("- **priority**"))
            .expect("priority is listed");
        assert!(
            priority.contains("More values can be added.")
                && priority.ends_with("Built in: it cannot be changed or removed."),
            "{priority}"
        );

        let bug = load_messages(
            &svc,
            &open_bound_chat(&svc, "bug", ChatModel::default())
                .await
                .unwrap(),
        )
        .await
        .unwrap()
        .remove(0);
        assert!(
            bug.content.contains("It is your own type"),
            "{}",
            bug.content
        );
        assert!(
            bug.content
                .contains("It extends `task`, so it has that type's fields as well."),
            "{}",
            bug.content
        );
        assert!(
            bug.content.contains("- **severity** (text)"),
            "{}",
            bug.content
        );
        assert!(bug.content.contains("From `task`."), "{}", bug.content);
    }

    /// A title cannot break the link that records the mention.
    #[tokio::test]
    async fn the_link_text_drops_the_characters_that_would_end_it() {
        let (svc, _dir) = test_service().await;
        let play = Node::new_with_id(
            "p1".to_string(),
            "play".to_string(),
            String::new(),
            json!({ "rules": [] }),
        );
        assert!(play_opening(&svc, "Close [done] parents", &play)
            .await
            .starts_with("Here is what [Close done parents](nodespace://p1) does."));
        assert!(play_opening(&svc, "  ", &play)
            .await
            .starts_with("Here is what [this play](nodespace://p1) does."));
    }
}
