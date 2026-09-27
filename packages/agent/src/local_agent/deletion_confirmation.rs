//! Agent deletes wait for an explicit yes from the user.
//!
//! `delete_node` is a hard delete that cascades through the node's subtree
//! (ADR-041), and nothing undoes it. Routing can pick the right skill and the
//! model can still pick the wrong records, so the model is never the one that
//! deletes. Its `delete_node` call resolves the target and is *held*: the turn
//! ends asking the user to confirm, naming every target and how many nested
//! nodes go with them. The next user message is then answered without the
//! model:
//!
//! - an affirmative reply deletes exactly the held ids, at the versions and
//!   subtree sizes the user was shown — any drift aborts the whole delete;
//! - a negative reply deletes nothing;
//! - anything else is an ordinary turn, and the held deletes lapse. A model
//!   that deletes again in that turn is held again.
//!
//! The held records travel on the confirmation message itself
//! ([`AiChatPendingDeletion`]), so only a reply to *that* message can
//! confirm them.

use nodespace_core::models::AiChatPendingDeletion;
use nodespace_core::services::{NodeService, NodeServiceError};
use serde_json::{json, Value};

use crate::agent_types::ToolExecutionRecord;

/// The option that confirms a held delete.
pub const CONFIRM_OPTION: &str = "Yes, delete";

/// The option that declines a held delete.
pub const DECLINE_OPTION: &str = "No, keep";

/// Marks a `delete_node` result as held rather than performed.
const HELD_KEY: &str = "held_for_confirmation";

/// Longest title named in a confirmation before it is clipped.
const TITLE_MAX_CHARS: usize = 80;

/// Whether a tool result is a held delete rather than a completed one.
///
/// A held delete changed nothing, so it must not be recorded as a completed
/// write — the cross-turn guard would otherwise refuse the real delete the
/// user goes on to ask for.
pub fn is_held_deletion(result: &Value) -> bool {
    result.get(HELD_KEY).and_then(Value::as_bool) == Some(true)
}

/// Whether an execution record is a write that actually changed the graph.
pub fn landed_write(record: &ToolExecutionRecord) -> bool {
    !record.is_error
        && super::tools::is_write_tool(&record.name)
        && !is_held_deletion(&record.result)
}

/// Resolve what deleting `node_id` would remove, without removing it.
///
/// `Ok(None)` when the node does not exist.
pub async fn preview_deletion(
    node_service: &NodeService,
    node_id: &str,
) -> Result<Option<AiChatPendingDeletion>, NodeServiceError> {
    let Some(node) = node_service.get_node(node_id).await? else {
        return Ok(None);
    };
    let descendant_count = descendant_count(node_service, node_id).await?;
    let title = node
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| node.content.lines().next().unwrap_or_default());
    Ok(Some(AiChatPendingDeletion {
        node_id: node.id.clone(),
        title: clip_title(title),
        node_type: node.node_type.clone(),
        version: node.version,
        descendant_count,
    }))
}

async fn descendant_count(
    node_service: &NodeService,
    node_id: &str,
) -> Result<u64, NodeServiceError> {
    // `collect_subtree_ids` is the exact set a delete removes — the root
    // included — so counting from it cannot disagree with what goes.
    let ids = node_service
        .store()
        .collect_subtree_ids(node_id)
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to collect subtree: {e}")))?;
    Ok(ids.len().saturating_sub(1) as u64)
}

fn clip_title(title: &str) -> String {
    // A newline would split the confirmation paragraph the chat UI renders
    // above the option chips.
    let flat = title.replace(['\n', '\r'], " ");
    let flat = flat.trim();
    if flat.is_empty() {
        return "(untitled)".to_string();
    }
    if flat.chars().count() > TITLE_MAX_CHARS {
        let head: String = flat.chars().take(TITLE_MAX_CHARS).collect();
        format!("{head}…")
    } else {
        flat.to_string()
    }
}

/// The ids of the nodes `node_id` sits under, nearest first.
///
/// Lets a turn that holds both a node and something nested under it confirm
/// only the outer one: its cascade removes the inner one, and counting both
/// would overstate what goes.
pub async fn ancestor_ids(
    node_service: &NodeService,
    node_id: &str,
) -> Result<Vec<String>, NodeServiceError> {
    let mut ancestors = Vec::new();
    let mut current = node_id.to_string();
    // Bounded like the subtree walk, so a malformed hierarchy cannot loop.
    while ancestors.len() < MAX_ANCESTOR_DEPTH {
        let Some(parent) = node_service.get_parent(&current).await? else {
            break;
        };
        current = parent.id.clone();
        ancestors.push(parent.id);
    }
    Ok(ancestors)
}

/// How far up [`ancestor_ids`] walks — the same depth cap as the store's
/// subtree traversal.
const MAX_ANCESTOR_DEPTH: usize = 100;

/// The tool result a held `delete_node` returns to the model.
pub fn held_result(pending: &AiChatPendingDeletion, ancestors: &[String]) -> Value {
    json!({
        HELD_KEY: true,
        "id": super::tools::node_uri(&pending.node_id),
        "title": pending.title,
        "node_type": pending.node_type,
        "version": pending.version,
        "descendant_count": pending.descendant_count,
        "ancestor_ids": ancestors,
        "message": "Not deleted yet. The user will be shown this record and asked to confirm; \
                    it is deleted only if they say yes. If the request covers other records, \
                    call delete_node for each of them too; otherwise stop.",
    })
}

fn pending_from_result(result: &Value) -> Option<(AiChatPendingDeletion, Vec<String>)> {
    if !is_held_deletion(result) {
        return None;
    }
    let id = result.get("id")?.as_str()?;
    let pending = AiChatPendingDeletion {
        node_id: id.strip_prefix("nodespace://").unwrap_or(id).to_string(),
        title: result.get("title")?.as_str()?.to_string(),
        node_type: result.get("node_type")?.as_str()?.to_string(),
        version: result.get("version")?.as_i64()?,
        descendant_count: result.get("descendant_count")?.as_u64()?,
    };
    let ancestors = result
        .get("ancestor_ids")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Some((pending, ancestors))
}

/// Every delete this turn held, in call order, once per node.
///
/// A target nested under another held target is left out: the outer target's
/// cascade removes it, and its own subtree is already inside the outer
/// target's descendant count.
pub fn held_deletions(executions: &[ToolExecutionRecord]) -> Vec<AiChatPendingDeletion> {
    let mut held: Vec<(AiChatPendingDeletion, Vec<String>)> = Vec::new();
    for (pending, ancestors) in executions
        .iter()
        .filter(|r| !r.is_error)
        .filter_map(|r| pending_from_result(&r.result))
    {
        if !held.iter().any(|(p, _)| p.node_id == pending.node_id) {
            held.push((pending, ancestors));
        }
    }
    let ids: Vec<String> = held.iter().map(|(p, _)| p.node_id.clone()).collect();
    held.into_iter()
        .filter(|(_, ancestors)| !ancestors.iter().any(|a| ids.contains(a)))
        .map(|(pending, _)| pending)
        .collect()
}

fn named(p: &AiChatPendingDeletion) -> String {
    format!("\"{}\" ({})", p.title, p.node_type)
}

fn nested(count: u64) -> String {
    if count == 1 {
        "1 item nested".to_string()
    } else {
        format!("{count} items nested")
    }
}

/// The confirmation question: every target, and the nested nodes the delete
/// cascades to. One paragraph, because the chat UI renders only the first
/// paragraph above the option chips.
pub fn confirmation_question(targets: &[AiChatPendingDeletion]) -> String {
    let nested_total: u64 = targets.iter().map(|t| t.descendant_count).sum();
    match targets {
        [only] if nested_total == 0 => {
            format!("Delete {}? This can't be undone.", named(only))
        }
        [only] => format!(
            "Delete {} and the {} under it? This can't be undone.",
            named(only),
            nested(nested_total)
        ),
        _ => {
            let list = targets.iter().map(named).collect::<Vec<_>>().join(", ");
            let cascade = if nested_total == 0 {
                String::new()
            } else {
                format!(" This also removes {} under them.", nested(nested_total))
            };
            format!(
                "Delete these {} records? {list}.{cascade} This can't be undone.",
                targets.len()
            )
        }
    }
}

/// The confirmation as chat text: the question, then the options as bullets
/// for readers that do not render option chips.
pub fn confirmation_text(question: &str) -> String {
    format!("{question}\n\n- {CONFIRM_OPTION}\n- {DECLINE_OPTION}")
}

/// How a user message answers a pending confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationReply {
    /// Delete the held records.
    Confirm,
    /// Delete nothing.
    Decline,
    /// Not an answer to the question — an ordinary message. Nothing held is
    /// deleted.
    Other,
}

/// Classify a reply to a pending confirmation.
///
/// Deliberately a whole-message match: "yes, but not the second one" is not a
/// yes to the list as shown, so it goes to the model as an ordinary turn, and
/// only an unqualified yes deletes.
pub fn classify_reply(message: &str) -> ConfirmationReply {
    let normalized: String = message
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    const CONFIRM: &[&str] = &[
        "yes delete",
        "yes",
        "y",
        "yes please",
        "yep",
        "yeah",
        "confirm",
        "confirmed",
        "delete",
        "delete it",
        "delete them",
        "yes delete it",
        "yes delete them",
        "go ahead",
        "do it",
    ];
    const DECLINE: &[&str] = &[
        "no keep",
        "no",
        "n",
        "nope",
        "no thanks",
        "keep",
        "keep it",
        "keep them",
        "no keep it",
        "no keep them",
        "cancel",
        "don't",
        "dont",
        "don't delete",
        "dont delete",
        "stop",
        "never mind",
        "nevermind",
    ];
    if CONFIRM.contains(&normalized.as_str()) {
        ConfirmationReply::Confirm
    } else if DECLINE.contains(&normalized.as_str()) {
        ConfirmationReply::Decline
    } else {
        ConfirmationReply::Other
    }
}

/// What running a confirmed delete did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedDeletion {
    /// Targets deleted, with how many nodes each removed (itself included).
    pub deleted: Vec<(AiChatPendingDeletion, u64)>,
    /// Targets already gone when the delete ran.
    pub already_gone: Vec<AiChatPendingDeletion>,
    /// Why the delete stopped short, when it did. Targets not listed in
    /// `deleted` or `already_gone` were left in place.
    pub stopped: Option<DeletionStop>,
}

/// Why a confirmed delete stopped short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionStop {
    /// A target no longer matches what was shown — asking again shows the
    /// current state.
    Changed(String),
    /// The delete could not run; asking again would not help.
    Failed(String),
}

impl std::fmt::Display for DeletionStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed(reason) | Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Delete exactly the confirmed targets.
///
/// Every target is checked before anything is deleted: a version or subtree
/// size that differs from what the user was shown aborts the whole delete, so
/// nothing the user did not see is removed. Each delete then runs under the
/// version the user confirmed, so a change that races the check still aborts
/// rather than deleting the newer node.
pub async fn execute_confirmed(
    node_service: &NodeService,
    targets: &[AiChatPendingDeletion],
) -> ConfirmedDeletion {
    let mut outcome = ConfirmedDeletion {
        deleted: Vec::new(),
        already_gone: Vec::new(),
        stopped: None,
    };

    let mut present = Vec::new();
    for target in targets {
        match preview_deletion(node_service, &target.node_id).await {
            Ok(None) => outcome.already_gone.push(target.clone()),
            Ok(Some(now)) if now.version != target.version => {
                outcome.stopped = Some(DeletionStop::Changed(format!(
                    "{} changed after you were asked",
                    named(target)
                )));
                return outcome;
            }
            Ok(Some(now)) if now.descendant_count != target.descendant_count => {
                outcome.stopped = Some(DeletionStop::Changed(format!(
                    "what is nested under {} changed after you were asked",
                    named(target)
                )));
                return outcome;
            }
            Ok(Some(_)) => present.push(target),
            Err(e) => {
                outcome.stopped = Some(DeletionStop::Failed(format!(
                    "{} could not be checked: {e}",
                    named(target)
                )));
                return outcome;
            }
        }
    }

    for target in present {
        match node_service
            .delete_node(&target.node_id, target.version)
            .await
        {
            Ok(r) if r.existed => outcome.deleted.push((target.clone(), r.deleted_count)),
            // Removed by an earlier target's cascade — one target nested
            // under another.
            Ok(_) => outcome.already_gone.push(target.clone()),
            // A version conflict here is a change that raced the check.
            Err(e @ NodeServiceError::VersionConflict { .. }) => {
                outcome.stopped = Some(DeletionStop::Changed(format!(
                    "{} changed after you were asked ({e})",
                    named(target)
                )));
                return outcome;
            }
            Err(e) => {
                outcome.stopped = Some(DeletionStop::Failed(format!(
                    "deleting {} failed: {e}",
                    named(target)
                )));
                return outcome;
            }
        }
    }
    outcome
}

/// The reply reporting a confirmed delete.
pub fn confirmed_deletion_text(outcome: &ConfirmedDeletion) -> String {
    let mut parts = Vec::new();
    if !outcome.deleted.is_empty() {
        let list = outcome
            .deleted
            .iter()
            .map(|(t, _)| named(t))
            .collect::<Vec<_>>()
            .join(", ");
        let nested_total: u64 = outcome
            .deleted
            .iter()
            .map(|(_, n)| n.saturating_sub(1))
            .sum();
        if nested_total == 0 {
            parts.push(format!("Deleted {list}."));
        } else {
            parts.push(format!(
                "Deleted {list}, along with {} under them.",
                nested(nested_total)
            ));
        }
    }
    if !outcome.already_gone.is_empty() {
        let list = outcome
            .already_gone
            .iter()
            .map(named)
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("Already gone: {list}."));
    }
    if let Some(reason) = &outcome.stopped {
        if outcome.deleted.is_empty() {
            parts.push(format!(
                "Nothing was deleted: {reason}. Ask again to see what would be deleted now."
            ));
        } else {
            parts.push(format!(
                "Stopped there: {reason}. The remaining records were left in place."
            ));
        }
    }
    if parts.is_empty() {
        parts.push("Nothing was deleted.".to_string());
    }
    parts.join(" ")
}

/// The reply to a declined confirmation.
pub const DECLINED_TEXT: &str = "OK — nothing was deleted.";

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, title: &str, descendants: u64) -> AiChatPendingDeletion {
        AiChatPendingDeletion {
            node_id: id.to_string(),
            title: title.to_string(),
            node_type: "task".to_string(),
            version: 1,
            descendant_count: descendants,
        }
    }

    fn held(id: &str, title: &str) -> ToolExecutionRecord {
        held_under(id, title, &[])
    }

    fn held_under(id: &str, title: &str, ancestors: &[&str]) -> ToolExecutionRecord {
        let ancestors: Vec<String> = ancestors.iter().map(|a| a.to_string()).collect();
        ToolExecutionRecord {
            tool_call_id: "c".into(),
            name: "delete_node".into(),
            args: json!({ "id": id }),
            result: held_result(&target(id, title, 0), &ancestors),
            is_error: false,
            duration_ms: 0,
        }
    }

    #[test]
    fn a_target_nested_under_another_held_target_is_left_out() {
        let got = held_deletions(&[
            held_under("step", "Step", &["plan"]),
            held("plan", "Plan"),
            held_under("other", "Other", &["unrelated"]),
        ]);
        let ids: Vec<_> = got.iter().map(|p| p.node_id.as_str()).collect();
        assert_eq!(ids, vec!["plan", "other"]);
    }

    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::NodeUpdate;
    use nodespace_core::services::{CreateNodeParams, InsertPositionOwned};
    use std::sync::Arc;

    async fn service() -> (Arc<NodeService>, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut store: Arc<SqliteStore> =
            Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
        (Arc::new(NodeService::new(&mut store).await.unwrap()), tmp)
    }

    async fn create(ns: &NodeService, content: &str, parent: Option<&str>) -> String {
        ns.create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: content.to_string(),
            parent_id: parent.map(str::to_string),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await
        .unwrap()
    }

    async fn exists(ns: &NodeService, id: &str) -> bool {
        ns.get_node(id).await.unwrap().is_some()
    }

    /// A plan with two children, and an unrelated note.
    async fn fixture(ns: &NodeService) -> (String, String) {
        let plan = create(ns, "Plan", None).await;
        let step = create(ns, "Step one", Some(&plan)).await;
        create(ns, "Step one detail", Some(&step)).await;
        let note = create(ns, "Note", None).await;
        (plan, note)
    }

    #[tokio::test]
    async fn preview_counts_the_subtree_and_deletes_nothing() {
        let (ns, _tmp) = service().await;
        let (plan, _) = fixture(&ns).await;

        let pending = preview_deletion(&ns, &plan).await.unwrap().unwrap();

        assert_eq!(pending.title, "Plan");
        assert_eq!(pending.descendant_count, 2);
        assert!(exists(&ns, &plan).await);
        assert!(preview_deletion(&ns, "missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn ancestor_ids_walk_up_to_the_root_nearest_first() {
        let (ns, _tmp) = service().await;
        let (plan, _) = fixture(&ns).await;
        let step = ns.get_children(&plan).await.unwrap().remove(0).id;
        let detail = ns.get_children(&step).await.unwrap().remove(0).id;

        assert_eq!(
            ancestor_ids(&ns, &detail).await.unwrap(),
            vec![step, plan.clone()]
        );
        assert!(ancestor_ids(&ns, &plan).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_confirmed_delete_removes_exactly_the_confirmed_targets() {
        let (ns, _tmp) = service().await;
        let (plan, note) = fixture(&ns).await;
        let target = preview_deletion(&ns, &plan).await.unwrap().unwrap();

        let outcome = execute_confirmed(&ns, std::slice::from_ref(&target)).await;

        assert_eq!(outcome.deleted, vec![(target, 3)]);
        assert!(outcome.stopped.is_none());
        assert!(!exists(&ns, &plan).await);
        assert!(exists(&ns, &note).await, "an unconfirmed node must survive");
    }

    #[tokio::test]
    async fn a_version_change_after_the_question_aborts_every_delete() {
        let (ns, _tmp) = service().await;
        let (plan, note) = fixture(&ns).await;
        let targets = vec![
            preview_deletion(&ns, &note).await.unwrap().unwrap(),
            preview_deletion(&ns, &plan).await.unwrap().unwrap(),
        ];
        let current = ns.get_node(&plan).await.unwrap().unwrap();
        ns.update_node(
            &plan,
            current.version,
            NodeUpdate::new().with_content("Plan (edited)".to_string()),
        )
        .await
        .unwrap();

        let outcome = execute_confirmed(&ns, &targets).await;

        assert!(outcome.deleted.is_empty());
        assert!(matches!(outcome.stopped, Some(DeletionStop::Changed(_))));
        assert!(exists(&ns, &plan).await);
        assert!(
            exists(&ns, &note).await,
            "the check runs before any delete, so an earlier target survives too"
        );
    }

    #[tokio::test]
    async fn a_node_added_under_a_target_after_the_question_aborts_the_delete() {
        let (ns, _tmp) = service().await;
        let (plan, _) = fixture(&ns).await;
        let target = preview_deletion(&ns, &plan).await.unwrap().unwrap();
        let unseen = create(&ns, "Added later", Some(&plan)).await;

        let outcome = execute_confirmed(&ns, &[target]).await;

        assert!(outcome.deleted.is_empty());
        assert!(exists(&ns, &unseen).await);
    }

    #[tokio::test]
    async fn a_target_nested_under_another_is_reported_as_already_gone() {
        let (ns, _tmp) = service().await;
        let (plan, _) = fixture(&ns).await;
        let step = ns.get_children(&plan).await.unwrap().remove(0).id;
        let targets = vec![
            preview_deletion(&ns, &plan).await.unwrap().unwrap(),
            preview_deletion(&ns, &step).await.unwrap().unwrap(),
        ];

        let outcome = execute_confirmed(&ns, &targets).await;

        assert_eq!(outcome.deleted.len(), 1);
        assert_eq!(outcome.already_gone.len(), 1);
        assert!(outcome.stopped.is_none());
    }

    #[test]
    fn held_result_round_trips_and_is_not_a_landed_write() {
        let record = held("n1", "Fix login");
        assert!(is_held_deletion(&record.result));
        assert!(!landed_write(&record));
        assert_eq!(
            held_deletions(&[record]),
            vec![target("n1", "Fix login", 0)]
        );
    }

    #[test]
    fn held_deletions_are_deduped_in_call_order() {
        let got = held_deletions(&[held("a", "A"), held("b", "B"), held("a", "A")]);
        let ids: Vec<_> = got.iter().map(|p| p.node_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn a_held_result_with_a_uri_id_stores_the_bare_id() {
        let mut record = held("n1", "A");
        record.result["id"] = json!("nodespace://n1");
        assert_eq!(held_deletions(&[record])[0].node_id, "n1");
    }

    #[test]
    fn question_names_every_target_and_the_nested_count() {
        assert_eq!(
            confirmation_question(&[target("a", "Fix login", 0)]),
            "Delete \"Fix login\" (task)? This can't be undone."
        );
        assert_eq!(
            confirmation_question(&[target("a", "Plan", 3)]),
            "Delete \"Plan\" (task) and the 3 items nested under it? This can't be undone."
        );
        let q = confirmation_question(&[target("a", "A", 1), target("b", "B", 0)]);
        assert_eq!(
            q,
            "Delete these 2 records? \"A\" (task), \"B\" (task). This also removes 1 item \
             nested under them. This can't be undone."
        );
        assert!(!q.contains("\n\n"));
    }

    #[test]
    fn a_multiline_title_cannot_split_the_question_paragraph() {
        let t = clip_title("first\n\nsecond");
        assert!(!t.contains('\n'));
    }

    #[test]
    fn only_an_unqualified_answer_is_classified() {
        for yes in [
            CONFIRM_OPTION,
            "yes",
            "Yes!",
            "  YES, delete them. ",
            "go ahead",
        ] {
            assert_eq!(classify_reply(yes), ConfirmationReply::Confirm, "{yes}");
        }
        for no in [DECLINE_OPTION, "no", "No.", "cancel", "don't delete"] {
            assert_eq!(classify_reply(no), ConfirmationReply::Decline, "{no}");
        }
        for other in [
            "yes but not the second one",
            "only delete the first",
            "what is nested under it?",
            "",
        ] {
            assert_eq!(classify_reply(other), ConfirmationReply::Other, "{other}");
        }
    }

    #[test]
    fn outcome_text_reports_an_abort_as_nothing_deleted() {
        let text = confirmed_deletion_text(&ConfirmedDeletion {
            deleted: Vec::new(),
            already_gone: Vec::new(),
            stopped: Some(DeletionStop::Changed(
                "\"A\" (task) changed after you were asked".into(),
            )),
        });
        assert!(text.starts_with("Nothing was deleted"), "{text}");
    }

    #[test]
    fn outcome_text_counts_nested_nodes_without_the_targets() {
        let text = confirmed_deletion_text(&ConfirmedDeletion {
            deleted: vec![(target("a", "A", 2), 3), (target("b", "B", 0), 1)],
            already_gone: Vec::new(),
            stopped: None,
        });
        assert_eq!(
            text,
            "Deleted \"A\" (task), \"B\" (task), along with 2 items nested under them."
        );
    }
}
