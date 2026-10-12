//! The `issue` and `software-project` core subtypes against a real store and
//! a running engine (ADR-098).
//!
//! A plain task and a plain project carry no development state; an issue and
//! a software project carry it in their own buckets. Whatever is written
//! against `task` works on an issue: a query, the seeded rules, the parent
//! roll-up and the seeded queues, each of which reads an issue's own status
//! as the task status it maps to.

use anyhow::Result;
use nodespace_core::models::{
    node_to_typed_value, CoreNodeType, IssueNodeUpdate, IssueStatus, Node, TaskNodeUpdate,
    TaskStatus,
};
use nodespace_core::ops::query_ops::{
    execute_query_nodes, run_saved_query_nodes, ExecuteQueryInput, RunSavedQueryInput,
};
use nodespace_core::services::query_service::core_queries::{
    AWAITING_REVIEW_QUERY_ID, CHANGES_REQUESTED_QUERY_ID, IN_PROGRESS_QUERY_ID,
    READY_TASKS_QUERY_ID, READY_TO_MERGE_QUERY_ID,
};
use nodespace_core::services::NodeServiceError;
use serde_json::{json, Value};
use std::time::Duration;

use crate::core_model_plays_test::Harness;

const TASK_NEEDS_LINEAGE: &str = "until that plan is approved and the task is linked";
const TASK_SPEC_NOT_APPROVED: &str = "none of its specs is approved";
const TASK_IS_BLOCKED: &str = "blocked by a task that is not finished";
const TASK_HAS_UNCHECKED_ITEM: &str = "its checklist has an unchecked item";
const TASK_NEEDS_CRITERIA: &str = "cannot be marked done without acceptance criteria";

#[track_caller]
fn assert_rejected<T: std::fmt::Debug>(
    result: Result<T, NodeServiceError>,
    fragment: &str,
    why: &str,
) {
    assert!(
        matches!(
            &result,
            Err(NodeServiceError::PlayRuleRejected { message, .. }) if message.contains(fragment)
        ),
        "{why}: expected a refusal saying '{fragment}', got {result:?}"
    );
}

/// The write is refused, and the refusal names `needle`.
#[track_caller]
fn assert_refused<T: std::fmt::Debug>(result: Result<T, NodeServiceError>, needle: &str) {
    let error = match result {
        Ok(value) => panic!("the write must be refused, got {value:?}"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains(needle), "{needle}: {error}");
}

async fn issue(h: &Harness) -> Result<String> {
    h.create("issue", json!({ "status": "open" })).await
}

async fn stored(h: &Harness, id: &str) -> Node {
    h.service.get_node(id).await.unwrap().expect("node exists")
}

async fn typed(h: &Harness, id: &str) -> Value {
    node_to_typed_value(stored(h, id).await).unwrap()
}

/// The ids a query on `target` with `filters` returns, in creation order.
async fn matching(h: &Harness, target: &str, filters: Value) -> Vec<String> {
    let input: ExecuteQueryInput = serde_json::from_value(json!({
        "target_type": target,
        "filters": filters,
        "sorting": [{ "field": "created_at", "direction": "asc" }]
    }))
    .unwrap();
    execute_query_nodes(&h.service, input)
        .await
        .unwrap_or_else(|e| panic!("querying {target} failed: {e}"))
        .into_iter()
        .map(|node| node.id)
        .collect()
}

fn status_is(value: &str) -> Value {
    json!([{ "type": "property", "operator": "equals", "property": "status", "value": value }])
}

async fn saved_query(h: &Harness, query: &str) -> Vec<String> {
    run_saved_query_nodes(
        &h.service,
        RunSavedQueryInput {
            query: query.to_string(),
            filters: Vec::new(),
            limit: None,
            max_rows: None,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("running '{query}' failed: {e}"))
    .nodes
    .into_iter()
    .map(|node| node.id)
    .collect()
}

async fn wait_for_issue_status(h: &Harness, id: &str, status: &str) -> bool {
    for _ in 0..80 {
        if h.field(id, "issue", "status").await.ok().flatten() == Some(json!(status)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Where the fields live
// ---------------------------------------------------------------------------

/// The development fields are the subtypes'. On the base types each is an
/// undeclared key, refused on create and on update, through the generic
/// write the CLI and the agent use.
#[tokio::test]
async fn the_base_types_refuse_the_development_fields() -> Result<()> {
    let h = Harness::start().await?;
    let link = json!({ "title": "PR 7", "url": "https://example.com/pull/7" });

    for (node_type, field, value) in [
        ("task", "pull_request", link.clone()),
        ("task", "commits", json!([link.clone()])),
        ("project", "repository", link.clone()),
        ("project", "checkout_path", json!("/work/core")),
    ] {
        assert_refused(
            h.try_create(node_type, "Named", json!({ field: value.clone() }))
                .await,
            field,
        );
        let id = h.create(node_type, json!({})).await?;
        let before = stored(&h, &id).await;
        assert_refused(h.set(&id, json!({ field: value })).await, field);
        assert_eq!(stored(&h, &id).await.version, before.version, "{field}");
    }

    // A plain task and a plain project travel with no development key.
    let task = h.task().await?;
    let wire = typed(&h, &task).await;
    assert!(wire.get("pullRequest").is_none() && wire.get("commits").is_none());
    let project = h.create("project", json!({})).await?;
    let wire = typed(&h, &project).await;
    assert!(wire.get("repository").is_none() && wire.get("checkoutPath").is_none());
    h.stop().await;
    Ok(())
}

/// An issue's inherited fields are stored where a task's are, and its own
/// (the status it declares again among them) in its own bucket. Both travel
/// at the top level of one typed node.
#[tokio::test]
async fn an_issue_keeps_inherited_fields_in_the_task_bucket_and_its_own_in_its_own() -> Result<()> {
    let h = Harness::start().await?;
    let id = h
        .create(
            "issue",
            json!({
                "status": "in_review",
                "priority": "high",
                "due_date": "2026-11-01",
                "pull_request": { "title": "PR 7", "url": "https://example.com/pull/7" },
                "custom:estimate": 3
            }),
        )
        .await?;

    let node = stored(&h, &id).await;
    assert_eq!(node.node_type, "issue");
    assert_eq!(node.properties["task"]["priority"], "high");
    assert_eq!(node.properties["task"]["due_date"], "2026-11-01");
    assert_eq!(node.properties["issue"]["status"], "in_review");
    assert_eq!(
        node.properties["issue"]["pull_request"]["url"],
        "https://example.com/pull/7"
    );
    assert!(node.properties["task"].get("status").is_none());

    let wire = typed(&h, &id).await;
    assert_eq!(wire["nodeType"], "issue");
    assert_eq!(wire["status"], "in_review");
    assert_eq!(wire["priority"], "high");
    assert_eq!(wire["dueDate"], "2026-11-01");
    assert_eq!(wire["pullRequest"]["title"], "PR 7");
    // Promotion is a move: only the extension field is left.
    assert_eq!(wire["properties"], json!({ "custom:estimate": 3 }));

    // An issue with no status takes the schema default, as a task does.
    let bare = h.create("issue", json!({})).await?;
    assert_eq!(typed(&h, &bare).await["status"], "open");

    // An undeclared key is refused on an issue too: the schema is closed.
    assert_refused(h.set(&id, json!({ "repository": "x" })).await, "repository");
    h.stop().await;
    Ok(())
}

/// A software project's inherited fields are stored where a project's are,
/// and the two that say where its source is in its own bucket.
#[tokio::test]
async fn a_software_project_keeps_its_source_fields_in_its_own_bucket() -> Result<()> {
    let h = Harness::start().await?;
    let id = h
        .create(
            "software-project",
            json!({
                "status": "active",
                "repository": { "title": "core", "url": "https://example.com/acme/core" },
                "checkout_path": "/work/core"
            }),
        )
        .await?;

    let node = stored(&h, &id).await;
    assert_eq!(node.properties["project"]["status"], "active");
    assert_eq!(
        node.properties["software-project"]["checkout_path"],
        "/work/core"
    );
    assert!(node.properties["project"].get("repository").is_none());

    let wire = typed(&h, &id).await;
    assert_eq!(wire["nodeType"], "software-project");
    assert_eq!(wire["status"], "active");
    assert_eq!(wire["repository"]["url"], "https://example.com/acme/core");
    assert_eq!(wire["checkoutPath"], "/work/core");
    assert_eq!(wire["properties"], json!({}));

    // A project's own rule reaches it: a name is required, and the dates
    // must be in order.
    assert!(h
        .try_create("software-project", "  ", json!({}))
        .await
        .is_err());
    assert_refused(
        h.set(
            &id,
            json!({ "start_date": "2026-05-01", "end_date": "2026-04-01" }),
        )
        .await,
        "start_date",
    );
    // It is a project to everything that asks through the registry.
    assert!(
        h.service
            .type_is_a("software-project", CoreNodeType::Project)
            .await?
    );
    assert!(
        !h.service
            .type_is_a("project", CoreNodeType::SoftwareProject)
            .await?
    );

    // A query for projects returns it, and a query for software projects
    // does not return a plain project.
    let plain = h.create("project", json!({ "status": "active" })).await?;
    assert_eq!(
        matching(&h, "project", status_is("active")).await,
        [id.clone(), plain]
    );
    assert_eq!(
        matching(&h, "software-project", status_is("active")).await,
        [id]
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// The status vocabulary
// ---------------------------------------------------------------------------

/// An issue takes its two own statuses; a plain task refuses them, through
/// the typed update and the generic one alike.
#[tokio::test]
async fn an_issue_takes_its_own_statuses_and_a_task_refuses_them() -> Result<()> {
    let h = Harness::start().await?;
    let issue = issue(&h).await?;
    let task = h.task().await?;

    for status in IssueStatus::OWN {
        let node = stored(&h, &issue).await;
        let updated = h
            .service
            .update_issue_node(
                &issue,
                node.version,
                IssueNodeUpdate {
                    status: Some(status.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| panic!("an issue takes {}: {e}", status.as_str()));
        assert_eq!(updated.properties["issue"]["status"], status.as_str());
        assert_eq!(typed(&h, &issue).await["status"], status.as_str());

        let before = stored(&h, &task).await;
        assert_refused(h.set_status(&task, status.as_str()).await, "status");
        assert_refused(
            h.service
                .update_task_node(
                    &task,
                    before.version,
                    TaskNodeUpdate {
                        status: Some(TaskStatus::from_value(status.as_str())),
                        ..Default::default()
                    },
                )
                .await,
            "status",
        );
        assert_eq!(stored(&h, &task).await.version, before.version);
    }

    // The task's statuses are an issue's too, and a value neither declares
    // is refused on both.
    for status in TaskStatus::CORE {
        h.set_status(&issue, status.as_str())
            .await
            .unwrap_or_else(|e| panic!("an issue takes {}: {e}", status.as_str()));
    }
    assert_refused(h.set_status(&issue, "triage").await, "status");
    assert_refused(h.set_status(&task, "triage").await, "status");
    h.stop().await;
    Ok(())
}

/// A typed update is for its own type: a task's refuses an issue and an
/// issue's refuses a task, so a client never writes one through the other's
/// vocabulary.
#[tokio::test]
async fn a_typed_update_refuses_a_node_of_the_other_type() -> Result<()> {
    let h = Harness::start().await?;
    let issue = issue(&h).await?;
    let task = h.task().await?;

    let node = stored(&h, &task).await;
    assert_refused(
        h.service
            .update_issue_node(
                &task,
                node.version,
                IssueNodeUpdate {
                    status: Some(IssueStatus::Done),
                    ..Default::default()
                },
            )
            .await,
        "not an issue node",
    );
    let node = stored(&h, &issue).await;
    assert_refused(
        h.service
            .update_task_node(
                &issue,
                node.version,
                TaskNodeUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await,
        "not a task node",
    );
    // An empty update changes nothing and says so.
    assert!(h
        .service
        .update_issue_node(&issue, node.version, IssueNodeUpdate::default())
        .await
        .is_err());
    // A stale version conflicts.
    let stale = h
        .service
        .update_issue_node(
            &issue,
            node.version + 5,
            IssueNodeUpdate {
                status: Some(IssueStatus::ReadyToMerge),
                ..Default::default()
            },
        )
        .await;
    assert!(
        matches!(stale, Err(NodeServiceError::VersionConflict { .. })),
        "{stale:?}"
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// What a query sees
// ---------------------------------------------------------------------------

/// A query written against `task` matches issues and reads an issue's own
/// status as the task status it maps to. One written against `issue` reads
/// the stored value and the issue's own fields.
#[tokio::test]
async fn a_task_query_reads_an_issue_status_as_the_one_it_maps_to() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.create("task", json!({ "status": "in_progress" })).await?;
    let changes = h
        .create(
            "issue",
            json!({
                "status": "changes_requested",
                "pull_request": { "title": "PR 7", "url": "https://example.com/pull/7" }
            }),
        )
        .await?;
    let ready = h
        .create("issue", json!({ "status": "ready_to_merge" }))
        .await?;
    let reviewing = h.create("issue", json!({ "status": "in_review" })).await?;

    // At `task` scope: `changes_requested` is `in_progress`, and
    // `ready_to_merge` is `in_review`, which the base cannot tell apart.
    assert_eq!(
        matching(&h, "task", status_is("in_progress")).await,
        [task.clone(), changes.clone()]
    );
    assert_eq!(
        matching(&h, "task", status_is("in_review")).await,
        [ready.clone(), reviewing.clone()]
    );
    // The base type has neither of the issue's statuses.
    for own in IssueStatus::OWN {
        assert!(
            matching(&h, "task", status_is(own.as_str()))
                .await
                .is_empty(),
            "{}",
            own.as_str()
        );
    }
    // Nor the issue's own field.
    let has_pull_request =
        json!([{ "type": "property", "operator": "exists", "property": "pull_request" }]);
    assert!(matching(&h, "task", has_pull_request.clone())
        .await
        .is_empty());

    // At `issue` scope the stored value is read, and plain tasks are out.
    assert_eq!(
        matching(&h, "issue", status_is("changes_requested")).await,
        std::slice::from_ref(&changes)
    );
    assert_eq!(
        matching(&h, "issue", status_is("ready_to_merge")).await,
        std::slice::from_ref(&ready)
    );
    assert_eq!(
        matching(&h, "issue", status_is("in_review")).await,
        std::slice::from_ref(&reviewing)
    );
    assert!(matching(&h, "issue", status_is("in_progress"))
        .await
        .is_empty());
    assert_eq!(
        matching(&h, "issue", has_pull_request).await,
        std::slice::from_ref(&changes)
    );

    // Returned at `task` scope, an issue carries the task's fields only and
    // the task's value.
    let input: ExecuteQueryInput = serde_json::from_value(json!({
        "target_type": "task",
        "filters": status_is("in_progress"),
        "sorting": [{ "field": "created_at", "direction": "asc" }]
    }))?;
    let nodes = execute_query_nodes(&h.service, input).await?;
    let projected = h
        .service
        .project_nodes_to_scope(nodes, Some("task"))
        .await?;
    let issue_row = projected.iter().find(|n| n.id == changes).expect("matched");
    assert_eq!(issue_row.properties["task"]["status"], "in_progress");
    assert!(issue_row.properties.get("issue").is_none());

    // "In progress" is written against `task`, where `changes_requested`
    // reads as `in_progress`, and compares the stored value so the issue is
    // left to "Changes requested". The three queues that follow an issue
    // through review are written against `issue`, so each lists it by the
    // status it stores.
    assert_eq!(saved_query(&h, IN_PROGRESS_QUERY_ID).await, [task]);
    assert_eq!(saved_query(&h, AWAITING_REVIEW_QUERY_ID).await, [reviewing]);
    assert_eq!(saved_query(&h, CHANGES_REQUESTED_QUERY_ID).await, [changes]);
    assert_eq!(saved_query(&h, READY_TO_MERGE_QUERY_ID).await, [ready]);
    h.stop().await;
    Ok(())
}

/// "Ready tasks" lists an open issue with a checklist as it lists a task,
/// and leaves out one the rules would refuse to start.
#[tokio::test]
async fn ready_tasks_lists_a_ready_issue() -> Result<()> {
    let h = Harness::start().await?;
    let ready = issue(&h).await?;
    h.child(&ready, "checkbox", "- [ ] It works").await?;

    // No checklist: not ready.
    let bare = issue(&h).await?;
    // A draft spec: the spec rule would refuse the start.
    let gated = issue(&h).await?;
    h.child(&gated, "checkbox", "- [ ] It works").await?;
    let draft = h.create("spec", json!({})).await?;
    h.link(&draft, "tasks", &gated).await?;
    // Already under way.
    let started = h
        .create("issue", json!({ "status": "changes_requested" }))
        .await?;
    h.child(&started, "checkbox", "- [ ] It works").await?;

    let listed = saved_query(&h, READY_TASKS_QUERY_ID).await;
    assert_eq!(listed, [ready]);
    assert!(!listed.contains(&bare) && !listed.contains(&gated) && !listed.contains(&started));
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// The seeded task rules, on an issue
// ---------------------------------------------------------------------------

/// The lineage rule: a planned issue advances only under an approved plan
/// whose spec it links. It guards the move into a started or finished
/// status, and both of the issue's own statuses read as a started one.
#[tokio::test]
async fn the_lineage_rule_holds_an_issue_as_it_holds_a_task() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;

    let untraced = issue(&h).await?;
    h.link(&l.plan, "tasks", &untraced).await?;
    for status in [
        "in_progress",
        "in_review",
        "changes_requested",
        "ready_to_merge",
        "done",
    ] {
        assert_rejected(
            h.set_status(&untraced, status).await,
            TASK_NEEDS_LINEAGE,
            &format!("an issue missing its spec link, moved to {status}"),
        );
    }
    // Abandoning work needs no approval.
    h.set_status(&untraced, "cancelled").await?;

    // Fully traced, an issue moves through its whole flow, both ways round
    // the review loop.
    let traced = issue(&h).await?;
    h.link(&l.plan, "tasks", &traced).await?;
    h.link(&l.spec, "tasks", &traced).await?;
    h.child(&traced, "checkbox", "- [x] It works").await?;
    for status in [
        "in_progress",
        "in_review",
        "changes_requested",
        "in_review",
        "ready_to_merge",
        "done",
    ] {
        h.set_status(&traced, status)
            .await
            .unwrap_or_else(|e| panic!("a traced issue moves to {status}: {e}"));
    }
    h.stop().await;
    Ok(())
}

/// The spec rule guards the move to `in_progress`. `changes_requested` reads
/// as `in_progress` at `task` scope, so sending an issue back from review is
/// checked again: it is refused when none of the issue's specs is approved
/// any more. `ready_to_merge` reads as `in_review`, which the rule does not
/// guard.
#[tokio::test]
async fn the_spec_rule_checks_an_issue_on_each_move_that_reads_as_in_progress() -> Result<()> {
    let h = Harness::start().await?;

    let draft = h.create("spec", json!({})).await?;
    h.child(&draft, "checkbox", "- [ ] It works").await?;
    let gated = issue(&h).await?;
    h.link(&draft, "tasks", &gated).await?;
    for status in ["in_progress", "changes_requested"] {
        assert_rejected(
            h.set_status(&gated, status).await,
            TASK_SPEC_NOT_APPROVED,
            &format!("an issue under a draft spec, moved to {status}"),
        );
    }
    // Review is not the move the rule guards.
    h.set_status(&gated, "in_review").await?;
    h.set_status(&gated, "ready_to_merge").await?;

    // Once the spec is approved, the issue starts and goes round the loop.
    h.set(&draft, json!({ "spec_status": "approved" })).await?;
    let started = issue(&h).await?;
    h.link(&draft, "tasks", &started).await?;
    for status in ["in_progress", "in_review", "changes_requested"] {
        h.set_status(&started, status).await?;
    }

    // An issue that links no spec is an ordinary piece of work.
    let plain = issue(&h).await?;
    h.set_status(&plain, "changes_requested").await?;
    h.stop().await;
    Ok(())
}

/// The blockers rule guards the moves to `in_progress` and `in_review`, so
/// it guards all four of an issue's started statuses. A move between two
/// statuses that read the same at `task` scope is a change of status and is
/// checked like any other.
#[tokio::test]
async fn the_blockers_rule_holds_an_issue_in_every_started_status() -> Result<()> {
    let h = Harness::start().await?;
    let blocker = issue(&h).await?;
    let blocked = issue(&h).await?;
    h.link(&blocker, "blocks", &blocked).await?;

    for status in [
        "in_progress",
        "in_review",
        "changes_requested",
        "ready_to_merge",
    ] {
        assert_rejected(
            h.set_status(&blocked, status).await,
            TASK_IS_BLOCKED,
            &format!("a blocked issue, moved to {status}"),
        );
    }
    // An issue's own statuses do not finish a blocker: only `done` or
    // `cancelled` does.
    h.set_status(&blocker, "ready_to_merge").await?;
    assert_rejected(
        h.set_status(&blocked, "in_progress").await,
        TASK_IS_BLOCKED,
        "blocked by an issue that is ready to merge but has not landed",
    );
    h.set_status(&blocker, "done").await?;
    h.set_status(&blocked, "in_review").await?;

    // A blocker reopened while the issue is in review: the move to
    // `ready_to_merge` reads as `in_review` to `in_review`, and is refused
    // as the move into review would be.
    h.set_status(&blocker, "open").await?;
    assert_rejected(
        h.set_status(&blocked, "ready_to_merge").await,
        TASK_IS_BLOCKED,
        "in_review to ready_to_merge with a reopened blocker",
    );
    // A task can block an issue and an issue a task: `blocks` is a task's.
    let task = h.task().await?;
    h.link(&blocked, "blocks", &task).await?;
    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_IS_BLOCKED,
        "a task blocked by an unfinished issue",
    );
    h.stop().await;
    Ok(())
}

/// The criteria rule guards `done`: an issue with an unchecked item cannot
/// land, and one under a spec needs a checklist. Neither of the issue's own
/// statuses reads as `done`, so both are free of it.
#[tokio::test]
async fn the_criteria_rule_holds_an_issue_at_done() -> Result<()> {
    let h = Harness::start().await?;
    let unmet = issue(&h).await?;
    let item = h.child(&unmet, "checkbox", "- [ ] Not met yet").await?;
    for status in [
        "in_progress",
        "in_review",
        "changes_requested",
        "ready_to_merge",
    ] {
        h.set_status(&unmet, status).await?;
    }
    assert_rejected(
        h.set_status(&unmet, "done").await,
        TASK_HAS_UNCHECKED_ITEM,
        "an issue with an unchecked item",
    );
    h.set_content(&item, "- [x] Met").await?;
    h.set_status(&unmet, "done").await?;

    let spec = h.approved_spec().await?;
    let governed = issue(&h).await?;
    h.link(&spec, "tasks", &governed).await?;
    h.set_status(&governed, "ready_to_merge").await?;
    assert_rejected(
        h.set_status(&governed, "done").await,
        TASK_NEEDS_CRITERIA,
        "an issue under a spec with no checklist",
    );
    h.stop().await;
    Ok(())
}

/// The roll-up finishes a parent when every sub-task is done or cancelled.
/// An issue is a sub-task and a parent like any task; one waiting to merge
/// has not landed, so it holds its parent open.
#[tokio::test]
async fn the_roll_up_treats_an_issue_as_a_task() -> Result<()> {
    let h = Harness::start().await?;

    // An issue parent with an issue and a task under it.
    let parent = issue(&h).await?;
    let child_issue = issue(&h).await?;
    let child_task = h.task().await?;
    h.link(&parent, "has_child", &child_issue).await?;
    h.link(&parent, "has_child", &child_task).await?;

    h.set_status(&child_task, "done").await?;
    h.set_status(&child_issue, "ready_to_merge").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.field(&parent, "issue", "status").await?,
        Some(json!("open")),
        "an issue that has not landed holds its parent open"
    );

    h.set_status(&child_issue, "done").await?;
    assert!(
        wait_for_issue_status(&h, &parent, "done").await,
        "the roll-up finishes an issue parent once every sub-task is done"
    );
    // The roll-up wrote the parent's status where an issue keeps it.
    assert!(stored(&h, &parent).await.properties["task"]
        .get("status")
        .is_none());

    // A task parent is finished by its issue children too.
    let task_parent = h.task().await?;
    let only_child = issue(&h).await?;
    h.link(&task_parent, "has_child", &only_child).await?;
    h.set_status(&only_child, "cancelled").await?;
    for _ in 0..80 {
        if h.field(&task_parent, "task", "status").await? == Some(json!("done")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        h.field(&task_parent, "task", "status").await?,
        Some(json!("done"))
    );
    h.stop().await;
    Ok(())
}

/// `started_at` is stamped on an issue's first start, in the bucket a task
/// keeps it in. Each of the issue's own statuses is a started one.
#[tokio::test]
async fn started_at_is_stamped_on_an_issues_first_start() -> Result<()> {
    let h = Harness::start().await?;
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    for status in [
        "in_progress",
        "in_review",
        "changes_requested",
        "ready_to_merge",
    ] {
        let id = issue(&h).await?;
        assert_eq!(h.field(&id, "task", "started_at").await?, None);
        h.set_status(&id, status).await?;
        assert_eq!(
            h.field(&id, "task", "started_at").await?,
            Some(json!(today)),
            "{status}"
        );
        assert_eq!(typed(&h, &id).await["startedAt"], json!(today), "{status}");
    }
    for status in ["done", "cancelled"] {
        let id = issue(&h).await?;
        h.set_status(&id, status).await?;
        assert_eq!(h.field(&id, "task", "started_at").await?, None, "{status}");
    }

    // Only the first start is recorded.
    let kept = h
        .create(
            "issue",
            json!({ "status": "open", "started_at": "2026-01-02" }),
        )
        .await?;
    h.set_status(&kept, "changes_requested").await?;
    assert_eq!(
        h.field(&kept, "task", "started_at").await?,
        Some(json!("2026-01-02"))
    );

    // Created already started, an issue is stamped as it is created.
    let born = h
        .create("issue", json!({ "status": "changes_requested" }))
        .await?;
    assert_eq!(
        h.field(&born, "task", "started_at").await?,
        Some(json!(today))
    );
    h.stop().await;
    Ok(())
}

/// A status a user adds to `issue` maps to one the issue already has, and a
/// task rule reads it through both hops.
#[tokio::test]
async fn a_user_status_added_to_issue_resolves_to_a_task_status() -> Result<()> {
    let h = Harness::start().await?;
    nodespace_core::schema::handle_update_schema(
        &h.service,
        json!({
            "schema_id": "issue",
            "add_field_values": [{
                "field": "status",
                "values": [{ "value": "qa", "label": "QA", "mapsTo": "ready_to_merge" }]
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adding a status to issue: {e}"))?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let id = issue(&h).await?;
    h.set_status(&id, "qa").await?;
    assert_eq!(typed(&h, &id).await["status"], "qa");
    assert_eq!(
        matching(&h, "task", status_is("in_review")).await,
        std::slice::from_ref(&id)
    );
    assert_eq!(
        matching(&h, "issue", status_is("qa")).await,
        std::slice::from_ref(&id)
    );

    // The update takes it as a user value.
    let node = stored(&h, &id).await;
    h.service
        .update_issue_node(
            &id,
            node.version,
            IssueNodeUpdate {
                status: Some(IssueStatus::User("qa".to_string())),
                priority: Some(Some(nodespace_core::models::Priority::High)),
                ..Default::default()
            },
        )
        .await?;
    h.stop().await;
    Ok(())
}

/// A task retyped into an issue keeps what it had. An issue declares
/// `status` itself, so the status the task held in the `task` bucket moves to
/// the `issue` bucket, and the fields a task declares stay where they were.
#[tokio::test]
async fn a_task_retyped_into_an_issue_keeps_its_fields() -> Result<()> {
    let h = Harness::start().await?;
    let id = h
        .create("task", json!({ "status": "in_review", "priority": "high" }))
        .await?;

    let node = stored(&h, &id).await;
    h.service
        .update_node(
            &id,
            node.version,
            nodespace_core::models::NodeUpdate::default().with_node_type("issue".to_string()),
        )
        .await?;

    let node = stored(&h, &id).await;
    assert_eq!(node.node_type, "issue");
    assert_eq!(
        node.properties["issue"]["status"], "in_review",
        "{}",
        node.properties
    );
    assert!(node.properties["task"].get("status").is_none());
    assert_eq!(node.properties["task"]["priority"], "high");
    let wire = typed(&h, &id).await;
    assert_eq!(wire["status"], "in_review");
    assert_eq!(wire["priority"], "high");
    // It now takes an issue's own status and fields.
    h.set_status(&id, "ready_to_merge").await?;
    h.set(
        &id,
        json!({ "pull_request": { "title": "PR 7", "url": "https://example.com/pull/7" } }),
    )
    .await?;
    h.stop().await;
    Ok(())
}

/// A write that names an issue's status inside the `task` bucket, where a
/// task keeps it, is a write of the issue's status: the value goes to the
/// `issue` bucket, which declares the field, and is not dropped.
#[tokio::test]
async fn a_status_addressed_to_the_task_bucket_is_written_as_the_issues() -> Result<()> {
    let h = Harness::start().await?;
    let id = issue(&h).await?;

    h.set(
        &id,
        json!({ "task": { "status": "in_review", "priority": "high" } }),
    )
    .await?;
    let node = stored(&h, &id).await;
    assert_eq!(
        node.properties["issue"]["status"], "in_review",
        "{}",
        node.properties
    );
    assert!(node.properties["task"].get("status").is_none());
    assert_eq!(node.properties["task"]["priority"], "high");
    assert_eq!(typed(&h, &id).await["status"], "in_review");

    // The same field named in both buckets at once: the `issue` bucket is
    // where an issue's status is declared, so its value is the one written.
    h.set(
        &id,
        json!({ "issue": { "status": "ready_to_merge" }, "task": { "status": "done" } }),
    )
    .await?;
    let node = stored(&h, &id).await;
    assert_eq!(
        node.properties["issue"]["status"], "ready_to_merge",
        "{}",
        node.properties
    );
    assert!(node.properties["task"].get("status").is_none());

    // Named in the `task` bucket beside an `issue` bucket that does not name
    // it: the stored status stands. The write does not move the issue to a
    // status through a bucket that is not the field's.
    h.set(&id, json!({ "issue": {}, "task": { "status": "done" } }))
        .await?;
    let node = stored(&h, &id).await;
    assert_eq!(
        node.properties["issue"]["status"], "ready_to_merge",
        "{}",
        node.properties
    );
    assert!(node.properties["task"].get("status").is_none());
    h.stop().await;
    Ok(())
}

/// An issue's `status` adds two values to the ones it inherits from `task`, so
/// a value a user adds to `task`'s status afterwards is an issue's too.
#[tokio::test]
async fn a_status_added_to_task_is_an_issues() -> Result<()> {
    let h = Harness::start().await?;
    nodespace_core::schema::handle_update_schema(
        &h.service,
        json!({
            "schema_id": "task",
            "add_field_values": [{
                "field": "status",
                "values": [{ "value": "waiting", "label": "Waiting" }]
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adding a status to task: {e}"))?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let task = h.task().await?;
    h.set_status(&task, "waiting").await?;
    let id = issue(&h).await?;
    h.set_status(&id, "waiting").await?;
    // The two statuses an issue adds are still its own.
    h.set_status(&id, "changes_requested").await?;
    assert_refused(h.set_status(&task, "changes_requested").await, "status");
    h.stop().await;
    Ok(())
}
