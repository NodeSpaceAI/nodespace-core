//! What the seeded spec, plan and decision rules do, against a running
//! engine (ADR-092 §6).
//!
//! Nothing here installs a rule: these drive the Plays every database is
//! seeded with. Every rule is a `reject`, and a reject rule whose condition
//! resolves to nothing is indistinguishable from a working one until the
//! write it should veto is attempted. So each rule is exercised both ways:
//! the write it must refuse, and the write beside it that it must let
//! through. A refusal is matched on the rule's own message, so a write
//! refused by a different rule does not pass for the one under test.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::playbook::core_plays::{
    CORE_PLAY_IDS, PLAN_APPROVAL_PLAY_ID, SPEC_APPROVAL_PLAY_ID, SUPERSEDED_LOCK_PLAY_ID,
    TASK_BLOCKERS_PLAY_ID, TASK_CRITERIA_PLAY_ID, TASK_LINEAGE_PLAY_ID, TASK_SPEC_PLAY_ID,
};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_core::PlaybookEngine;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

// A fragment of each rule's message, enough to tell the rules apart.
const SPEC_NEEDS_CRITERIA: &str = "cannot be approved until it has success criteria";
const PLAN_NEEDS_APPROVED_SPEC: &str = "cannot be approved until it is linked to an approved spec";
const PLAN_CREATED_APPROVED: &str = "cannot be created already approved";
const TASK_NEEDS_LINEAGE: &str = "until that plan is approved and the task is linked";
const TASK_NEEDS_APPROVED_SPEC: &str = "it is not linked to an approved spec";
const TASK_IS_BLOCKED: &str = "blocked by a task that is not finished";
const TASK_HAS_UNCHECKED_ITEM: &str = "its checklist has an unchecked item";
const TASK_NEEDS_CRITERIA: &str = "cannot be marked done without acceptance criteria";
const IS_LOCKED: &str = "is locked as the record of what was agreed";
const CANNOT_BE_REINSTATED: &str = "cannot be reinstated";

pub(crate) struct Harness {
    pub(crate) service: Arc<NodeService>,
    _tmp: TempDir,
    shutdown: watch::Sender<bool>,
    engine: tokio::task::JoinHandle<Result<()>>,
}

impl Harness {
    /// Open a fresh database and start the engine, so the seeded invariant
    /// rules are live on the write path.
    pub(crate) async fn start() -> Result<Self> {
        let tmp = TempDir::new()?;
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);

        let (shutdown, rx) = watch::channel(false);
        let engine = Arc::new(PlaybookEngine::new(Arc::clone(&service)));
        service.set_playbook_lifecycle(engine.lifecycle().clone());
        let engine = tokio::spawn(async move { engine.start(rx).await });
        tokio::time::sleep(Duration::from_millis(80)).await;

        Ok(Self {
            service,
            _tmp: tmp,
            shutdown,
            engine,
        })
    }

    pub(crate) async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), self.engine).await;
    }

    pub(crate) async fn try_create(
        &self,
        node_type: &str,
        content: &str,
        properties: Value,
    ) -> Result<String, NodeServiceError> {
        self.service
            .create_node(Node::new(
                node_type.to_string(),
                content.to_string(),
                properties,
            ))
            .await
    }

    pub(crate) async fn create(&self, node_type: &str, properties: Value) -> Result<String> {
        Ok(self
            .try_create(node_type, &format!("A {node_type}"), properties)
            .await?)
    }

    /// An open task in the light lane (`requires_spec: false`), so the tests
    /// of every other rule can start it without first linking an approved
    /// spec. The spec rule's own tests use [`Self::spec_gated_task`].
    pub(crate) async fn task(&self) -> Result<String> {
        self.create("task", json!({ "status": "open", "requires_spec": false }))
            .await
    }

    /// An open task as a user creates it: nothing marks it as not needing a
    /// spec, so the schema default (a spec is required) applies.
    pub(crate) async fn spec_gated_task(&self) -> Result<String> {
        self.create("task", json!({ "status": "open" })).await
    }

    /// A child of `parent`, placed directly under it.
    pub(crate) async fn child(
        &self,
        parent: &str,
        node_type: &str,
        content: &str,
    ) -> Result<String> {
        let id = self.try_create(node_type, content, json!({})).await?;
        self.link(parent, "has_child", &id).await?;
        Ok(id)
    }

    pub(crate) async fn link(&self, source: &str, name: &str, target: &str) -> Result<()> {
        self.service
            .create_relationship(source, name, target, json!({}))
            .await?;
        Ok(())
    }

    /// Apply `properties` to `id` with the version just read.
    pub(crate) async fn set(&self, id: &str, properties: Value) -> Result<Node, NodeServiceError> {
        let node = self.service.get_node(id).await?.expect("node exists");
        self.service
            .update_node(
                id,
                node.version,
                NodeUpdate::default().with_properties(properties),
            )
            .await
    }

    pub(crate) async fn set_status(
        &self,
        id: &str,
        status: &str,
    ) -> Result<Node, NodeServiceError> {
        self.set(id, json!({ "status": status })).await
    }

    pub(crate) async fn set_content(&self, id: &str, content: &str) -> Result<()> {
        let node = self.service.get_node(id).await?.expect("node exists");
        self.service
            .update_node(
                id,
                node.version,
                NodeUpdate::default().with_content(content.to_string()),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn field(
        &self,
        id: &str,
        node_type: &str,
        field: &str,
    ) -> Result<Option<Value>> {
        let node = self.service.get_node(id).await?.expect("node exists");
        Ok(node
            .properties
            .get(node_type)
            .and_then(|bucket| bucket.get(field))
            .filter(|v| !v.is_null())
            .cloned())
    }

    /// A spec with one criterion, approved.
    pub(crate) async fn approved_spec(&self) -> Result<String> {
        let spec = self.create("spec", json!({})).await?;
        self.child(&spec, "checkbox", "- [ ] It works").await?;
        self.set(&spec, json!({ "spec_status": "approved" }))
            .await?;
        Ok(spec)
    }

    /// An approved spec, an approved plan against it, and an open task
    /// linked to both: the fully traced starting point each task test
    /// narrows from. The task has no checklist yet.
    pub(crate) async fn traced_task(&self) -> Result<Lineage> {
        let spec = self.approved_spec().await?;
        let plan = self.create("plan", json!({})).await?;
        self.link(&plan, "spec", &spec).await?;
        self.set(&plan, json!({ "plan_status": "approved" }))
            .await?;
        let task = self.task().await?;
        self.link(&plan, "tasks", &task).await?;
        self.link(&spec, "tasks", &task).await?;
        Ok(Lineage { spec, plan, task })
    }
}

pub(crate) struct Lineage {
    pub(crate) spec: String,
    pub(crate) plan: String,
    pub(crate) task: String,
}

/// Whether `result` is a refusal by the rule whose message holds `fragment`.
fn rejected<T: std::fmt::Debug>(result: &Result<T, NodeServiceError>, fragment: &str) -> bool {
    matches!(
        result,
        Err(NodeServiceError::PlayRuleRejected { message, .. }) if message.contains(fragment)
    )
}

#[track_caller]
fn assert_rejected<T: std::fmt::Debug>(
    result: Result<T, NodeServiceError>,
    fragment: &str,
    why: &str,
) {
    assert!(
        rejected(&result, fragment),
        "{why}: expected a refusal saying '{fragment}', got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// Every database is seeded with the seven rules, under their fixed ids,
/// switched on.
#[tokio::test]
async fn the_seven_plays_are_seeded_with_fixed_ids_and_enabled() -> Result<()> {
    let h = Harness::start().await?;
    for id in [
        SPEC_APPROVAL_PLAY_ID,
        PLAN_APPROVAL_PLAY_ID,
        TASK_LINEAGE_PLAY_ID,
        TASK_BLOCKERS_PLAY_ID,
        TASK_CRITERIA_PLAY_ID,
        SUPERSEDED_LOCK_PLAY_ID,
        TASK_SPEC_PLAY_ID,
    ] {
        assert!(CORE_PLAY_IDS.contains(&id));
        let play = h
            .service
            .get_node(id)
            .await?
            .unwrap_or_else(|| panic!("play {id} is seeded"));
        assert_eq!(play.node_type, "play");
        assert_eq!(play.properties["play"]["enabled"], json!(true), "{id}");
        assert!(
            play.properties["play"]["rules"]
                .as_array()
                .is_some_and(|rules| !rules.is_empty()),
            "{id} carries its rules"
        );
    }
    h.stop().await;
    Ok(())
}

/// A user's edit to a seeded Play survives a restart (ADR-072): reopening
/// the database reconciles the seed table and leaves an edited Play alone.
#[tokio::test]
async fn a_users_edit_to_a_seeded_play_survives_a_restart() -> Result<()> {
    let tmp = TempDir::new()?;
    let db_path = tmp.path().join("test.db");

    {
        let mut store = Arc::new(SqliteStore::new(db_path.clone()).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);
        // Switch one rule off, and reword another's description.
        for (id, patch) in [
            (TASK_CRITERIA_PLAY_ID, json!({ "enabled": false })),
            (
                TASK_BLOCKERS_PLAY_ID,
                json!({ "description": "Our team's wording." }),
            ),
        ] {
            let play = service.get_node(id).await?.expect("seeded");
            service
                .update_node(
                    id,
                    play.version,
                    NodeUpdate::default().with_properties(patch),
                )
                .await?;
        }
    }

    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    let criteria = service
        .get_node(TASK_CRITERIA_PLAY_ID)
        .await?
        .expect("still there");
    assert_eq!(criteria.properties["play"]["enabled"], json!(false));
    let blockers = service
        .get_node(TASK_BLOCKERS_PLAY_ID)
        .await?
        .expect("still there");
    assert_eq!(
        blockers.properties["play"]["description"],
        json!("Our team's wording.")
    );
    // An untouched Play is still as shipped, and none is duplicated.
    let lineage = service
        .get_node(TASK_LINEAGE_PLAY_ID)
        .await?
        .expect("still there");
    assert_eq!(lineage.properties["play"]["enabled"], json!(true));
    let mut ids: Vec<String> = service
        .query_nodes_by_type("play", true)
        .await?
        .into_iter()
        .map(|play| play.id)
        .collect();
    ids.sort();
    let mut expected: Vec<String> = CORE_PLAY_IDS.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(ids, expected);
    Ok(())
}

/// A rule the user switched off no longer refuses anything.
#[tokio::test]
async fn a_play_switched_off_stops_rejecting() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.task().await?;
    h.child(&task, "checkbox", "- [ ] Not met yet").await?;
    assert_rejected(
        h.set_status(&task, "done").await,
        TASK_HAS_UNCHECKED_ITEM,
        "the rule is on",
    );

    h.set(TASK_CRITERIA_PLAY_ID, json!({ "enabled": false }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.set_status(&task, "done").await?;
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Spec approval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_spec_with_no_checkbox_child_cannot_be_approved() -> Result<()> {
    let h = Harness::start().await?;

    // No children at all.
    let bare = h.create("spec", json!({})).await?;
    assert_rejected(
        h.set(&bare, json!({ "spec_status": "approved" })).await,
        SPEC_NEEDS_CRITERIA,
        "a spec with no children",
    );

    // Children, but no checkbox among them. A text node that merely starts
    // with the checkbox prefix is not a checkbox, and a checkbox nested
    // deeper is ordinary content: only direct children count.
    let notes_only = h.create("spec", json!({})).await?;
    let note = h
        .child(&notes_only, "text", "- [ ] text, not a checkbox")
        .await?;
    h.child(&note, "checkbox", "- [x] nested one level down")
        .await?;
    assert_rejected(
        h.set(&notes_only, json!({ "spec_status": "approved" }))
            .await,
        SPEC_NEEDS_CRITERIA,
        "a spec whose only checkbox is nested deeper",
    );

    // One criterion directly under the spec, met or not: it can be approved.
    let ready = h.create("spec", json!({})).await?;
    h.child(&ready, "text", "Background").await?;
    h.child(&ready, "checkbox", "- [ ] It works").await?;
    h.set(&ready, json!({ "spec_status": "approved" })).await?;
    assert_eq!(
        h.field(&ready, "spec", "spec_status").await?,
        Some(json!("approved"))
    );

    // The rule is about approval only: a criterion-less spec can still be
    // edited and superseded.
    h.set(&bare, json!({ "objective": "Still a draft" }))
        .await?;
    h.set(&bare, json!({ "spec_status": "superseded" })).await?;
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Plan approval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_plan_cannot_be_approved_against_a_draft_spec() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.create("spec", json!({})).await?;
    h.child(&spec, "checkbox", "- [ ] It works").await?;
    let plan = h.create("plan", json!({})).await?;
    h.link(&plan, "spec", &spec).await?;

    assert_rejected(
        h.set(&plan, json!({ "plan_status": "approved" })).await,
        PLAN_NEEDS_APPROVED_SPEC,
        "a plan whose spec is still draft",
    );

    h.set(&spec, json!({ "spec_status": "approved" })).await?;
    h.set(&plan, json!({ "plan_status": "approved" })).await?;
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_plan_with_no_spec_cannot_be_approved() -> Result<()> {
    let h = Harness::start().await?;
    let plan = h.create("plan", json!({})).await?;
    assert_rejected(
        h.set(&plan, json!({ "plan_status": "approved" })).await,
        PLAN_NEEDS_APPROVED_SPEC,
        "a plan with no spec",
    );
    // It can still be written and superseded.
    h.set(&plan, json!({ "approach": "Two phases" })).await?;
    h.set(&plan, json!({ "plan_status": "superseded" })).await?;
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_plan_cannot_be_created_already_approved() -> Result<()> {
    let h = Harness::start().await?;
    assert_rejected(
        h.try_create("plan", "A plan", json!({ "plan_status": "approved" }))
            .await,
        PLAN_CREATED_APPROVED,
        "creating an approved plan",
    );
    // A new plan is a draft unless it says otherwise.
    let plan = h.create("plan", json!({})).await?;
    assert_eq!(
        h.field(&plan, "plan", "plan_status").await?,
        Some(json!("draft"))
    );
    h.stop().await;
    Ok(())
}

/// A reject rule must fail closed on data shape. `bulk_create` applies no
/// schema defaults, so an imported spec can carry no `spec_status` at all,
/// which is a draft. An unguarded read of it would fail the condition and
/// let the approval through.
#[tokio::test]
async fn a_spec_with_no_status_does_not_count_as_approved() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h
        .service
        .bulk_create(vec![Node::new(
            "spec".to_string(),
            "Imported spec".to_string(),
            json!({ "objective": "Imported without a status" }),
        )])
        .await?
        .remove(0);
    let plan = h.create("plan", json!({})).await?;
    h.link(&plan, "spec", &spec).await?;

    assert_rejected(
        h.set(&plan, json!({ "plan_status": "approved" })).await,
        PLAN_NEEDS_APPROVED_SPEC,
        "a spec with no spec_status is not approved",
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task lineage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fully_traced_task_moves_through_every_status() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;
    h.child(&l.task, "checkbox", "- [x] It works").await?;
    for status in ["in_progress", "in_review", "done"] {
        h.set_status(&l.task, status).await?;
    }
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_task_under_a_draft_plan_cannot_start_be_reviewed_or_finish() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.approved_spec().await?;
    let plan = h.create("plan", json!({})).await?;
    h.link(&plan, "spec", &spec).await?;
    let task = h.task().await?;
    h.link(&plan, "tasks", &task).await?;
    h.link(&spec, "tasks", &task).await?;
    h.child(&task, "checkbox", "- [x] It works").await?;

    for status in ["in_progress", "in_review", "done"] {
        assert_rejected(
            h.set_status(&task, status).await,
            TASK_NEEDS_LINEAGE,
            &format!("moving a task under a draft plan to {status}"),
        );
    }

    // Approving the plan is what lets the same task move.
    h.set(&plan, json!({ "plan_status": "approved" })).await?;
    h.set_status(&task, "in_review").await?;
    h.stop().await;
    Ok(())
}

/// Abandoning work needs no approval.
#[tokio::test]
async fn a_task_under_a_draft_plan_can_be_cancelled() -> Result<()> {
    let h = Harness::start().await?;
    let plan = h.create("plan", json!({})).await?;
    let task = h.task().await?;
    h.link(&plan, "tasks", &task).await?;
    h.set_status(&task, "cancelled").await?;
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_planned_task_must_link_its_plans_own_spec() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;

    // No spec link at all.
    let unlinked = h.task().await?;
    h.link(&l.plan, "tasks", &unlinked).await?;
    assert_rejected(
        h.set_status(&unlinked, "in_progress").await,
        TASK_NEEDS_LINEAGE,
        "a planned task with no spec link",
    );

    // Linked to some other approved spec, not the plan's.
    let other_spec = h.approved_spec().await?;
    let misdirected = h.task().await?;
    h.link(&l.plan, "tasks", &misdirected).await?;
    h.link(&other_spec, "tasks", &misdirected).await?;
    assert_rejected(
        h.set_status(&misdirected, "in_review").await,
        TASK_NEEDS_LINEAGE,
        "a planned task linked to an unrelated spec",
    );

    // Linking the plan's own spec is what lets it move. A task may serve
    // more than one spec.
    h.link(&l.spec, "tasks", &misdirected).await?;
    h.set_status(&misdirected, "in_review").await?;
    h.stop().await;
    Ok(())
}

/// The same shape as the spec with no status, one level down: a plan with no
/// `plan_status` holds its tasks at `open`.
#[tokio::test]
async fn a_task_under_a_plan_with_no_status_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.approved_spec().await?;
    let plan = h
        .service
        .bulk_create(vec![Node::new(
            "plan".to_string(),
            "Imported plan".to_string(),
            json!({ "approach": "Imported without a status" }),
        )])
        .await?
        .remove(0);
    h.link(&plan, "spec", &spec).await?;
    let task = h.task().await?;
    h.link(&plan, "tasks", &task).await?;
    h.link(&spec, "tasks", &task).await?;

    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_NEEDS_LINEAGE,
        "a task under a plan with no status",
    );
    h.stop().await;
    Ok(())
}

/// A type that extends `task` is held to the task rules. Both directions,
/// because a subtype can fail two opposite ways: its status change never
/// reaching the task-scoped rule, or the rule losing the plan's status when
/// it reads the node at `task` scope.
#[tokio::test]
async fn a_task_subtype_is_held_to_the_task_rules() -> Result<()> {
    let h = Harness::start().await?;
    handle_create_schema(
        &h.service,
        json!({
            "name": "Ticket",
            "extends": "task",
            "fields": [{ "name": "estimate", "type": "number" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ticket schema: {e}"))?;
    // The engine learns of a new subtype from the schema's event.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let l = h.traced_task().await?;

    let traced = h.create("ticket", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &traced).await?;
    h.link(&l.spec, "tasks", &traced).await?;
    h.set_status(&traced, "in_progress").await?;

    let untraced = h.create("ticket", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &untraced).await?;
    assert_rejected(
        h.set_status(&untraced, "in_progress").await,
        TASK_NEEDS_LINEAGE,
        "a ticket missing its spec link",
    );

    // The checklist rule reaches it too.
    h.child(&traced, "checkbox", "- [ ] Not met yet").await?;
    assert_rejected(
        h.set_status(&traced, "done").await,
        TASK_HAS_UNCHECKED_ITEM,
        "a ticket with an unchecked item",
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task blockers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_blocked_task_cannot_start_or_be_put_in_review() -> Result<()> {
    let h = Harness::start().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;

    for status in ["in_progress", "in_review"] {
        assert_rejected(
            h.set_status(&blocked, status).await,
            TASK_IS_BLOCKED,
            &format!("moving a blocked task to {status}"),
        );
    }
    // A blocker that has only been started still blocks.
    h.set_status(&blocker, "in_progress").await?;
    assert_rejected(
        h.set_status(&blocked, "in_progress").await,
        TASK_IS_BLOCKED,
        "a blocker in progress is not finished",
    );

    // Finishing the blocker is what lets the blocked task move.
    h.set_status(&blocker, "done").await?;
    h.set_status(&blocked, "in_progress").await?;
    h.set_status(&blocked, "in_review").await?;
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_blocker_no_longer_blocks() -> Result<()> {
    let h = Harness::start().await?;
    let finished = h.task().await?;
    let abandoned = h.task().await?;
    let blocked = h.task().await?;
    h.link(&finished, "blocks", &blocked).await?;
    h.link(&abandoned, "blocks", &blocked).await?;
    h.set_status(&finished, "done").await?;

    // One blocker is done, the other still open.
    assert_rejected(
        h.set_status(&blocked, "in_review").await,
        TASK_IS_BLOCKED,
        "one of two blockers is still open",
    );
    h.set_status(&abandoned, "cancelled").await?;
    h.set_status(&blocked, "in_review").await?;
    h.stop().await;
    Ok(())
}

/// The rule guards starting only: a blocked task may wait in `open`, and may
/// be abandoned.
#[tokio::test]
async fn a_blocked_task_can_be_cancelled() -> Result<()> {
    let h = Harness::start().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;
    h.set_status(&blocked, "cancelled").await?;
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task spec (ADR-097)
// ---------------------------------------------------------------------------

/// A task nobody marked cannot start with no spec linked, and the refusal
/// says what to do next. Cancelling is still allowed, and the task starts
/// once an approved spec is linked.
#[tokio::test]
async fn a_task_with_no_spec_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.spec_gated_task().await?;
    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_NEEDS_APPROVED_SPEC,
        "no spec is linked",
    );
    let refused = h.set_status(&task, "in_progress").await;
    let Err(NodeServiceError::PlayRuleRejected { message, .. }) = refused else {
        panic!("expected a refusal, got {refused:?}");
    };
    assert!(message.contains("Writing a Spec"), "{message}");
    assert!(message.contains("nodespace "), "{message}");
    assert!(message.contains("do not retry"), "{message}");

    h.set_status(&task, "cancelled").await?;

    let other = h.spec_gated_task().await?;
    let spec = h.approved_spec().await?;
    h.link(&spec, "tasks", &other).await?;
    h.set_status(&other, "in_progress").await?;
    h.stop().await;
    Ok(())
}

/// A linked spec that is still a draft does not count, whether or not the
/// task has a plan. This is the case the lineage rule does not reach.
#[tokio::test]
async fn a_task_with_a_draft_spec_and_no_plan_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.spec_gated_task().await?;
    let spec = h.create("spec", json!({})).await?;
    h.link(&spec, "tasks", &task).await?;
    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_NEEDS_APPROVED_SPEC,
        "the spec is a draft",
    );

    h.child(&spec, "checkbox", "- [ ] It works").await?;
    h.set(&spec, json!({ "spec_status": "approved" })).await?;
    h.set_status(&task, "in_progress").await?;
    h.stop().await;
    Ok(())
}

/// The light lane: a task marked as not needing a spec starts with none
/// linked, and one explicitly marked as needing one is held to the rule.
#[tokio::test]
async fn a_task_marked_as_not_needing_a_spec_can_start() -> Result<()> {
    let h = Harness::start().await?;
    let chore = h.task().await?;
    h.set_status(&chore, "in_progress").await?;

    let task = h.spec_gated_task().await?;
    h.set(&task, json!({ "requires_spec": false })).await?;
    h.set_status(&task, "in_progress").await?;
    h.set_status(&task, "open").await?;
    h.set(&task, json!({ "requires_spec": true })).await?;
    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_NEEDS_APPROVED_SPEC,
        "the task is marked as needing a spec",
    );
    h.stop().await;
    Ok(())
}

/// The rule is the user's: switched off, or edited to say nothing, it no
/// longer refuses anything (ADR-072).
#[tokio::test]
async fn the_spec_play_switched_off_or_emptied_stops_rejecting() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.spec_gated_task().await?;
    assert_rejected(
        h.set_status(&task, "in_progress").await,
        TASK_NEEDS_APPROVED_SPEC,
        "the rule is on",
    );

    h.set(TASK_SPEC_PLAY_ID, json!({ "enabled": false }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.set_status(&task, "in_progress").await?;
    h.set_status(&task, "open").await?;

    h.set(TASK_SPEC_PLAY_ID, json!({ "enabled": true, "rules": [] }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.set_status(&task, "in_progress").await?;
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task criteria
// ---------------------------------------------------------------------------

/// A task is also an ordinary to-do: with no plan, no spec and no checklist
/// it moves through every status, and none of the rules touches it. The same
/// task with one unchecked checkbox child cannot move to `done`.
#[tokio::test]
async fn a_plain_task_moves_freely_until_it_has_an_unchecked_item() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.task().await?;
    for status in ["in_progress", "in_review", "done", "cancelled", "open"] {
        h.set_status(&task, status).await?;
    }

    let item = h.child(&task, "checkbox", "- [ ] Buy milk").await?;
    assert_rejected(
        h.set_status(&task, "done").await,
        TASK_HAS_UNCHECKED_ITEM,
        "a task with an unchecked item",
    );
    // The checklist guards finishing only.
    for status in ["in_progress", "in_review", "cancelled", "open"] {
        h.set_status(&task, status).await?;
    }

    // Checking the item is what lets it close: the rule reads the
    // checkbox's derived `checked`, which follows its content.
    h.set_content(&item, "- [x] Buy milk").await?;
    h.set_status(&task, "done").await?;
    h.stop().await;
    Ok(())
}

/// Only direct checkbox children are criteria. A note that starts with the
/// checkbox prefix is not one, and a checkbox nested deeper is ordinary
/// content.
#[tokio::test]
async fn only_direct_checkbox_children_are_criteria() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.task().await?;
    let note = h.child(&task, "text", "- [ ] text, not a checkbox").await?;
    h.child(&note, "checkbox", "- [ ] nested one level down")
        .await?;
    h.set_status(&task, "done").await?;

    // Among several items, one unchecked is enough to hold the task.
    let listed = h.task().await?;
    h.child(&listed, "checkbox", "- [x] First").await?;
    h.child(&listed, "text", "A note between them").await?;
    h.child(&listed, "checkbox", "- [ ] Second").await?;
    assert_rejected(
        h.set_status(&listed, "done").await,
        TASK_HAS_UNCHECKED_ITEM,
        "one unchecked item among several",
    );
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_task_under_a_plan_or_spec_needs_a_checklist_to_finish() -> Result<()> {
    let h = Harness::start().await?;

    // Under a plan and its spec.
    let l = h.traced_task().await?;
    h.set_status(&l.task, "in_progress").await?;
    assert_rejected(
        h.set_status(&l.task, "done").await,
        TASK_NEEDS_CRITERIA,
        "a planned task with no checklist",
    );
    // A note is not a criterion.
    h.child(&l.task, "text", "Looks fine to me").await?;
    assert_rejected(
        h.set_status(&l.task, "done").await,
        TASK_NEEDS_CRITERIA,
        "a planned task with a note and no checkbox",
    );
    h.child(&l.task, "checkbox", "- [x] It works").await?;
    h.set_status(&l.task, "done").await?;

    // Under a spec alone.
    let spec_only = h.task().await?;
    h.link(&l.spec, "tasks", &spec_only).await?;
    assert_rejected(
        h.set_status(&spec_only, "done").await,
        TASK_NEEDS_CRITERIA,
        "a task under a spec with no checklist",
    );
    h.child(&spec_only, "checkbox", "- [X] It works").await?;
    h.set_status(&spec_only, "done").await?;
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Superseded lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_superseded_spec_is_locked() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.approved_spec().await?;

    // An approved spec can still be edited, and can be superseded.
    h.set(&spec, json!({ "objective": "Clarified" })).await?;
    h.set(&spec, json!({ "spec_status": "superseded" })).await?;

    for field in ["objective", "boundaries"] {
        assert_rejected(
            h.set(&spec, json!({ field: "Rewritten afterwards" })).await,
            IS_LOCKED,
            &format!("editing a superseded spec's {field}"),
        );
    }
    for status in ["draft", "approved"] {
        assert_rejected(
            h.set(&spec, json!({ "spec_status": status })).await,
            CANNOT_BE_REINSTATED,
            &format!("moving a superseded spec back to {status}"),
        );
    }
    assert_eq!(
        h.field(&spec, "spec", "objective").await?,
        Some(json!("Clarified")),
        "a refused edit leaves the field as it was"
    );
    h.stop().await;
    Ok(())
}

/// Clearing the status is leaving `superseded` too: a cleared status reads
/// as the default, and the field locks would no longer hold. The flat write
/// path the CLI and the agent use can name `null`, so the lock must refuse
/// it there, for each of the three types.
#[tokio::test]
async fn a_superseded_status_cannot_be_cleared() -> Result<()> {
    let h = Harness::start().await?;
    for (node_type, status_field, editable) in [
        ("spec", "spec_status", Some("objective")),
        ("plan", "plan_status", Some("approach")),
        ("decision", "decision_status", None),
    ] {
        let node = h.create(node_type, json!({})).await?;
        h.set(&node, json!({ status_field: "superseded" })).await?;

        assert_rejected(
            h.set(&node, json!({ status_field: null })).await,
            CANNOT_BE_REINSTATED,
            &format!("clearing a superseded {node_type}'s status"),
        );
        assert_eq!(
            h.field(&node, node_type, status_field).await?,
            Some(json!("superseded"))
        );
        // The lock still holds afterwards.
        if let Some(field) = editable {
            assert_rejected(
                h.set(&node, json!({ field: "Rewritten afterwards" })).await,
                IS_LOCKED,
                &format!("editing a superseded {node_type} after the refused clear"),
            );
        }

        // A status that was never superseded can be cleared: the rule is
        // about leaving `superseded`, not about the field.
        let draft = h.create(node_type, json!({})).await?;
        h.set(&draft, json!({ status_field: null })).await?;
    }
    h.stop().await;
    Ok(())
}

/// One write that edits a field and supersedes is refused like an edit made
/// afterwards: the lock reads the status as the write leaves it. Superseding
/// is a write of its own.
#[tokio::test]
async fn editing_and_superseding_in_one_write_is_refused() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.create("spec", json!({})).await?;
    assert_rejected(
        h.set(
            &spec,
            json!({ "objective": "Last word", "spec_status": "superseded" }),
        )
        .await,
        IS_LOCKED,
        "editing and superseding together",
    );
    h.set(&spec, json!({ "objective": "Last word" })).await?;
    h.set(&spec, json!({ "spec_status": "superseded" })).await?;
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_superseded_plan_is_locked() -> Result<()> {
    let h = Harness::start().await?;
    let plan = h
        .create("plan", json!({ "approach": "Two phases" }))
        .await?;
    h.set(&plan, json!({ "risks": "Time" })).await?;
    h.set(&plan, json!({ "plan_status": "superseded" })).await?;

    for field in ["approach", "risks"] {
        assert_rejected(
            h.set(&plan, json!({ field: "Rewritten afterwards" })).await,
            IS_LOCKED,
            &format!("editing a superseded plan's {field}"),
        );
    }
    assert_rejected(
        h.set(&plan, json!({ "plan_status": "draft" })).await,
        CANNOT_BE_REINSTATED,
        "moving a superseded plan back to draft",
    );
    h.stop().await;
    Ok(())
}

/// A decision's status is its only field, so its lock is the status rule.
#[tokio::test]
async fn a_superseded_decision_cannot_be_reinstated() -> Result<()> {
    let h = Harness::start().await?;
    let decision = h.create("decision", json!({})).await?;
    assert_eq!(
        h.field(&decision, "decision", "decision_status").await?,
        Some(json!("proposed"))
    );

    // Every other move is free, in either direction.
    h.set(&decision, json!({ "decision_status": "accepted" }))
        .await?;
    h.set(&decision, json!({ "decision_status": "proposed" }))
        .await?;
    h.set(&decision, json!({ "decision_status": "superseded" }))
        .await?;

    for status in ["proposed", "accepted"] {
        assert_rejected(
            h.set(&decision, json!({ "decision_status": status })).await,
            CANNOT_BE_REINSTATED,
            &format!("moving a superseded decision back to {status}"),
        );
    }
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// The rules beside the parent-task roll-up
// ---------------------------------------------------------------------------

async fn wait_for_status(h: &Harness, id: &str, status: &str) -> bool {
    for _ in 0..80 {
        if h.field(id, "task", "status").await.ok().flatten() == Some(json!(status)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// The roll-up completes a parent by writing its status, and that write is
/// held to the rules like any other. When a rule refuses it the roll-up is
/// declined, not broken: the parent stays as it was, the roll-up Play is not
/// suspended, and it goes on completing the parents it may.
#[tokio::test]
async fn a_roll_up_the_rules_refuse_leaves_the_roll_up_play_running() -> Result<()> {
    use nodespace_core::playbook::core_plays::PARENT_TASK_COMPLETION_PLAY_ID;

    let h = Harness::start().await?;

    // A parent under a spec, with no checklist of its own: the criteria
    // rule refuses to finish it.
    let l = h.traced_task().await?;
    let governed_child = h.task().await?;
    h.link(&l.task, "has_child", &governed_child).await?;
    h.set_status(&governed_child, "done").await?;

    // A plain parent, completed after the refusal: only a Play that is
    // still running rolls it up.
    let plain_parent = h.task().await?;
    let plain_child = h.task().await?;
    h.link(&plain_parent, "has_child", &plain_child).await?;
    h.set_status(&plain_child, "done").await?;
    assert!(
        wait_for_status(&h, &plain_parent, "done").await,
        "the roll-up must still complete a parent no rule refuses"
    );

    assert_eq!(
        h.field(&l.task, "task", "status").await?,
        Some(json!("open")),
        "the refused parent stays as it was"
    );
    let play = h
        .service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .expect("seeded");
    assert_eq!(play.properties["play"]["enabled"], json!(true));
    assert!(
        play.properties["play"]
            .get("suspended_reason")
            .is_none_or(|v| v.is_null()),
        "a refused write must not suspend the roll-up: {}",
        play.properties["play"]
    );
    h.stop().await;
    Ok(())
}

/// In a `for_each`, a refusal declines that item alone. A team's own Play
/// starts every task of a plan when the plan is approved; one of the tasks is
/// blocked, so the blocker rule refuses it. The other tasks are still
/// started, and the Play keeps running.
#[tokio::test]
async fn a_refused_item_does_not_stop_the_rest_of_a_for_each() -> Result<()> {
    let h = Harness::start().await?;
    let play = h
        .try_create(
            "play",
            "Start a plan's tasks when it is approved",
            json!({
                "enabled": true,
                "rules": [{
                    "name": "start-tasks-of-an-approved-plan",
                    "description": "Start every task of a plan once the plan is approved",
                    "class": "reactive",
                    "trigger": {
                        "type": "graph_event",
                        "on": "property_changed",
                        "select": { "target_type": "plan" },
                        "property_key": "plan.plan_status"
                    },
                    "conditions": [{
                        "expr": "node.plan_status == 'approved'",
                        "description": "The plan is approved"
                    }],
                    "actions": [{
                        "action_type": "update_node",
                        "description": "Start the task",
                        "for_each": "trigger.node.tasks",
                        "params": {
                            "node_id": "{item.id}",
                            "properties": { "status": "in_progress" }
                        }
                    }]
                }]
            }),
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let spec = h.approved_spec().await?;
    let plan = h.create("plan", json!({})).await?;
    h.link(&plan, "spec", &spec).await?;
    let blocker = h.task().await?;
    let mut tasks = Vec::new();
    for _ in 0..3 {
        let task = h.task().await?;
        h.link(&plan, "tasks", &task).await?;
        h.link(&spec, "tasks", &task).await?;
        tasks.push(task);
    }
    // The task in the middle is blocked, so whichever order the items come
    // in, a free task follows a refused one or the refused one is last.
    let (first, blocked, last) = (&tasks[0], &tasks[1], &tasks[2]);
    h.link(&blocker, "blocks", blocked).await?;
    // The same with the blocked task first and last in creation order.
    let edge_plan = h.create("plan", json!({})).await?;
    h.link(&edge_plan, "spec", &spec).await?;
    let mut edge_tasks = Vec::new();
    for _ in 0..3 {
        let task = h.task().await?;
        h.link(&edge_plan, "tasks", &task).await?;
        h.link(&spec, "tasks", &task).await?;
        edge_tasks.push(task);
    }
    h.link(&blocker, "blocks", &edge_tasks[0]).await?;
    h.link(&blocker, "blocks", &edge_tasks[2]).await?;

    h.set(&plan, json!({ "plan_status": "approved" })).await?;
    for free in [first, last] {
        assert!(
            wait_for_status(&h, free, "in_progress").await,
            "a task the rules allow must be started although another was refused"
        );
    }
    h.set(&edge_plan, json!({ "plan_status": "approved" }))
        .await?;
    assert!(
        wait_for_status(&h, &edge_tasks[1], "in_progress").await,
        "the task between two refused ones must be started"
    );

    for refused in [blocked, &edge_tasks[0], &edge_tasks[2]] {
        assert_eq!(
            h.field(refused, "task", "status").await?,
            Some(json!("open")),
            "a blocked task stays as it was"
        );
    }
    let stored = h.service.get_node(&play).await?.expect("the play");
    assert!(
        stored.properties["play"]
            .get("suspended_reason")
            .is_none_or(|v| v.is_null()),
        "a refused item must not suspend the Play: {}",
        stored.properties["play"]
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// The model's links and when work began
// ---------------------------------------------------------------------------

/// A task reaches its decisions, and a decision the one it replaces, through
/// declared relationships that read from both ends.
#[tokio::test]
async fn decisions_link_to_specs_tasks_and_each_other() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.create("spec", json!({})).await?;
    let task = h.task().await?;
    let old = h.create("decision", json!({})).await?;
    let new = h.create("decision", json!({})).await?;
    h.link(&spec, "decisions", &new).await?;
    h.link(&task, "decisions", &new).await?;
    h.link(&new, "supersedes", &old).await?;

    let reached = |from: &str, name: &str| {
        let service = Arc::clone(&h.service);
        let from = from.to_string();
        let name = name.to_string();
        async move {
            let output = nodespace_core::ops::rel_ops::get_related_nodes(
                &service,
                nodespace_core::ops::rel_ops::GetRelatedInput {
                    node_id: from,
                    relationship_name: name,
                    direction: "out".to_string(),
                },
            )
            .await
            .expect("the relationship resolves");
            output
                .related_nodes
                .iter()
                .filter_map(|n| n["id"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(reached(&new, "specs").await, std::slice::from_ref(&spec));
    assert_eq!(reached(&new, "tasks").await, std::slice::from_ref(&task));
    assert_eq!(
        reached(&old, "superseded_by").await,
        std::slice::from_ref(&new)
    );
    assert_eq!(reached(&task, "decisions").await, [new]);
    h.stop().await;
    Ok(())
}

/// `started_at` is stamped on the first move to `in_progress` or
/// `in_review`, and only then.
#[tokio::test]
async fn started_at_is_stamped_on_the_first_start() -> Result<()> {
    let h = Harness::start().await?;
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let started = |id: &str| {
        let id = id.to_string();
        let h = &h;
        async move { h.field(&id, "task", "started_at").await }
    };

    // An open task has not started.
    let task = h.task().await?;
    assert_eq!(started(&task).await?, None);
    h.set_status(&task, "in_progress").await?;
    assert_eq!(started(&task).await?, Some(json!(today)));

    // A task put straight into review was started too.
    let reviewed = h.task().await?;
    h.set_status(&reviewed, "in_review").await?;
    assert_eq!(started(&reviewed).await?, Some(json!(today)));

    // Finishing or cancelling a task that never started stamps nothing.
    for status in ["done", "cancelled"] {
        let skipped = h.task().await?;
        h.set_status(&skipped, status).await?;
        assert_eq!(started(&skipped).await?, None, "{status}");
    }

    // Only the first start is recorded: a date already there is kept through
    // a later start, and a write that names its own date keeps that one.
    let restarted = h
        .create(
            "task",
            json!({
                "status": "open",
                "requires_spec": false,
                "started_at": "2026-01-02"
            }),
        )
        .await?;
    h.set_status(&restarted, "in_progress").await?;
    h.set_status(&restarted, "open").await?;
    h.set_status(&restarted, "in_review").await?;
    assert_eq!(started(&restarted).await?, Some(json!("2026-01-02")));

    let explicit = h.task().await?;
    h.set(
        &explicit,
        json!({ "status": "in_progress", "started_at": "2026-02-03" }),
    )
    .await?;
    assert_eq!(started(&explicit).await?, Some(json!("2026-02-03")));

    // A task created already started is stamped as it is created, on the
    // single-node path and on the batch and hierarchy ones.
    // Creation is not a move: the status rules, the spec rule among them,
    // guard a change of status, so a task can be created already started
    // with no spec. The gate's dry run still refuses it a repository write.
    let born_started = h.create("task", json!({ "status": "in_progress" })).await?;
    assert_eq!(started(&born_started).await?, Some(json!(today)));

    let batch = h
        .service
        .bulk_create(vec![Node::new(
            "task".to_string(),
            "Batch".to_string(),
            json!({ "task": { "status": "in_review" } }),
        )])
        .await?
        .remove(0);
    assert_eq!(started(&batch).await?, Some(json!(today)));

    let row = |id: &str, status: &str| {
        (
            id.to_string(),
            "task".to_string(),
            "Imported".to_string(),
            None,
            1.0,
            json!({ "status": status }),
        )
    };
    let (imported, trusted, unstarted) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    h.service
        .bulk_create_hierarchy(vec![row(&imported, "in_progress"), row(&unstarted, "open")])
        .await?;
    h.service
        .bulk_create_hierarchy_trusted(vec![row(&trusted, "in_review")])
        .await?;
    assert_eq!(started(&imported).await?, Some(json!(today)));
    assert_eq!(started(&trusted).await?, Some(json!(today)));
    assert_eq!(started(&unstarted).await?, None);
    h.stop().await;
    Ok(())
}
