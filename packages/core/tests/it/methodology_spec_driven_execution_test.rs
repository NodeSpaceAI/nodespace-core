//! What the spec-driven playbook's gates actually do, against a running engine.
//!
//! Every gate here is a `reject`, and a reject rule whose condition resolves
//! to nothing is indistinguishable from a working one until the write it
//! should veto is attempted. So each gate is exercised both ways: the write it
//! must refuse, and the legitimate write it must let through — otherwise a
//! gate that rejects everything would pass as well as one that works.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::methodology::{install_playbook, playbook_by_id};
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::services::NodeService;
use nodespace_core::PlaybookEngine;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

struct Harness {
    service: Arc<NodeService>,
    _tmp: TempDir,
    shutdown: watch::Sender<bool>,
    engine: tokio::task::JoinHandle<Result<()>>,
}

impl Harness {
    /// Install the playbook and start the engine, so invariant rules are live
    /// on the write path.
    async fn start() -> Result<Self> {
        let tmp = TempDir::new()?;
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);

        let playbook = playbook_by_id("spec-driven").expect("spec-driven playbook ships");
        let report = install_playbook(&service, &playbook).await;
        assert!(report.success, "install failed: {:?}", report.failure());

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

    async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), self.engine).await;
    }

    async fn create(&self, node_type: &str, properties: Value) -> Result<String> {
        Ok(self
            .service
            .create_node(Node::new(
                node_type.to_string(),
                format!("A {node_type}"),
                properties,
            ))
            .await?)
    }

    async fn link(&self, source: &str, name: &str, target: &str) -> Result<()> {
        self.service
            .create_relationship(source, name, target, json!({}))
            .await?;
        Ok(())
    }

    /// Apply `properties` to `id`, returning whether the write landed.
    async fn set(&self, id: &str, properties: Value) -> Result<bool> {
        let node = self.service.get_node(id).await?.expect("node exists");
        Ok(self
            .service
            .update_node(
                id,
                node.version,
                NodeUpdate::default().with_properties(properties),
            )
            .await
            .is_ok())
    }

    /// An approved spec, an approved plan against it, and a task linked to
    /// both — the fully-traced starting point each task test narrows from.
    async fn traced_task(&self) -> Result<Lineage> {
        let spec = self
            .create("spec", json!({ "spec_status": "approved" }))
            .await?;
        let plan = self
            .create("plan", json!({ "plan_status": "draft" }))
            .await?;
        self.link(&plan, "spec", &spec).await?;
        assert!(
            self.set(&plan, json!({ "plan_status": "approved" }))
                .await?
        );
        let task = self.create("task", json!({ "status": "open" })).await?;
        self.link(&plan, "tasks", &task).await?;
        self.link(&spec, "tasks", &task).await?;
        Ok(Lineage { spec, plan, task })
    }
}

struct Lineage {
    spec: String,
    plan: String,
    task: String,
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// The four skills land as ordinary `skill` nodes — which is all skill search
/// and `nodespace skill guidance` need to find them — each carrying the tools
/// its guidance tells the agent to call. A skill that says "use
/// create_relationship" without that tool on its whitelist is guidance the
/// agent cannot follow.
#[tokio::test]
async fn the_four_skills_are_seeded_with_the_tools_they_direct() -> Result<()> {
    let h = Harness::start().await?;
    let skills = h.service.query_nodes_by_type("skill", false).await?;

    for (title, tool) in [
        ("Writing a Spec", "create_node"),
        ("Writing a Plan from a Spec", "create_relationship"),
        ("Creating Implementation Tasks", "create_relationship"),
        ("Completing a Spec-Driven Task", "update_task_status"),
    ] {
        let node = skills
            .iter()
            .find(|n| n.content == title)
            .unwrap_or_else(|| panic!("no seeded skill '{title}'"));
        let skill = nodespace_core::models::SkillNode::from_node(node)?;
        assert!(
            skill.tool_whitelist.iter().any(|t| t == tool),
            "'{title}' directs {tool} but its whitelist is {:?}",
            skill.tool_whitelist
        );
    }
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Plan approval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_plan_cannot_be_approved_against_a_draft_spec() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h.create("spec", json!({ "spec_status": "draft" })).await?;
    let plan = h.create("plan", json!({ "plan_status": "draft" })).await?;
    h.link(&plan, "spec", &spec).await?;

    assert!(
        !h.set(&plan, json!({ "plan_status": "approved" })).await?,
        "approving a plan whose spec is still draft must be rejected"
    );

    assert!(h.set(&spec, json!({ "spec_status": "approved" })).await?);
    assert!(
        h.set(&plan, json!({ "plan_status": "approved" })).await?,
        "once the spec is approved, the plan's approval must go through"
    );
    h.stop().await;
    Ok(())
}

/// A reject gate must fail closed on data shape. `bulk_create` applies no
/// schema defaults, so an imported spec can carry no `spec_status` at all —
/// semantically a draft. An unguarded `node.spec.spec_status` read raises
/// NoSuchKey there, the condition fails, and the reject rule lets the
/// approval through.
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
    let plan = h.create("plan", json!({ "plan_status": "draft" })).await?;
    h.link(&plan, "spec", &spec).await?;

    assert!(
        !h.set(&plan, json!({ "plan_status": "approved" })).await?,
        "a spec with no spec_status is not approved"
    );
    h.stop().await;
    Ok(())
}

/// The same shape one level down: a plan with no `plan_status` must hold its
/// tasks at `open`.
#[tokio::test]
async fn a_task_under_a_plan_with_no_status_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h
        .create("spec", json!({ "spec_status": "approved" }))
        .await?;
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
    let task = h.create("task", json!({ "status": "open" })).await?;
    h.link(&plan, "tasks", &task).await?;
    h.link(&spec, "tasks", &task).await?;

    assert!(!h.set(&task, json!({ "status": "in_progress" })).await?);
    h.stop().await;
    Ok(())
}

/// Linking some other spec does not satisfy the gate: the task must name the
/// spec its plan implements.
#[tokio::test]
async fn a_planned_task_linked_to_an_unrelated_spec_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;
    let other = h
        .create("spec", json!({ "spec_status": "approved" }))
        .await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &task).await?;
    h.link(&other, "tasks", &task).await?;

    assert!(
        !h.set(&task, json!({ "status": "in_progress" })).await?,
        "the task's spec must be the one its plan implements"
    );
    h.link(&l.spec, "tasks", &task).await?;
    assert!(
        h.set(&task, json!({ "status": "in_progress" })).await?,
        "linked to its plan's spec as well, the task may start"
    );
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_plan_with_no_spec_cannot_be_approved() -> Result<()> {
    let h = Harness::start().await?;
    let plan = h.create("plan", json!({ "plan_status": "draft" })).await?;
    assert!(
        !h.set(&plan, json!({ "plan_status": "approved" })).await?,
        "a plan with no spec link has no lineage to approve against"
    );
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_plan_cannot_be_created_already_approved() -> Result<()> {
    let h = Harness::start().await?;
    assert!(
        h.create("plan", json!({ "plan_status": "approved" }))
            .await
            .is_err(),
        "a new plan has no spec link yet, so it cannot be born approved"
    );
    assert!(h
        .create("plan", json!({ "plan_status": "draft" }))
        .await
        .is_ok());
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task lineage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fully_traced_task_can_start() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;
    assert!(h.set(&l.task, json!({ "status": "in_progress" })).await?);
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_task_under_a_draft_plan_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h
        .create("spec", json!({ "spec_status": "approved" }))
        .await?;
    let plan = h.create("plan", json!({ "plan_status": "draft" })).await?;
    h.link(&plan, "spec", &spec).await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    h.link(&plan, "tasks", &task).await?;
    h.link(&spec, "tasks", &task).await?;

    assert!(
        !h.set(&task, json!({ "status": "in_progress" })).await?,
        "a task whose plan is not approved must not start"
    );
    assert!(
        h.set(&task, json!({ "status": "cancelled" })).await?,
        "abandoning work needs no approval"
    );
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_planned_task_without_a_spec_link_cannot_start() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &task).await?;

    assert!(
        !h.set(&task, json!({ "status": "in_progress" })).await?,
        "a planned task must name its spec directly before it can start"
    );
    h.link(&l.spec, "tasks", &task).await?;
    assert!(h.set(&task, json!({ "status": "in_progress" })).await?);
    h.stop().await;
    Ok(())
}

/// A task subtype — Linear's `issue extends task` — is gated like a task.
///
/// Both directions, because a subtype can fail two opposite ways: its
/// status change never reaching the task-scoped rule (never gated), or the
/// rule projecting the linked plan through the trigger's `task` scope and
/// losing `plan_status` (always rejected).
#[tokio::test]
async fn an_issue_is_gated_like_a_task() -> Result<()> {
    let h = Harness::start().await?;
    let linear = playbook_by_id("linear").expect("linear playbook ships");
    let report = install_playbook(&h.service, &linear).await;
    assert!(
        report.success,
        "linear install failed: {:?}",
        report.failure()
    );
    tokio::time::sleep(Duration::from_millis(80)).await;

    let l = h.traced_task().await?;
    let traced = h.create("issue", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &traced).await?;
    h.link(&l.spec, "tasks", &traced).await?;
    assert!(
        h.set(&traced, json!({ "status": "in_progress" })).await?,
        "a fully traced issue must be able to start"
    );

    let untraced = h.create("issue", json!({ "status": "open" })).await?;
    h.link(&l.plan, "tasks", &untraced).await?;
    assert!(
        !h.set(&untraced, json!({ "status": "in_progress" })).await?,
        "an issue missing its spec link must not start"
    );
    h.stop().await;
    Ok(())
}

/// Installing the methodology must not change how an ordinary task behaves.
#[tokio::test]
async fn a_task_outside_the_methodology_is_never_gated() -> Result<()> {
    let h = Harness::start().await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    assert!(h.set(&task, json!({ "status": "in_progress" })).await?);
    assert!(
        h.set(&task, json!({ "status": "done" })).await?,
        "an unrelated task must close without a verification method"
    );
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification method
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_traced_task_cannot_close_without_a_verification_method() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;
    assert!(h.set(&l.task, json!({ "status": "in_progress" })).await?);

    assert!(
        !h.set(&l.task, json!({ "status": "done" })).await?,
        "closing without a verification method must be rejected"
    );
    assert!(
        h.set(
            &l.task,
            json!({ "custom:verification_method": "test: bun run test, 0 new failures" })
        )
        .await?
    );
    assert!(
        h.set(&l.task, json!({ "status": "done" })).await?,
        "with a verification method recorded, closing must go through"
    );
    h.stop().await;
    Ok(())
}

/// The gate keys on either link: a task tied only to a spec is still in the
/// methodology.
#[tokio::test]
async fn a_spec_only_task_also_needs_a_verification_method() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h
        .create("spec", json!({ "spec_status": "approved" }))
        .await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    h.link(&spec, "tasks", &task).await?;

    assert!(!h.set(&task, json!({ "status": "done" })).await?);
    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Supersession lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_superseded_spec_is_locked() -> Result<()> {
    let h = Harness::start().await?;
    let spec = h
        .create(
            "spec",
            json!({ "spec_status": "approved", "objective": "Original objective" }),
        )
        .await?;

    assert!(
        h.set(&spec, json!({ "objective": "Edited while live" }))
            .await?,
        "a live spec stays editable"
    );
    assert!(
        h.set(&spec, json!({ "spec_status": "superseded" })).await?,
        "superseding itself must not trip the lock"
    );
    assert!(
        !h.set(&spec, json!({ "objective": "Rewritten history" }))
            .await?,
        "a superseded spec's content must be locked"
    );
    assert!(
        !h.set(&spec, json!({ "spec_status": "approved" })).await?,
        "a superseded spec must not be reinstated — that would reopen the lock"
    );
    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_superseded_plan_is_locked() -> Result<()> {
    let h = Harness::start().await?;
    let l = h.traced_task().await?;

    assert!(
        h.set(&l.plan, json!({ "plan_status": "superseded" }))
            .await?
    );
    assert!(!h.set(&l.plan, json!({ "approach": "Rewritten" })).await?);
    assert!(!h.set(&l.plan, json!({ "risks": "Rewritten" })).await?);
    assert!(!h.set(&l.plan, json!({ "plan_status": "draft" })).await?);
    h.stop().await;
    Ok(())
}
