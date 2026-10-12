//! The shipped workflow as seeded data (ADR-092 §8, ADR-094): the procedure
//! skills, the queues they are attached to, and what a task carries at each
//! stage.
//!
//! The database is seeded the way the daemon seeds one and the engine is
//! running, so the seeded rules are live. The two procedures are driven
//! through `GraphToolExecutor::execute`, step by step as their bodies say, so
//! a step the tools cannot perform fails here.

use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::skill_pipeline::{
    link_seeded_skills, seed_skill_nodes, seed_tool_nodes, shipped_attached_to_for,
    IMPLEMENTING_A_TASK_SKILL_ID, SKILL_SEEDS,
};
use nodespace_agent::{AgentToolExecutor, ToolResult};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::{Node, NodeUpdate, SkillFields, SKILL_APPLIES_TO, SKILL_ATTACHED_TO};
use nodespace_core::services::query_service::core_queries::{
    AWAITING_REVIEW_QUERY_ID, CHANGES_REQUESTED_QUERY_ID, IN_PROGRESS_QUERY_ID,
    NEEDS_SHAPING_QUERY_ID, PLANS_BY_STATUS_QUERY_ID, READY_TASKS_QUERY_ID,
    SPECS_BY_STATUS_QUERY_ID, WAITING_ON_YOU_QUERY_ID,
};
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use nodespace_core::PlaybookEngine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::{watch, RwLock};

const IMPLEMENTING: &str = "Implementing a Task";
const REVIEWING: &str = "Reviewing a Task";
const ADDRESSING: &str = "Addressing Review Findings";
const CREATING: &str = "Creating a Task";
const ANSWERING: &str = "Answering a Question";

struct Harness {
    executor: GraphToolExecutor,
    ns: Arc<NodeService>,
    _tmp: TempDir,
    _shutdown: watch::Sender<bool>,
}

/// What a daemon does to the skill and tool tables on every open.
async fn seed_agent_tables(ns: &NodeService) {
    let groups: Vec<_> = seed_skill_nodes()
        .iter()
        .chain(seed_tool_nodes().iter())
        .map(|template| prepare_nodes_from_template(template).expect("the seed parses"))
        .collect();
    ns.seed_nodes_from_templates(groups).await.unwrap();
    link_seeded_skills(ns).await.unwrap();
}

async fn start() -> Harness {
    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    seed_agent_tables(&ns).await;

    let (shutdown, rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(&ns)));
    ns.set_playbook_lifecycle(engine.lifecycle().clone());
    tokio::spawn(async move { engine.start(rx).await });
    tokio::time::sleep(Duration::from_millis(80)).await;

    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    Harness {
        executor,
        ns,
        _tmp: tmp,
        _shutdown: shutdown,
    }
}

impl Harness {
    async fn call(&self, tool: &str, args: Value) -> ToolResult {
        self.executor
            .execute(tool, args)
            .await
            .unwrap_or_else(|e| panic!("{tool} must return a tool result: {e}"))
    }

    /// A call that must succeed.
    async fn ok(&self, tool: &str, args: Value) -> Value {
        let result = self.call(tool, args).await;
        assert!(!result.is_error, "{tool} failed: {}", result.result);
        result.result
    }

    /// A call that must be refused: what the model is told.
    async fn refused(&self, tool: &str, args: Value) -> String {
        match self.executor.execute(tool, args).await {
            Ok(result) => {
                assert!(result.is_error, "{tool} was not refused: {}", result.result);
                result.result.to_string()
            }
            Err(error) => error.to_string(),
        }
    }

    async fn create(&self, node_type: &str, content: &str, props: Value) -> String {
        self.ns
            .create_node(Node::new(node_type.to_string(), content.to_string(), props))
            .await
            .unwrap_or_else(|e| panic!("creating the {node_type} '{content}' failed: {e}"))
    }

    async fn child(&self, parent: &str, node_type: &str, content: &str) -> String {
        self.ns
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: node_type.to_string(),
                content: content.to_string(),
                parent_id: Some(parent.to_string()),
                position: InsertPositionOwned::End,
                properties: json!({}),
                lifecycle_status: None,
            })
            .await
            .unwrap_or_else(|e| panic!("placing '{content}' under {parent} failed: {e}"))
    }

    /// A section beneath `parent`: a `## ` header named for it, as the
    /// procedures write it, and a text node under it for each item. Written
    /// through the tools, so a step the tools cannot perform fails here.
    async fn section(&self, parent: &str, name: &str, items: &[&str]) -> String {
        let header = self
            .ok(
                "create_node",
                json!({
                    "node_type": "header",
                    "parent_id": uri(parent),
                    "content": format!("## {name}")
                }),
            )
            .await["id"]
            .as_str()
            .expect("create_node returns the id")
            .trim_start_matches("nodespace://")
            .to_string();
        for item in items {
            self.ok(
                "create_node",
                json!({ "node_type": "text", "parent_id": uri(&header), "content": item }),
            )
            .await;
        }
        header
    }

    async fn link(&self, from: &str, name: &str, to: &str) {
        self.ns
            .create_relationship(from, name, to, json!({}))
            .await
            .unwrap_or_else(|e| panic!("linking {from} -{name}-> {to} failed: {e}"));
    }

    async fn set(&self, id: &str, properties: Value) {
        let version = self.ns.get_node(id).await.unwrap().unwrap().version;
        self.ns
            .update_node(id, version, NodeUpdate::new().with_properties(properties))
            .await
            .unwrap_or_else(|e| panic!("updating {id} failed: {e}"));
    }

    async fn context(&self, id: &str) -> Value {
        self.ok("get_node_context", json!({ "id": uri(id) })).await
    }

    /// The stored status of a task or an issue. An issue keeps its status in
    /// its own bucket, since it declares the field again to add statuses.
    async fn status(&self, task: &str) -> String {
        let node = self.ns.get_node(task).await.unwrap().unwrap();
        ["issue", "task"]
            .iter()
            .find_map(|bucket| node.properties[*bucket]["status"].as_str())
            .unwrap()
            .to_string()
    }
}

struct Work {
    project: String,
    spec: String,
    plan: String,
    task_decision: String,
    spec_decision: String,
    task: String,
}

/// A project's task under an approved spec and plan, with a decision linked
/// from the task and another from its spec, and a two-item checklist.
async fn planned_task(h: &Harness) -> Work {
    planned_work(h, "project", "task").await
}

/// [`planned_task`] for a code change: an issue of a software project.
async fn planned_issue(h: &Harness) -> Work {
    planned_work(h, "software-project", "issue").await
}

async fn planned_work(h: &Harness, project_type: &str, task_type: &str) -> Work {
    let project = h.create(project_type, "Apollo", json!({})).await;
    let spec = h.create("spec", "Export", json!({})).await;
    h.child(&spec, "checkbox", "- [ ] Exports finish in a second")
        .await;
    let plan = h.create("plan", "Streamed export", json!({})).await;
    h.link(&plan, "spec", &spec).await;
    h.set(&spec, json!({ "spec_status": "approved" })).await;
    h.set(&plan, json!({ "plan_status": "approved" })).await;

    let task_decision = h.create("decision", "Rows are streamed", json!({})).await;
    let spec_decision = h.create("decision", "CSV only", json!({})).await;
    h.link(&spec, "decisions", &spec_decision).await;

    let task = h.create(task_type, "Stream the rows", json!({})).await;
    h.child(&task, "checkbox", "- [ ] Rows are streamed").await;
    h.child(&task, "checkbox", "- [ ] A test covers 10,000 rows")
        .await;
    h.link(&project, "tasks", &task).await;
    h.link(&plan, "tasks", &task).await;
    h.link(&spec, "tasks", &task).await;
    h.link(&task, "decisions", &task_decision).await;

    Work {
        project,
        spec,
        plan,
        task_decision,
        spec_decision,
        task,
    }
}

/// An id without the `nodespace://` scheme a tool result names it with.
fn bare(id: &str) -> String {
    id.strip_prefix("nodespace://").unwrap_or(id).to_string()
}

fn uri(id: &str) -> String {
    format!("nodespace://{id}")
}

fn skill_names(read: &Value) -> Vec<&str> {
    read["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .map(|skill| skill["name"].as_str().unwrap())
        .collect()
}

/// The ids each of a read's paths reached, keyed by the dotted path.
fn reached(paths: &Value) -> Vec<(String, Vec<String>)> {
    paths
        .as_array()
        .expect("paths")
        .iter()
        .map(|group| {
            let path: Vec<&str> = group["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap())
                .collect();
            let ids = group["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| node["id"].as_str().unwrap().to_string())
                .collect();
            (path.join("."), ids)
        })
        .collect()
}

#[tokio::test]
async fn the_ten_procedures_are_seeded_with_their_links() {
    let h = start().await;
    // A second open changes nothing.
    seed_agent_tables(&h.ns).await;

    let titles = [
        "Writing a Spec",
        "Reviewing a Plan",
        "Splitting a Plan into Tasks",
        "Asking the User",
        "Explaining",
        IMPLEMENTING,
        REVIEWING,
        ADDRESSING,
        "Completing a Task",
        "Recording a Decision",
        "Authoring a Skill",
        CREATING,
        ANSWERING,
    ];
    for title in titles {
        let seed = SKILL_SEEDS
            .iter()
            .find(|seed| seed.title == title)
            .unwrap_or_else(|| panic!("{title} is not in the seed table"));
        assert!(uuid::Uuid::parse_str(seed.id).is_ok(), "{title}");

        let node = h.ns.get_node(seed.id).await.unwrap().expect(title);
        assert_eq!(node.node_type, "skill");
        assert_eq!(node.content, title);
        assert!(
            !h.ns.get_children(seed.id).await.unwrap().is_empty(),
            "{title} has no body"
        );
        // Asking the User and Explaining are about no type.
        if title != "Asking the User" && title != "Explaining" {
            assert!(!seed.applies_to.is_empty(), "{title} is linked to no type");
        }

        for (relationship, named) in [
            (SKILL_APPLIES_TO, seed.applies_to),
            (SKILL_ATTACHED_TO, seed.attached_to),
        ] {
            let links =
                h.ns.store()
                    .get_edge_targets_by_source(&[seed.id.to_string()], relationship)
                    .await
                    .unwrap();
            let mut linked = links.get(seed.id).cloned().unwrap_or_default();
            linked.sort();
            let mut named: Vec<String> = named.iter().map(|id| id.to_string()).collect();
            named.sort();
            assert_eq!(linked, named, "{title} {relationship}");
        }
    }

    let attached = |title: &str| {
        SKILL_SEEDS
            .iter()
            .find(|seed| seed.title == title)
            .unwrap()
            .attached_to
    };
    assert_eq!(
        attached(IMPLEMENTING),
        [READY_TASKS_QUERY_ID, IN_PROGRESS_QUERY_ID]
    );
    assert_eq!(attached(REVIEWING), [AWAITING_REVIEW_QUERY_ID]);
    assert_eq!(attached(ADDRESSING), [CHANGES_REQUESTED_QUERY_ID]);
    assert_eq!(attached("Writing a Spec"), [SPECS_BY_STATUS_QUERY_ID]);
    assert_eq!(attached("Reviewing a Plan"), [PLANS_BY_STATUS_QUERY_ID]);
    assert_eq!(
        attached("Splitting a Plan into Tasks"),
        [PLANS_BY_STATUS_QUERY_ID]
    );
    assert_eq!(
        attached("Asking the User"),
        [SPECS_BY_STATUS_QUERY_ID, PLANS_BY_STATUS_QUERY_ID]
    );
    assert_eq!(attached(CREATING), [NEEDS_SHAPING_QUERY_ID]);
    assert_eq!(attached(ANSWERING), [WAITING_ON_YOU_QUERY_ID]);
    // No other built-in is attached to anything.
    let attached_titles = [
        IMPLEMENTING,
        REVIEWING,
        ADDRESSING,
        CREATING,
        ANSWERING,
        "Writing a Spec",
        "Reviewing a Plan",
        "Splitting a Plan into Tasks",
        "Asking the User",
    ];
    for seed in SKILL_SEEDS {
        if !attached_titles.contains(&seed.title) {
            assert!(seed.attached_to.is_empty(), "{}", seed.title);
        }
    }
}

/// Every procedure the planning skills depend on reaches an agent with the
/// work: it is attached to a queue, or a skill that is names it, and so on
/// down the chain. None depends on the agent searching for it.
#[test]
fn every_planning_skill_arrives_with_the_work() {
    let body = |title: &str| {
        SKILL_SEEDS
            .iter()
            .find(|seed| seed.title == title)
            .unwrap_or_else(|| panic!("{title} is not in the seed table"))
            .body
    };
    let attached = |title: &str| {
        !SKILL_SEEDS
            .iter()
            .find(|seed| seed.title == title)
            .unwrap()
            .attached_to
            .is_empty()
    };
    for title in [
        "Writing a Spec",
        "Reviewing a Plan",
        "Splitting a Plan into Tasks",
    ] {
        assert!(attached(title), "{title} is attached to no queue");
        assert!(
            body(title).contains("Asking the User"),
            "{title} does not name Asking the User"
        );
    }
    // The delivery procedures reach an agent with the work: each is attached
    // to the queue of its stage, and the implementing one names the one that
    // takes over when a review sends findings back.
    for title in [IMPLEMENTING, REVIEWING, ADDRESSING] {
        assert!(attached(title), "{title} is attached to no queue");
    }
    assert!(
        body(IMPLEMENTING).contains(ADDRESSING),
        "Implementing a Task does not name {ADDRESSING}"
    );
    // Asking the User is attached, and names Explaining.
    assert!(attached("Asking the User"));
    assert!(
        body("Asking the User").contains("Explaining"),
        "Asking the User does not name Explaining"
    );
}

/// An adapted skill speaks NodeSpace and nothing else: no harness skill
/// tool, slash command, relative file link, issue tracker or label.
#[test]
fn the_planning_skills_name_no_harness_tracker_or_link() {
    let planning = [
        "Writing a Spec",
        "Reviewing a Plan",
        "Splitting a Plan into Tasks",
        "Asking the User",
        "Explaining",
        IMPLEMENTING,
        REVIEWING,
        ADDRESSING,
    ];
    // Built from fragments so this list is not itself a hit for a checker.
    let forbidden = [
        "Skill tool",
        "slash command",
        "](",
        ".md",
        "GitHub",
        "issue tracker",
        "label",
        "Linear",
        "Jira",
        "ready-for-agent",
        "setup-matt",
    ];
    for title in planning {
        let seed = SKILL_SEEDS.iter().find(|seed| seed.title == title).unwrap();
        // What each audience reads: the rules are rendered into both forms.
        for (form, text) in [
            ("agent", seed.template().markdown_content),
            ("external", seed.external_body()),
        ] {
            // A slash command is a word that opens with a slash.
            for word in text.split_whitespace() {
                assert!(
                    !(word.starts_with('/') && word.len() > 1),
                    "{title} ({form}) names a slash command: {word}"
                );
            }
            for needle in forbidden {
                assert!(
                    !text.contains(needle),
                    "{title} ({form}) contains {needle:?}"
                );
            }
            // A skill that was replaced is named by no one.
            for gone in [
                "Writing a Plan",
                "Breaking a Plan into Tasks",
                "Splitting into Issues",
            ] {
                assert!(!text.contains(gone), "{title} ({form}) names {gone:?}");
            }
        }
    }

    // The splitting steps name the relationships the tools write.
    let splitting = SKILL_SEEDS
        .iter()
        .find(|seed| seed.title == "Splitting a Plan into Tasks")
        .unwrap();
    let rendered = splitting.template().markdown_content;
    for needle in ["\"tasks\"", "\"blocks\"", "\"issue\"", "\"header\""] {
        assert!(rendered.contains(needle), "splitting never names {needle}");
    }
}

/// A spec and a plan each carry the procedure for working them, and the
/// splitting procedure's steps can be followed through the tools: issues
/// that link the plan and the spec, each with a brief and its own checklist,
/// the later one blocked by the first, and only the first one ready.
#[tokio::test]
async fn a_plan_is_split_into_linked_issues_with_briefs_and_checklists() {
    let h = start().await;
    let spec = h.create("spec", "Export", json!({})).await;
    h.child(&spec, "checkbox", "- [ ] Exports finish in a second")
        .await;
    let spec_read = h.context(&spec).await;
    let mut on_spec = skill_names(&spec_read);
    on_spec.sort_unstable();
    assert_eq!(
        on_spec,
        ["Asking the User", "Writing a Spec"],
        "a spec carries the spec-writing procedure and asking the user"
    );
    let plan = h.create("plan", "Streamed export", json!({})).await;
    h.link(&plan, "spec", &spec).await;
    let plan_read = h.context(&plan).await;
    let mut on_plan = skill_names(&plan_read);
    on_plan.sort_unstable();
    assert_eq!(
        on_plan,
        [
            "Asking the User",
            "Reviewing a Plan",
            "Splitting a Plan into Tasks"
        ],
        "a plan carries the review, the splitting and asking the user"
    );
    h.set(&spec, json!({ "spec_status": "approved" })).await;
    h.set(&plan, json!({ "plan_status": "approved" })).await;

    // Each slice, in dependency order: the issue, its brief, its checklist,
    // its links.
    let mut issues = Vec::new();
    for (title, criterion) in [
        ("Stream the rows", "- [ ] Rows are streamed"),
        (
            "Offer the export in the menu",
            "- [ ] The menu offers the export",
        ),
    ] {
        let created = h
            .ok(
                "create_node",
                json!({ "node_type": "issue", "content": title }),
            )
            .await;
        let issue = created["id"]
            .as_str()
            .unwrap()
            .trim_start_matches("nodespace://")
            .to_string();
        for (section, item) in [
            ("Behaviour", "Rows are written as they are read"),
            ("Interfaces", "The exporter takes a row source"),
            ("Out of scope", "Other formats"),
        ] {
            h.section(&issue, section, &[item]).await;
        }
        h.ok(
            "create_node",
            json!({ "node_type": "checkbox", "parent_id": uri(&issue), "content": criterion }),
        )
        .await;
        for owner in [&plan, &spec] {
            h.ok(
                "create_relationship",
                json!({
                    "from_id": uri(owner),
                    "relationship_type": "tasks",
                    "to_id": uri(&issue)
                }),
            )
            .await;
        }
        issues.push(issue);
    }
    h.ok(
        "create_relationship",
        json!({
            "from_id": uri(&issues[0]),
            "relationship_type": "blocks",
            "to_id": uri(&issues[1])
        }),
    )
    .await;

    // Each issue is its own checklist beneath a brief, and is linked to the
    // spec and the plan.
    for issue in &issues {
        let children = h.ns.get_children(issue).await.unwrap();
        assert_eq!(
            children
                .iter()
                .filter(|c| c.node_type == "checkbox")
                .count(),
            1
        );
        // The brief is three header sections, each with a text node beneath.
        let sections: Vec<_> = children
            .iter()
            .filter(|c| c.node_type == "header")
            .collect();
        assert_eq!(sections.len(), 3);
        for section in sections {
            assert!(section.content.starts_with("## "), "{}", section.content);
            let items = h.ns.get_children(&section.id).await.unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].node_type, "text");
        }
        let read = h
            .ok(
                "get_node_context",
                json!({ "id": uri(issue), "paths": [["spec"], ["plan"]] }),
            )
            .await;
        let paths = reached(&read["paths"]);
        assert!(
            paths.contains(&("spec".to_string(), vec![uri(&spec)])),
            "{read}"
        );
        assert!(
            paths.contains(&("plan".to_string(), vec![uri(&plan)])),
            "{read}"
        );
    }

    // Readiness is the queue, not a mark: only the unblocked issue is listed.
    let run = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true }),
        )
        .await;
    let listed: Vec<&str> = run["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [uri(&issues[0])], "{run}");
}

/// A run of "Ready tasks" with context hands over the top task whole, and
/// the task carries the procedure of the stage it is in from then on.
#[tokio::test]
async fn a_task_carries_the_procedure_of_the_stage_it_is_in() {
    let h = start().await;
    let work = planned_task(&h).await;

    let run = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true, "limit": 1 }),
        )
        .await;
    assert_eq!(run["count"], 1, "{run}");
    let item = &run["items"][0];
    assert_eq!(item["id"], uri(&work.task));

    // Its governing nodes: the paths the task schema is seeded with.
    assert_eq!(
        reached(&item["paths"]),
        [
            ("spec".to_string(), vec![uri(&work.spec)]),
            ("plan".to_string(), vec![uri(&work.plan)]),
            ("decisions".to_string(), vec![uri(&work.task_decision)]),
            ("spec.decisions".to_string(), vec![uri(&work.spec_decision)]),
            ("project".to_string(), vec![uri(&work.project)]),
        ]
    );
    // The spec comes with its criteria.
    assert_eq!(
        item["paths"][0]["nodes"][0]["checkboxes"][0]["content"],
        "- [ ] Exports finish in a second"
    );
    // And the implementing procedure, with its instructions.
    assert_eq!(skill_names(&run), [IMPLEMENTING]);
    assert!(
        run["skills"][0]["instructions"]
            .as_str()
            .unwrap()
            .contains("STEP 1, THE USER CHOOSES THE WORK"),
        "{run}"
    );

    // Started: it left "Ready tasks", and still carries the procedure.
    h.ok(
        "update_task_status",
        json!({ "id": uri(&work.task), "status": "in_progress", "version": item["node_version"] }),
    )
    .await;
    let started = h.context(&work.task).await;
    assert_eq!(skill_names(&started), [IMPLEMENTING]);
    assert_eq!(
        started["skills"][0]["matched_queries"][0]["title"],
        "In progress"
    );

    // In review, a plain task is in no queue: Awaiting review lists issues,
    // so it carries no procedure and is reviewed when the user names it.
    h.ok(
        "update_task_status",
        json!({ "id": uri(&work.task), "status": "in_review" }),
    )
    .await;
    assert!(skill_names(&h.context(&work.task).await).is_empty());
}

/// A team's standard is a skill the user attaches to a project. Every task
/// of that project carries it, with nothing seeded changed.
#[tokio::test]
async fn a_skill_attached_to_a_project_comes_with_each_of_its_tasks() {
    let h = start().await;
    let work = planned_task(&h).await;
    // A small change: ready with a checklist of its own and no spec.
    let second = h.create("task", "Document the export", json!({})).await;
    h.child(&second, "checkbox", "- [ ] The guide covers it")
        .await;
    h.link(&work.project, "tasks", &second).await;

    let standard =
        h.ns.create_node(
            SkillFields::new("Naming conventions for Apollo.", &["get_node"], 2)
                .into_node("Apollo Naming"),
        )
        .await
        .unwrap();
    h.child(&standard, "text", "Name things plainly.").await;
    h.link(&standard, SKILL_ATTACHED_TO, &work.project).await;

    for task in [&work.task, &second] {
        let read = h.ok("get_node_context", json!({ "id": uri(task) })).await;
        assert_eq!(skill_names(&read), [IMPLEMENTING, "Apollo Naming"]);
    }
    // A task of no project does not.
    let other = h.create("task", "Unrelated", json!({})).await;
    h.child(&other, "checkbox", "- [ ] Done").await;
    let read = h.ok("get_node_context", json!({ "id": uri(&other) })).await;
    assert_eq!(skill_names(&read), [IMPLEMENTING]);
}

/// "Implementing a Task" and then "Reviewing a Task", each step made as its
/// body says and with the versions the reads returned, under the live rules.
/// The work is a code change, so the task is an issue: the procedures are
/// written against `task` and reach it, and step 6 records where it landed.
#[tokio::test]
async fn the_two_procedures_can_be_followed_step_by_step_through_the_tools() {
    let h = start().await;
    let work = planned_issue(&h).await;
    let task = uri(&work.task);

    // -- Implementing --
    // Step 1: list what is ready, take nothing; the user chooses, and the
    // chosen task is read with what governs it.
    let listed = h
        .ok("run_query", json!({ "query": "Ready tasks", "limit": 10 }))
        .await;
    assert_eq!(listed["nodes"][0]["id"], task, "{listed}");
    assert_eq!(h.status(&work.task).await, "open", "listing starts nothing");
    let run = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true, "limit": 1 }),
        )
        .await;
    let item = &run["items"][0];
    assert_eq!(item["id"], task);

    // Step 2: start it with the version read. The same version is refused
    // afterwards: the task has moved.
    let start = json!({ "id": task, "status": "in_progress", "version": item["node_version"] });
    h.ok("update_task_status", start.clone()).await;
    let second_start = h.refused("update_task_status", start).await;
    assert!(
        second_start.contains("has changed since it was read"),
        "{second_start}"
    );
    assert_eq!(h.status(&work.task).await, "in_progress");

    // Step 5: tick each item with the version read, evidence beneath it.
    for checkbox in item["checkboxes"].as_array().unwrap() {
        let text = checkbox["content"].as_str().unwrap();
        h.ok(
            "update_node",
            json!({
                "id": checkbox["id"],
                "content": text.replacen("- [ ] ", "- [x] ", 1),
                "version": checkbox["node_version"],
            }),
        )
        .await;
        h.ok(
            "create_node",
            json!({
                "node_type": "text",
                "parent_id": checkbox["id"],
                "content": "cargo test export:: printed 4 passed",
            }),
        )
        .await;
    }

    // Step 6: the pull request and the commits.
    h.ok(
        "update_node",
        json!({ "id": task, "field_values": {
            "pull_request": { "title": "Stream the export", "url": "https://example.com/pr/7" },
            "commits": [{ "title": "Stream rows", "url": "https://example.com/c/abc" }],
        } }),
    )
    .await;

    // Step 7: read the version now, and move it to review with it.
    let now = h.ok("get_node_context", json!({ "id": task })).await;
    h.ok(
        "update_task_status",
        json!({ "id": task, "status": "in_review", "version": now["node"]["node_version"] }),
    )
    .await;
    assert_eq!(h.status(&work.task).await, "in_review");

    // -- Reviewing --
    // Step 1: take a task awaiting review.
    let run = h
        .ok(
            "run_query",
            json!({ "query": "Awaiting review", "with_context": true, "limit": 1 }),
        )
        .await;
    let item = &run["items"][0];
    assert_eq!(item["id"], task);
    assert_eq!(skill_names(&run), [REVIEWING]);
    assert_eq!(
        item["properties"]["pull_request"]["url"], "https://example.com/pr/7",
        "{item}"
    );

    // Step 2: the checklist with the evidence under each item.
    let read = h
        .ok("get_node", json!({ "id": task, "format": "markdown" }))
        .await;
    let markdown = read["markdown"].as_str().unwrap_or_default();
    assert!(markdown.contains("- [x] Rows are streamed"), "{read}");
    assert!(markdown.contains("printed 4 passed"), "{read}");

    // Step 4: the outcome, as a note under the task.
    h.ok(
        "create_node",
        json!({ "node_type": "text", "parent_id": task, "content": "Reviewed: both items hold." }),
    )
    .await;

    // Step 9: an issue that passed is ready to merge, not done, with the
    // version this read returned. It is listed to be landed, and is no
    // longer awaiting review.
    h.ok(
        "update_task_status",
        json!({ "id": task, "status": "ready_to_merge", "version": item["node_version"] }),
    )
    .await;
    assert_eq!(h.status(&work.task).await, "ready_to_merge");
    let awaiting = h
        .ok("run_query", json!({ "query": "Awaiting review" }))
        .await;
    assert_eq!(awaiting["count"], 0, "{awaiting}");
    let landing = h
        .ok("run_query", json!({ "query": "Ready to merge" }))
        .await;
    assert_eq!(landing["nodes"][0]["id"], task, "{landing}");

    // The change landed: done is set then.
    h.set(&work.task, json!({ "status": "done" })).await;
    assert_eq!(h.status(&work.task).await, "done");
}

/// `update_task_status` holds a status to the node's own type: an issue takes
/// the two statuses it adds to a task's, and a plain task refuses them, with
/// the statuses a task has named.
#[tokio::test]
async fn update_task_status_takes_an_issues_own_statuses_on_an_issue_only() {
    let h = start().await;
    let issue = h.create("issue", "Stream the rows", json!({})).await;
    let task = h.create("task", "Book the venue", json!({})).await;

    for status in [
        "in_review",
        "changes_requested",
        "in_review",
        "ready_to_merge",
    ] {
        h.ok(
            "update_task_status",
            json!({ "id": uri(&issue), "status": status }),
        )
        .await;
        assert_eq!(h.status(&issue).await, status);
    }

    for status in ["changes_requested", "ready_to_merge"] {
        let refused = h
            .refused(
                "update_task_status",
                json!({ "id": uri(&task), "status": status }),
            )
            .await;
        assert!(
            refused.contains("Must be one of: open, in_progress, in_review, done, cancelled"),
            "{refused}"
        );
        assert_eq!(h.status(&task).await, "open");
    }
    // A value neither type has is refused on an issue with the issue's values.
    let refused = h
        .refused(
            "update_task_status",
            json!({ "id": uri(&issue), "status": "triage" }),
        )
        .await;
    assert!(refused.contains("ready_to_merge"), "{refused}");
}

/// Writing a Spec, step by step through the tools: the spec is created without
/// a status and so is a draft, each criterion is an unchecked checkbox under
/// it, and approving one with no criterion is rejected. Approval is the
/// user's, so nothing here sets it before the criteria are in.
#[tokio::test]
async fn a_spec_is_written_as_a_draft_with_checkbox_criteria() {
    let h = start().await;
    let created = h
        .ok(
            "create_node",
            json!({
                "node_type": "spec",
                "content": "Export",
                "field_values": {
                    "objective": "Let a user export a project as CSV",
                    "boundaries": "Never include archived records"
                }
            }),
        )
        .await;
    let spec = created["id"]
        .as_str()
        .expect("create_node returns the id")
        .trim_start_matches("nodespace://")
        .to_string();
    let stored = h.ns.get_node(&spec).await.unwrap().unwrap();
    assert_eq!(stored.properties["spec"]["spec_status"], "draft");

    // The spec has no criterion yet, so an approval is rejected.
    let version = stored.version;
    let rejected = h
        .refused(
            "update_node",
            json!({
                "id": uri(&spec),
                "version": version,
                "field_values": { "spec_status": "approved" }
            }),
        )
        .await;
    assert!(rejected.to_lowercase().contains("checkbox"), "{rejected}");

    for criterion in [
        "- [ ] The export finishes in under a second for 10,000 rows",
        "- [ ] Archived records are left out",
    ] {
        h.ok(
            "create_node",
            json!({ "node_type": "checkbox", "parent_id": uri(&spec), "content": criterion }),
        )
        .await;
    }
    // The sections are ordinary nodes beside the criteria, not criteria.
    for (section, items) in [
        (
            "User stories",
            &["As a user, I want to export a project, so that I can share it"][..],
        ),
        (
            "Testing decisions",
            &[
                "Tested at: the export command",
                "Only visible behaviour is checked",
            ][..],
        ),
        ("Out of scope", &["Other formats"][..]),
    ] {
        h.section(&spec, section, items).await;
    }
    let children = h.ns.get_children(&spec).await.unwrap();
    let criteria: Vec<_> = children
        .iter()
        .filter(|c| c.node_type == "checkbox")
        .collect();
    assert_eq!(criteria.len(), 2);
    assert!(criteria.iter().all(|c| c.content.starts_with("- [ ] ")));
    assert_eq!(
        children.iter().filter(|c| c.node_type == "header").count(),
        3
    );
    let still_draft = h.ns.get_node(&spec).await.unwrap().unwrap();
    assert_eq!(still_draft.properties["spec"]["spec_status"], "draft");

    // With its criteria in, and its sections beside them, it can be
    // approved: the user's call, made here as the user's write.
    h.ok(
        "update_node",
        json!({
            "id": uri(&spec),
            "version": still_draft.version,
            "field_values": { "spec_status": "approved" }
        }),
    )
    .await;
}

/// Only direct checkbox children are a checklist. A checkbox beneath a
/// section header is content: it is no criterion for readiness, and an
/// unticked one does not stop the direct checklist closing the task.
#[tokio::test]
async fn a_checkbox_beneath_a_section_is_not_a_checklist_item() {
    let h = start().await;

    // An issue whose only checkbox sits under a header has no checklist, so
    // it is not ready.
    let hidden = h.create("issue", "Hidden checklist", json!({})).await;
    let section = h.section(&hidden, "Acceptance", &[]).await;
    h.child(&section, "checkbox", "- [ ] Not a criterion").await;
    let run = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true }),
        )
        .await;
    assert_eq!(run["count"], 0, "{run}");

    // With a direct checklist it is ready, and the nested gap does not stop
    // it being closed once the direct item is ticked.
    h.child(&hidden, "checkbox", "- [x] The change landed")
        .await;
    let run = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true }),
        )
        .await;
    assert_eq!(run["items"][0]["id"], uri(&hidden), "{run}");
    for status in ["in_progress", "in_review", "done"] {
        h.ok(
            "update_task_status",
            json!({ "id": uri(&hidden), "status": status }),
        )
        .await;
    }
    assert_eq!(h.status(&hidden).await, "done");
}

/// The sections a procedure writes are read back whole by the rule the
/// reading procedures include, in order, which a context read does not do.
#[tokio::test]
async fn a_specs_sections_are_read_back_with_the_markdown_read() {
    let h = start().await;
    let spec = h.create("spec", "Export", json!({})).await;
    h.child(&spec, "checkbox", "- [ ] Exports finish in a second")
        .await;
    h.section(
        &spec,
        "Testing decisions",
        &["Tested at: the export command"],
    )
    .await;
    h.section(&spec, "Out of scope", &["Other formats"]).await;

    let read = h
        .ok(
            "get_node",
            json!({ "id": uri(&spec), "format": "markdown" }),
        )
        .await;
    let text = read.to_string();
    for needle in [
        "## Testing decisions",
        "Tested at: the export command",
        "## Out of scope",
        "Other formats",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
    assert!(
        text.find("## Testing decisions") < text.find("## Out of scope"),
        "{text}"
    );
}

/// A real spec is several kilobytes of sections. The markdown read returns
/// all of it, the last section and a gap in a plan's failure modes included,
/// and an issue's brief reads back the same way.
#[tokio::test]
async fn a_long_record_is_read_back_whole_with_its_last_section() {
    let h = start().await;
    let spec = h.create("spec", "Export", json!({})).await;
    let stories: Vec<String> = (1..=40)
        .map(|n| {
            format!(
                "As user {n}, I want to export project {n} as a CSV file, so that I can \
                 share it with the people I work with"
            )
        })
        .collect();
    let story_refs: Vec<&str> = stories.iter().map(String::as_str).collect();
    h.section(&spec, "User stories", &story_refs).await;
    h.section(&spec, "Out of scope", &["Other formats"]).await;
    h.child(&spec, "checkbox", "- [ ] The last criterion").await;

    let text = h
        .ok(
            "get_node",
            json!({ "id": uri(&spec), "format": "markdown" }),
        )
        .await
        .to_string();
    assert!(text.len() > 4_000, "the fixture must exceed the search cut");
    for needle in [
        "As user 40",
        "## Out of scope",
        "Other formats",
        "The last criterion",
    ] {
        assert!(text.contains(needle), "{needle} cut from the read");
    }
    assert!(!text.contains("[truncated]"), "{text}");

    let plan = h.create("plan", "Streamed export", json!({})).await;
    let failures = h
        .section(&plan, "Failure modes", &["A write fails half way"])
        .await;
    h.child(&failures, "checkbox", "- [ ] GAP: wide rows are untested")
        .await;
    let issue = h.create("issue", "Stream the rows", json!({})).await;
    h.section(&issue, "Behaviour", &["Rows are written as they are read"])
        .await;
    for (id, needles) in [
        (
            &plan,
            [
                "## Failure modes",
                "A write fails half way",
                "GAP: wide rows are untested",
            ],
        ),
        (
            &issue,
            [
                "## Behaviour",
                "Rows are written as they are read",
                "Behaviour",
            ],
        ),
    ] {
        let text = h
            .ok("get_node", json!({ "id": uri(id), "format": "markdown" }))
            .await
            .to_string();
        for needle in needles {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }
}

/// A plan's review is ordinary nodes beneath it: a Not in scope and a Failure
/// modes section, with a gap as a checkbox until it is closed. None of it is
/// a criterion, so the plan is approved on its spec alone.
#[tokio::test]
async fn a_plans_review_is_ordinary_nodes_and_gates_nothing() {
    let h = start().await;
    let spec = h.create("spec", "Export", json!({})).await;
    h.child(&spec, "checkbox", "- [ ] Exports finish in a second")
        .await;
    h.set(&spec, json!({ "spec_status": "approved" })).await;
    let plan = h
        .ok(
            "create_node",
            json!({
                "node_type": "plan",
                "content": "Streamed export",
                "field_values": {
                    "approach": "Stream rows to the file as they are read",
                    "risks": "A very wide row exhausts the buffer"
                }
            }),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string();
    h.link(&plan, "spec", &spec).await;

    h.section(&plan, "Not in scope", &["Other formats: CSV is the ask"])
        .await;
    let failures = h
        .section(
            &plan,
            "Failure modes",
            &["A write fails half way: handled, a test covers it, the user sees it"],
        )
        .await;
    h.ok(
        "create_node",
        json!({
            "node_type": "checkbox",
            "parent_id": uri(&failures),
            "content": "- [ ] GAP: a wide row has no test and no handling"
        }),
    )
    .await;

    // The gap is a checkbox beneath a section, not a direct child, so it
    // is not a criterion of anything; the plan is approved on its spec.
    let sections = h.ns.get_children(&plan).await.unwrap();
    assert_eq!(sections.len(), 2);
    assert!(sections.iter().all(|c| c.node_type == "header"));
    h.set(&plan, json!({ "plan_status": "approved" })).await;
    assert_eq!(
        h.ns.get_node(&plan).await.unwrap().unwrap().properties["plan"]["plan_status"],
        "approved"
    );
}

/// A review that finds an item unmet unticks it, says why beneath it, and
/// sends the task back, where it carries the implementing procedure again.
#[tokio::test]
async fn a_review_that_fails_sends_the_task_back_to_the_implementer() {
    let h = start().await;
    let work = planned_task(&h).await;
    let task = uri(&work.task);
    for checkbox in h.ns.get_children(&work.task).await.unwrap() {
        let ticked = checkbox.content.replacen("- [ ] ", "- [x] ", 1);
        h.ns.update_node(
            &checkbox.id,
            checkbox.version,
            NodeUpdate::new().with_content(ticked),
        )
        .await
        .unwrap();
    }
    h.set(&work.task, json!({ "status": "in_progress" })).await;
    h.set(&work.task, json!({ "status": "in_review" })).await;

    let read = h.ok("get_node_context", json!({ "id": task })).await;
    let unmet = &read["node"]["checkboxes"][1];
    h.ok(
        "update_node",
        json!({ "id": unmet["id"], "content": "- [ ] A test covers 10,000 rows" }),
    )
    .await;
    h.ok(
        "create_node",
        json!({ "node_type": "text", "parent_id": unmet["id"], "content": "The test stops at 100 rows." }),
    )
    .await;
    h.ok(
        "update_task_status",
        json!({ "id": task, "status": "in_progress", "version": read["node"]["node_version"] }),
    )
    .await;

    assert_eq!(h.status(&work.task).await, "in_progress");
    let back = h.ok("get_node_context", json!({ "id": task })).await;
    assert_eq!(skill_names(&back), [IMPLEMENTING]);
    // And it cannot be finished while the item is unticked.
    let done = h
        .refused(
            "update_task_status",
            json!({ "id": task, "status": "done" }),
        )
        .await;
    assert!(done.contains("unchecked item"), "{done}");
}

/// ADR-072: an edit to a seeded skill's body is kept by the next open, and a
/// reset restores what ships.
#[tokio::test]
async fn an_edited_procedure_survives_an_open_and_a_reset_restores_it() {
    let h = start().await;
    let body = |h: &Harness| {
        let ns = h.ns.clone();
        async move {
            ns.get_children(IMPLEMENTING_A_TASK_SKILL_ID)
                .await
                .unwrap()
                .into_iter()
                .map(|line| line.content)
                .collect::<Vec<_>>()
        }
    };
    let shipped = body(&h).await;

    let first =
        h.ns.get_children(IMPLEMENTING_A_TASK_SKILL_ID)
            .await
            .unwrap()[0]
            .clone();
    h.ns.update_node(
        &first.id,
        first.version,
        NodeUpdate::new().with_content("Our team pairs on every task.".to_string()),
    )
    .await
    .unwrap();

    seed_agent_tables(&h.ns).await;
    assert_eq!(body(&h).await[0], "Our team pairs on every task.");

    let template = seed_skill_nodes()
        .into_iter()
        .find(|template| template.id == IMPLEMENTING_A_TASK_SKILL_ID)
        .unwrap();
    h.ns.reset_seed_node(
        &prepare_nodes_from_template(&template).unwrap(),
        false,
        true,
    )
    .await
    .unwrap();
    assert_eq!(body(&h).await, shipped);
}

/// ADR-072: a procedure the user detached from a queue stays detached across
/// an open, including one that replaces the skill's config, and a reset of
/// its links restores it.
#[tokio::test]
async fn a_detached_procedure_stays_detached_and_a_reset_restores_it() {
    let h = start().await;
    let attached = |h: &Harness| {
        let ns = h.ns.clone();
        async move {
            let mut targets = ns
                .current_links(IMPLEMENTING_A_TASK_SKILL_ID)
                .await
                .unwrap();
            targets.sort();
            targets
        }
    };
    let mut shipped = vec![
        READY_TASKS_QUERY_ID.to_string(),
        IN_PROGRESS_QUERY_ID.to_string(),
    ];
    shipped.sort();
    assert_eq!(attached(&h).await, shipped);

    h.ns.delete_relationship(
        IMPLEMENTING_A_TASK_SKILL_ID,
        SKILL_ATTACHED_TO,
        READY_TASKS_QUERY_ID,
    )
    .await
    .unwrap();

    // A restart, and a config reset that rewrites the skill's `_seed`.
    seed_agent_tables(&h.ns).await;
    let template = seed_skill_nodes()
        .into_iter()
        .find(|template| template.id == IMPLEMENTING_A_TASK_SKILL_ID)
        .unwrap();
    h.ns.reset_seed_node(
        &prepare_nodes_from_template(&template).unwrap(),
        true,
        false,
    )
    .await
    .unwrap();
    seed_agent_tables(&h.ns).await;
    assert_eq!(attached(&h).await, [IN_PROGRESS_QUERY_ID]);

    let links = shipped_attached_to_for(IMPLEMENTING_A_TASK_SKILL_ID).unwrap();
    assert!(h.ns.reset_links(links).await.unwrap());
    assert_eq!(attached(&h).await, shipped);
    seed_agent_tables(&h.ns).await;
    assert_eq!(attached(&h).await, shipped);
}

/// "Creating a Task" and "Answering a Question", each step made as its body
/// says, through the tools: a task created outside a spec is ready as it is
/// created, a task with no checklist is in Needs shaping with its procedure,
/// and a question that blocks a task is the person's to answer, which
/// releases it with its status and assignee as they were (ADR-099).
#[tokio::test]
async fn creating_a_task_and_answering_a_question_can_be_followed_through_the_tools() {
    let h = start().await;
    let project = h.create("software-project", "Apollo", json!({})).await;

    // Creating, steps 1 and 2: look for it built and for a decision that
    // declined it. Nothing is found, and what was searched is said.
    for node_type in ["task", "spec", "decision"] {
        h.ok(
            "search_nodes",
            json!({ "query": "retry the upload", "node_type": node_type }),
        )
        .await;
    }

    // Steps 6 to 8: the issue, its brief, its checklist and its project.
    let created = h
        .ok(
            "create_node",
            json!({ "node_type": "issue", "content": "An upload retries on a dropped connection" }),
        )
        .await;
    let issue = bare(created["id"].as_str().unwrap());
    h.ok(
        "create_node",
        json!({
            "node_type": "text", "parent_id": issue,
            "content": "Uploads resume after a dropped connection. Out of scope: resumable chunks.",
        }),
    )
    .await;
    h.ok(
        "create_node",
        json!({
            "node_type": "checkbox", "parent_id": issue,
            "content": "- [ ] A dropped upload is retried up to three times",
        }),
    )
    .await;
    h.ok(
        "create_relationship",
        json!({ "from_id": uri(&project), "relationship_type": "tasks", "to_id": issue }),
    )
    .await;

    // It is ready the moment it exists, and not in Needs shaping.
    let ready = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true, "limit": 1 }),
        )
        .await;
    assert_eq!(ready["items"][0]["id"], uri(&issue), "{ready}");
    let shaping = h.ok("run_query", json!({ "query": "Needs shaping" })).await;
    assert_eq!(shaping["count"], 0, "{shaping}");

    // A task typed by hand has no checklist: Needs shaping lists it, with
    // Creating a Task attached.
    let by_hand = h
        .create("task", "Look at the export timeout", json!({}))
        .await;
    let shaping = h
        .ok(
            "run_query",
            json!({ "query": "Needs shaping", "with_context": true, "limit": 1 }),
        )
        .await;
    assert_eq!(shaping["items"][0]["id"], uri(&by_hand), "{shaping}");
    assert_eq!(skill_names(&shaping), [CREATING]);

    // A question the issue's owner needs answered by the user: a task
    // assigned to them that blocks the issue.
    let me = h.ns.get_local_person().await.unwrap().unwrap().id;
    let question = bare(
        h.ok(
            "create_node",
            json!({ "node_type": "task", "content": "Is three retries enough, or is it five?" }),
        )
        .await["id"]
            .as_str()
            .unwrap(),
    );
    h.ok(
        "create_relationship",
        json!({ "from_id": uri(&me), "relationship_type": "tasks", "to_id": question }),
    )
    .await;
    h.ok(
        "create_relationship",
        json!({ "from_id": question, "relationship_type": "blocks", "to_id": issue }),
    )
    .await;

    // The question has no checklist, and is not offered for shaping: it
    // blocks a task, so it is answered from Waiting on you.
    let shaping = h.ok("run_query", json!({ "query": "Needs shaping" })).await;
    assert_eq!(
        shaping["count"], 1,
        "only the task typed by hand is shaped: {shaping}"
    );

    // The stopped issue is no longer ready and has not changed.
    assert_eq!(h.status(&issue).await, "open");
    let ready = h.ok("run_query", json!({ "query": "Ready tasks" })).await;
    assert_eq!(ready["count"], 0, "{ready}");

    // Answering, steps 1 to 3: the question is the user's, with its own
    // procedure; the answer is recorded under it and it is completed.
    let waiting = h
        .ok(
            "run_query",
            json!({ "query": "Waiting on you", "with_context": true, "limit": 1 }),
        )
        .await;
    let item = &waiting["items"][0];
    assert_eq!(item["id"], uri(&question), "{waiting}");
    assert_eq!(skill_names(&waiting), [ANSWERING], "{waiting}");
    h.ok(
        "create_node",
        json!({ "node_type": "text", "parent_id": question, "content": "Five." }),
    )
    .await;
    h.ok(
        "update_task_status",
        json!({ "id": uri(&question), "status": "done", "version": item["node_version"] }),
    )
    .await;

    // The issue is ready again, with nothing else changed on it.
    assert_eq!(h.status(&issue).await, "open");
    let ready = h
        .ok(
            "run_query",
            json!({ "query": "Ready tasks", "with_context": true, "limit": 1 }),
        )
        .await;
    assert_eq!(ready["items"][0]["id"], uri(&issue), "{ready}");
    let waiting = h
        .ok("run_query", json!({ "query": "Waiting on you" }))
        .await;
    assert_eq!(waiting["count"], 0, "{waiting}");
}

/// The text of a finding, as a reviewer writes it and then decides it.
const UNDECIDED: &str = "- [ ] Finding (spec, high): rows are not streamed.";
const TO_FIX: &str =
    "- [ ] Finding (spec, high): rows are not streamed. Decision: fix. Reason: spec item one.";
const FIXED: &str =
    "- [x] Finding (spec, high): rows are not streamed. Decision: fix. Reason: spec item one.";

/// Tick both checklist items of a planned work item and move it into review.
async fn into_review(h: &Harness, work: &Work) {
    for checkbox in h.ns.get_children(&work.task).await.unwrap() {
        let ticked = checkbox.content.replacen("- [ ] ", "- [x] ", 1);
        h.ns.update_node(
            &checkbox.id,
            checkbox.version,
            NodeUpdate::new().with_content(ticked),
        )
        .await
        .unwrap();
    }
    h.set(&work.task, json!({ "status": "in_progress" })).await;
    h.set(&work.task, json!({ "status": "in_review" })).await;
}

/// Move `id` to `status`, and return the refusal when a rule stops it.
async fn try_status(h: &Harness, id: &str, status: &str) -> Result<(), String> {
    let version = h.ns.get_node(id).await.unwrap().unwrap().version;
    h.ns.update_node(
        id,
        version,
        NodeUpdate::new().with_properties(json!({ "status": status })),
    )
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn rewrite(h: &Harness, id: &str, content: &str) {
    let version = h.ns.get_node(id).await.unwrap().unwrap().version;
    h.ns.update_node(
        id,
        version,
        NodeUpdate::new().with_content(content.to_string()),
    )
    .await
    .unwrap();
}

/// The review flow of an issue, through the rules and the queues: a finding
/// with no decision keeps it in review, a finding decided fix sends it back
/// and keeps it out of review and out of ready to merge until it is fixed,
/// and each stage's queue lists it alone with its own procedure.
#[tokio::test]
async fn an_issue_moves_through_review_by_its_findings() {
    let h = start().await;
    let work = planned_issue(&h).await;
    let issue = uri(&work.task);
    into_review(&h, &work).await;

    // A finding that has no decision stops the issue leaving review, in
    // either direction.
    let finding = h.child(&work.task, "checkbox", UNDECIDED).await;
    for status in ["changes_requested", "ready_to_merge"] {
        let refused = try_status(&h, &work.task, status).await.unwrap_err();
        assert!(refused.contains("no decision"), "{status}: {refused}");
    }
    // Done is held by the findings rule too, and not only by the unchecked
    // box: a ticked finding with no decision is refused for that alone.
    rewrite(
        &h,
        &finding,
        "- [x] Finding (spec, high): rows are not streamed.",
    )
    .await;
    let refused = try_status(&h, &work.task, "done").await.unwrap_err();
    assert!(refused.contains("no decision"), "done: {refused}");
    rewrite(&h, &finding, UNDECIDED).await;
    assert_eq!(h.status(&work.task).await, "in_review");

    // Decided fix, it can be sent back, and is then in Changes requested
    // alone, with the procedure for addressing the findings.
    rewrite(&h, &finding, TO_FIX).await;
    try_status(&h, &work.task, "changes_requested")
        .await
        .unwrap();
    let back = h
        .ok(
            "run_query",
            json!({ "query": "Changes requested", "with_context": true, "limit": 1 }),
        )
        .await;
    assert_eq!(back["items"][0]["id"], issue, "{back}");
    // It carries the addressing procedure. It also carries the implementing
    // one: "In progress" is written against `task`, where `changes_requested`
    // reads as the `in_progress` it maps to, so the issue is still listed
    // there. The implementing procedure hands over to the addressing one.
    let mut names = skill_names(&back);
    names.sort();
    assert_eq!(names, [ADDRESSING, IMPLEMENTING], "{back}");
    let in_progress = h.ok("run_query", json!({ "query": "In progress" })).await;
    assert_eq!(in_progress["nodes"][0]["id"], issue, "{in_progress}");
    for queue in ["Awaiting review", "Ready to merge"] {
        let run = h.ok("run_query", json!({ "query": queue })).await;
        assert_eq!(run["count"], 0, "{queue}: {run}");
    }

    // Neither review nor merge while the fix is open.
    for status in ["in_review", "ready_to_merge"] {
        let refused = try_status(&h, &work.task, status).await.unwrap_err();
        assert!(refused.contains("decided fix"), "{status}: {refused}");
    }
    assert_eq!(h.status(&work.task).await, "changes_requested");

    // Fixed, it goes back to review, and is awaiting review alone.
    rewrite(&h, &finding, FIXED).await;
    try_status(&h, &work.task, "in_review").await.unwrap();
    let awaiting = h
        .ok("run_query", json!({ "query": "Awaiting review" }))
        .await;
    assert_eq!(awaiting["nodes"][0]["id"], issue, "{awaiting}");
    let changes = h
        .ok("run_query", json!({ "query": "Changes requested" }))
        .await;
    assert_eq!(changes["count"], 0, "{changes}");

    // A deferred finding is settled, so it holds nothing back, and the
    // issue is ready to merge, listed to be landed and no longer awaiting.
    h.child(
        &work.task,
        "checkbox",
        "- [x] Finding (standards, low): a name. Decision: defer. Reason: renamed later.",
    )
    .await;
    try_status(&h, &work.task, "ready_to_merge").await.unwrap();
    let landing = h
        .ok("run_query", json!({ "query": "Ready to merge" }))
        .await;
    assert_eq!(landing["nodes"][0]["id"], issue, "{landing}");
    let awaiting = h
        .ok("run_query", json!({ "query": "Awaiting review" }))
        .await;
    assert_eq!(awaiting["count"], 0, "{awaiting}");

    // The landing is the team's: done is set when the change lands, and an
    // unfixed finding could not have got here, since it is an unchecked
    // checkbox.
    try_status(&h, &work.task, "done").await.unwrap();
}

/// A finding decided fix cannot be skipped on the way to done either: it is
/// an unchecked checkbox, so the criteria rule that already stands refuses.
#[tokio::test]
async fn an_unfixed_finding_cannot_reach_done() {
    let h = start().await;
    let work = planned_issue(&h).await;
    into_review(&h, &work).await;
    h.child(&work.task, "checkbox", TO_FIX).await;
    try_status(&h, &work.task, "changes_requested")
        .await
        .unwrap();
    let refused = try_status(&h, &work.task, "done").await.unwrap_err();
    assert!(refused.contains("unchecked item"), "{refused}");
}

/// The review rules are written against `issue`: a plain task is touched by
/// none of them, whatever its checkboxes say, and goes from review to done.
#[tokio::test]
async fn a_plain_task_is_touched_by_none_of_the_review_rules() {
    let h = start().await;
    let work = planned_task(&h).await;
    into_review(&h, &work).await;

    h.child(&work.task, "checkbox", TO_FIX).await;
    h.child(&work.task, "checkbox", UNDECIDED).await;
    // A task with a checklist item still unchecked is held by the criteria
    // rule, as before; that is its checklist and not a finding.
    let refused = try_status(&h, &work.task, "done").await.unwrap_err();
    assert!(
        refused.contains("unchecked item") && !refused.contains("finding"),
        "{refused}"
    );

    // Moves the findings rules guard on an issue are free on a task.
    try_status(&h, &work.task, "in_progress").await.unwrap();
    try_status(&h, &work.task, "in_review").await.unwrap();
    let awaiting = h
        .ok("run_query", json!({ "query": "Awaiting review" }))
        .await;
    assert_eq!(awaiting["count"], 0, "the queue lists issues: {awaiting}");
}

/// What each delivery procedure must say, read from the body an agent gets
/// (agent and external form alike).
#[test]
fn the_delivery_procedures_state_what_they_are_held_to() {
    let rendered = |title: &str| {
        let seed = SKILL_SEEDS.iter().find(|s| s.title == title).unwrap();
        [seed.template().markdown_content, seed.external_body()]
    };

    for text in rendered(IMPLEMENTING) {
        // The user chooses; nothing is started on the agent's own.
        assert!(
            text.contains("you do not take a task on your own"),
            "{text}"
        );
        assert!(text.contains("stop until the user chooses"), "{text}");
        // Test-first, one item at a time, and bulk test writing named as not
        // this; the evidence shows both runs.
        assert!(text.contains("write one failing test"), "{text}");
        assert!(text.contains("see it fail"), "{text}");
        assert!(
            text.contains("Writing all the tests first and the code afterwards is not this"),
            "{text}"
        );
        assert!(
            text.contains("the failing run, then the passing run"),
            "{text}"
        );
        // The full suite once, and bounded fix attempts.
        assert!(
            text.contains("run the project's whole test suite once"),
            "{text}"
        );
        assert!(text.contains("three at most"), "{text}");
        assert!(text.contains("a single rerun"), "{text}");
    }

    for text in rendered(REVIEWING) {
        // A different session: how the reviewer tells is written down.
        assert!(text.contains("you implemented it: stop"), "{text}");
        assert!(text.contains("ticked an item"), "{text}");
        // Two separate passes, with the audit's five marks.
        assert!(text.contains("PASS ONE, AGAINST THE SPEC"), "{text}");
        assert!(text.contains("PASS TWO, AGAINST THE STANDARDS"), "{text}");
        assert!(text.contains("never merged"), "{text}");
        for mark in ["done", "partial", "not done", "changed", "unverifiable"] {
            assert!(text.contains(mark), "the audit has no mark {mark:?}");
        }
        assert!(text.contains("did not ask for"), "{text}");
        // A weakened check is the user's, as restore or keep.
        assert!(text.contains("restore or keep"), "{text}");
        assert!(text.contains("never decided by you"), "{text}");
        // A decision per finding, before any fix.
        assert!(text.contains("fix, defer or reject"), "{text}");
        // A task with no spec has what it is reviewed against.
        assert!(text.contains("A task with no spec"), "{text}");
        // An issue that passed is not done.
        assert!(text.contains("ready to merge and not done"), "{text}");
    }

    for text in rendered(ADDRESSING) {
        assert!(text.contains("before any fix"), "{text}");
        assert!(
            text.contains("do not review the issue again yourself"),
            "{text}"
        );
        assert!(text.contains("`in_review`"), "{text}");
    }
}
