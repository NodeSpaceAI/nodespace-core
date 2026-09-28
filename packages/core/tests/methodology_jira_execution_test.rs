//! What the Jira-style playbook's types and sprint gates actually do, against
//! a running engine.
//!
//! Every sprint gate is a `reject`, and a reject rule whose condition resolves
//! to nothing is indistinguishable from a working one until the write it should
//! veto is attempted. So each gate is exercised both ways: the write it must
//! refuse, and the legitimate write it must let through.

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
    /// on the write path and reactive ones run.
    async fn start() -> Result<Self> {
        let tmp = TempDir::new()?;
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);

        let playbook = playbook_by_id("jira").expect("jira playbook ships");
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

    async fn try_create(&self, node_type: &str, properties: Value) -> Result<String> {
        Ok(self
            .service
            .create_node(Node::new(
                node_type.to_string(),
                format!("A {node_type}"),
                properties,
            ))
            .await?)
    }

    async fn create(&self, node_type: &str, properties: Value) -> Result<String> {
        self.try_create(node_type, properties).await
    }

    /// Create `source -name-> target`, returning whether the write landed.
    async fn link(&self, source: &str, name: &str, target: &str) -> bool {
        self.service
            .create_relationship(source, name, target, json!({}))
            .await
            .is_ok()
    }

    /// Remove `source -name-> target`, returning whether the write landed.
    async fn unlink(&self, source: &str, name: &str, target: &str) -> bool {
        self.service
            .delete_relationship(source, name, target)
            .await
            .is_ok()
    }

    /// Apply `update` to `id`, returning whether the write landed.
    async fn apply(&self, id: &str, update: NodeUpdate) -> Result<bool> {
        let node = self.service.get_node(id).await?.expect("node exists");
        Ok(self
            .service
            .update_node(id, node.version, update)
            .await
            .is_ok())
    }

    async fn set(&self, id: &str, properties: Value) -> Result<bool> {
        self.apply(id, NodeUpdate::default().with_properties(properties))
            .await
    }

    /// A field as stored: under the node's own type bucket, or an ancestor's.
    async fn field(&self, id: &str, field: &str) -> Result<Option<Value>> {
        let node = self.service.get_node(id).await?.expect("node exists");
        let props = &node.properties;
        Ok([node.node_type.as_str(), "task"]
            .iter()
            .find_map(|bucket| props.get(*bucket).and_then(|b| b.get(field)))
            .or_else(|| props.get(field))
            .cloned())
    }

    /// What `container` holds through its `issues` relationship.
    async fn issues_of(&self, container: &str) -> Result<Vec<String>> {
        Ok(self
            .service
            .get_related_nodes(container, "issues", "out")
            .await?
            .into_iter()
            .map(|n| n.id)
            .collect())
    }

    /// The `container_type` nodes holding `issue` — the reverse read. Both
    /// containers store their edge as `issues`, so the type tells them apart.
    async fn containers_of(&self, issue: &str, container_type: &str) -> Result<Vec<String>> {
        Ok(self
            .service
            .get_related_nodes(issue, "issues", "in")
            .await?
            .into_iter()
            .filter(|n| n.node_type == container_type)
            .map(|n| n.id)
            .collect())
    }

    /// Poll until `field` on `id` satisfies `accept`, for reactive rules.
    async fn wait_for(&self, id: &str, field: &str, accept: impl Fn(&Value) -> bool) -> bool {
        for _ in 0..120 {
            if let Ok(Some(v)) = self.field(id, field).await {
                if accept(&v) {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    /// A sprint moved to `active`, with dates.
    async fn active_sprint(&self) -> Result<String> {
        let sprint = self.create("sprint", json!({})).await?;
        assert!(
            self.set(
                &sprint,
                json!({
                    "start_date": "2026-09-01",
                    "end_date": "2026-09-14",
                    "sprint_status": "active",
                }),
            )
            .await?,
            "starting a sprint with its dates in the same update must be allowed"
        );
        Ok(sprint)
    }

    /// A sprint moved all the way to `closed`.
    async fn closed_sprint(&self) -> Result<String> {
        let sprint = self.active_sprint().await?;
        assert!(
            self.set(&sprint, json!({ "sprint_status": "closed" }))
                .await?
        );
        Ok(sprint)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Each issue type is its own node type carrying its own fields, and each
/// field's declared type is enforced on write.
#[tokio::test]
async fn issue_types_carry_their_own_validated_fields() -> Result<()> {
    let h = Harness::start().await?;

    let story = h.create("story", json!({ "story_points": 5 })).await?;
    let bug = h
        .create(
            "bug",
            json!({ "severity": "major", "environment": "Safari 18, macOS 15" }),
        )
        .await?;
    let epic = h
        .create("epic", json!({ "target_date": "2026-12-01" }))
        .await?;

    for (id, node_type) in [(&story, "story"), (&bug, "bug"), (&epic, "epic")] {
        let node = h.service.get_node(id).await?.expect("exists");
        assert_eq!(node.node_type, node_type);
    }
    assert_eq!(h.field(&story, "story_points").await?, Some(json!(5)));
    assert_eq!(h.field(&bug, "severity").await?, Some(json!("major")));

    assert!(
        h.try_create("bug", json!({ "severity": "catastrophic" }))
            .await
            .is_err(),
        "severity is an enum"
    );
    assert!(
        h.try_create("story", json!({ "story_points": "large" }))
            .await
            .is_err(),
        "story_points is a number"
    );
    assert!(
        h.try_create("epic", json!({ "target_date": "next spring" }))
            .await
            .is_err(),
        "target_date is a date"
    );

    h.stop().await;
    Ok(())
}

/// One `issues` relationship per container accepts every issue type, because
/// its target is `task`. An issue has at most one epic, so a second link moves
/// it; it may sit in several sprints.
#[tokio::test]
async fn epics_and_sprints_group_every_issue_type() -> Result<()> {
    let h = Harness::start().await?;
    let epic = h.create("epic", json!({})).await?;
    let other_epic = h.create("epic", json!({})).await?;
    let sprint = h.create("sprint", json!({})).await?;
    let next_sprint = h.create("sprint", json!({})).await?;

    let task = h.create("task", json!({})).await?;
    let story = h.create("story", json!({})).await?;
    let bug = h.create("bug", json!({})).await?;

    for issue in [&task, &story, &bug] {
        assert!(
            h.link(&epic, "issues", issue).await,
            "epic.issues -> {issue}"
        );
        assert!(
            h.link(&sprint, "issues", issue).await,
            "sprint.issues -> {issue}"
        );
    }
    assert_eq!(h.issues_of(&epic).await?.len(), 3);

    assert!(h.link(&other_epic, "issues", &story).await);
    assert_eq!(
        h.containers_of(&story, "epic").await?,
        vec![other_epic.clone()]
    );
    assert_eq!(
        h.issues_of(&epic).await?.len(),
        2,
        "an issue has one epic: the second link moves it"
    );

    assert!(h.link(&next_sprint, "issues", &story).await);
    assert_eq!(
        h.containers_of(&story, "sprint").await?.len(),
        2,
        "an issue may be planned into more than one sprint"
    );

    h.stop().await;
    Ok(())
}

/// The core parent-completion Play is written against `task`, and reaches
/// stories and bugs through subtype-aware triggering with no change.
#[tokio::test]
async fn sub_tasks_complete_their_parent_across_issue_types() -> Result<()> {
    let h = Harness::start().await?;
    let story = h.create("story", json!({ "status": "open" })).await?;
    let bug = h.create("bug", json!({ "status": "open" })).await?;
    let task = h.create("task", json!({ "status": "open" })).await?;
    assert!(h.link(&story, "has_child", &bug).await);
    assert!(h.link(&story, "has_child", &task).await);

    assert!(h.set(&bug, json!({ "status": "done" })).await?);
    assert!(h.set(&task, json!({ "status": "done" })).await?);

    assert!(
        h.wait_for(&story, "status", |v| v == "done").await,
        "a story whose sub-tasks are all done must be completed"
    );

    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sprint lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sprint_cannot_start_without_both_dates() -> Result<()> {
    let h = Harness::start().await?;
    let sprint = h
        .create("sprint", json!({ "start_date": "2026-09-01" }))
        .await?;
    assert_eq!(
        h.field(&sprint, "sprint_status").await?,
        Some(json!("future"))
    );

    assert!(
        !h.set(&sprint, json!({ "sprint_status": "active" })).await?,
        "starting without an end_date must be rejected"
    );
    assert!(h.set(&sprint, json!({ "end_date": "2026-09-14" })).await?);
    assert!(
        h.set(&sprint, json!({ "sprint_status": "active" })).await?,
        "starting with both dates must be allowed"
    );

    h.stop().await;
    Ok(())
}

/// Only future → active → closed. Everything else, including every way out
/// of `closed`, is rejected.
#[tokio::test]
async fn a_sprint_moves_only_forward() -> Result<()> {
    let h = Harness::start().await?;

    let future = h
        .create(
            "sprint",
            json!({ "start_date": "2026-09-01", "end_date": "2026-09-14" }),
        )
        .await?;
    assert!(
        !h.set(&future, json!({ "sprint_status": "closed" })).await?,
        "future → closed skips the sprint and must be rejected"
    );

    let active = h.active_sprint().await?;
    assert!(
        !h.set(&active, json!({ "sprint_status": "future" })).await?,
        "active → future must be rejected"
    );

    let closed = h.closed_sprint().await?;
    for status in ["active", "future"] {
        assert!(
            !h.set(&closed, json!({ "sprint_status": status })).await?,
            "closed → {status} reopens a closed sprint and must be rejected"
        );
    }

    h.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_sprint_is_created_as_future() -> Result<()> {
    let h = Harness::start().await?;
    let dates = json!({ "start_date": "2026-09-01", "end_date": "2026-09-14" });

    for status in ["active", "closed"] {
        let mut props = dates.clone();
        props["sprint_status"] = json!(status);
        assert!(
            h.try_create("sprint", props).await.is_err(),
            "creating a sprint already {status} must be rejected"
        );
    }
    let mut props = dates.clone();
    props["completed_date"] = json!("2026-09-14");
    assert!(
        h.try_create("sprint", props).await.is_err(),
        "creating a sprint with a completed_date must be rejected"
    );
    assert!(h.try_create("sprint", dates).await.is_ok());

    h.stop().await;
    Ok(())
}

/// Closing stamps `completed_date` once, with the close's own timestamp; it
/// can never be written by hand.
#[tokio::test]
async fn closing_a_sprint_records_when_it_closed() -> Result<()> {
    let h = Harness::start().await?;

    let open = h.active_sprint().await?;
    assert!(
        !h.set(&open, json!({ "completed_date": "2026-09-14" }))
            .await?,
        "completed_date on an open sprint must be rejected"
    );

    assert!(
        !h.set(
            &open,
            json!({ "sprint_status": "closed", "completed_date": "2020-01-01" })
        )
        .await?,
        "a hand-set completed_date riding along with the close must be rejected"
    );

    assert!(h.set(&open, json!({ "sprint_status": "closed" })).await?);
    assert!(
        h.wait_for(&open, "completed_date", |v| v
            .as_str()
            .is_some_and(|s| !s.is_empty()))
            .await,
        "closing a sprint must record its completed_date"
    );

    assert!(
        !h.set(&open, json!({ "completed_date": "2020-01-01" }))
            .await?,
        "a recorded completed_date must not change"
    );

    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Close-lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_closed_sprint_keeps_its_dates_but_not_its_name_or_goal() -> Result<()> {
    let h = Harness::start().await?;
    let sprint = h.closed_sprint().await?;

    for field in ["start_date", "end_date"] {
        assert!(
            !h.set(&sprint, json!({ field: "2027-01-01" })).await?,
            "{field} of a closed sprint must be locked"
        );
    }
    assert!(
        h.set(&sprint, json!({ "goal": "Shipped the importer" }))
            .await?,
        "goal stays editable"
    );
    assert!(
        h.apply(
            &sprint,
            NodeUpdate::default().with_content("Sprint 12 (renamed)".to_string())
        )
        .await?,
        "the name stays editable"
    );

    let active = h.active_sprint().await?;
    assert!(
        h.set(&active, json!({ "end_date": "2026-09-21" })).await?,
        "an open sprint's dates stay editable"
    );

    h.stop().await;
    Ok(())
}

/// Membership of a closed sprint is locked: nothing can be added or removed,
/// and a rejected change leaves the edges exactly as they were.
#[tokio::test]
async fn a_closed_sprints_issues_are_locked() -> Result<()> {
    let h = Harness::start().await?;
    let sprint = h.active_sprint().await?;
    let kept = h.create("story", json!({})).await?;
    let late = h.create("bug", json!({})).await?;
    assert!(h.link(&sprint, "issues", &kept).await);

    assert!(h.set(&sprint, json!({ "sprint_status": "closed" })).await?);

    assert!(
        !h.link(&sprint, "issues", &late).await,
        "adding work to a closed sprint must be rejected"
    );
    assert!(
        !h.unlink(&sprint, "issues", &kept).await,
        "removing work from a closed sprint must be rejected"
    );
    assert_eq!(
        h.issues_of(&sprint).await?,
        vec![kept.clone()],
        "a rejected membership change must leave no trace"
    );

    // Carrying unfinished work forward means linking the next sprint, which
    // the lock leaves alone.
    let next = h.create("sprint", json!({})).await?;
    assert!(h.link(&next, "issues", &kept).await);

    // An open sprint's membership is unrestricted.
    let open = h.active_sprint().await?;
    assert!(h.link(&open, "issues", &late).await);
    assert!(h.unlink(&open, "issues", &late).await);

    h.stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Engine: relationship dispatch
// ---------------------------------------------------------------------------

/// Relinking an issue to a second epic evicts its edge from the first
/// (`epic` is one-per-issue). The eviction is a removal in its own right: a
/// `relationship_removed` rule on the first epic must see it, and its veto must
/// roll back the whole relink — new edge included.
#[tokio::test]
async fn a_cardinality_eviction_dispatches_its_removal() -> Result<()> {
    let h = Harness::start().await?;
    let epic = h.create("epic", json!({})).await?;
    let other = h.create("epic", json!({})).await?;
    let story = h.create("story", json!({})).await?;
    assert!(h.link(&epic, "issues", &story).await);

    h.service
        .create_node(Node::new(
            "play".to_string(),
            "Keep work in an in-progress epic".to_string(),
            json!({ "rules": [{
                "name": "reject-leaving-an-in-progress-epic",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "relationship_removed",
                    "node_type": "epic",
                },
                "conditions": ["node.status == 'in_progress'"],
                "actions": [{
                    "action_type": "reject",
                    "params": { "message": "in-progress epics keep their issues" },
                }],
            }] }),
        ))
        .await?;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(h.set(&epic, json!({ "status": "in_progress" })).await?);

    assert!(
        !h.link(&other, "issues", &story).await,
        "moving an issue out of an in-progress epic must be rejected via its eviction"
    );
    assert_eq!(h.containers_of(&story, "epic").await?, vec![epic.clone()]);
    assert!(
        h.issues_of(&other).await?.is_empty(),
        "the rejected relink's new edge must be rolled back too"
    );

    h.stop().await;
    Ok(())
}
