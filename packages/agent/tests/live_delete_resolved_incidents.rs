//! "delete the resolved incidents" must act on the incidents, not the conflict
//! journal.
//!
//! The request shares "resolved" with Conflict Journal's vocabulary, and
//! `list_conflicts` has a `status: resolved` filter. That pairing was measured
//! pulling the model onto `list_conflicts` 6/6 even while Node Deletion won
//! retrieval, so the deletion never happened. This guards against the pull
//! returning, and against the fix for it costing the conflict journal its own
//! requests.
//!
//! Drives the production stack in-process: the seeded skill registry embedded
//! with the real embedding model, a real `GraphToolExecutor` over a real
//! `SqliteStore`, `PromptAssembler`, workspace context, and the locked E4B
//! model through `LocalAgentService::send_message` (Stage 1, skill retrieval,
//! Stage 2). Outcomes are scored from the STORE and the executed calls.
//!
//! Set `NODESPACE_PROMPT_DUMP=<file>` to capture every prompt the model saw.
//!
//! Ignored by default — loads the 5GB locked GGUF and the embedding model:
//! ```text
//! cargo test --release -p nodespace-agent --test live_delete_resolved_incidents -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{AgentToolExecutor, ModelFamily};
use nodespace_agent::local_agent::agent_loop::LocalAgentService;
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::ops::context_ops::build_workspace_context;
use nodespace_core::services::node_service::CreateNodeParams;
use nodespace_core::services::{
    InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::chat::ChatConfig;
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Single-run results on this stack are not decision-grade.
const REPS: usize = 3;

fn model_path() -> String {
    let home = std::env::var("HOME").expect("HOME must be set");
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

/// Loaded once per test and shared by every rep's fixture; each rep still
/// gets a fresh DB.
fn load_embedder() -> Arc<EmbeddingService> {
    let mut nlp = EmbeddingService::new(EmbeddingConfig::default()).expect("embedding config");
    nlp.initialize().expect("embedding model must load");
    Arc::new(nlp)
}

fn load_engine() -> Arc<LlamaChatInferenceEngine> {
    let config = ChatConfig {
        n_ctx: 32768,
        ..Default::default()
    };
    Arc::new(
        LlamaChatInferenceEngine::load(&model_path(), ModelFamily::Gemma4, config)
            .expect("model must load from the standard catalog path"),
    )
}

/// An incident created by the fixture: store id plus the title the model sees.
struct Incident {
    id: String,
    title: &'static str,
}

struct Fixture {
    node_service: Arc<NodeService>,
    embedding_service: Arc<NodeEmbeddingService>,
    executor: Arc<GraphToolExecutor>,
    resolved: Vec<Incident>,
    open: Vec<Incident>,
    _tmp: TempDir,
}

/// A fresh DB with the seeded skill registry, an `Incident` schema whose
/// `state` enum includes `resolved`, two resolved incidents and one open one.
/// Every root is embedded, as the daemon's embedding pass does in steady state.
async fn fixture(
    engine: Arc<LlamaChatInferenceEngine>,
    embedder: Arc<EmbeddingService>,
) -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Arc::new(
        SqliteStore::new(tmp.path().join("live.db"))
            .await
            .expect("store must open"),
    );
    let node_service = Arc::new(NodeService::new(&mut store).await.expect("node service"));

    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        embedder,
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));

    for tmpl in seed_skill_nodes() {
        let prepared = prepare_nodes_from_template(&tmpl).expect("template must parse");
        for p in &prepared {
            node_service
                .create_node_with_parent(CreateNodeParams {
                    id: Some(p.id.clone()),
                    node_type: p.node_type.clone(),
                    content: p.content.clone(),
                    parent_id: p.parent_id.clone(),
                    position: InsertPositionOwned::End,
                    properties: p.properties.clone(),
                    lifecycle_status: None,
                })
                .await
                .expect("skill node must insert");
        }
        embedding_service
            .embed_root_node(&prepared[0].id)
            .await
            .expect("skill root must embed");
    }

    let executor = Arc::new(GraphToolExecutor {
        node_service: Some(node_service.clone()),
        embedding_service: Arc::new(RwLock::new(Some(embedding_service.clone()))),
        inference_engine: Some(engine),
        playbook_lifecycle: None,
    });

    let schema = executor
        .execute(
            "create_schema",
            json!({
                "name": "Incident",
                "fields": [
                    {"name": "state", "type": "enum", "required": true, "coreValues": [
                        {"value": "open", "label": "Open"},
                        {"value": "resolved", "label": "Resolved"}
                    ]},
                    {"name": "severity", "type": "text"}
                ]
            }),
        )
        .await
        .expect("create_schema must run");
    assert!(
        !schema.is_error,
        "create_schema failed: {:?}",
        schema.result
    );
    embedding_service
        .embed_root_node("incident")
        .await
        .expect("incident schema must embed");

    let mut resolved = Vec::new();
    let mut open = Vec::new();
    for (title, state) in [
        ("Checkout outage on Friday", "resolved"),
        ("Login latency spike", "resolved"),
        ("Search index lagging", "open"),
    ] {
        let created = executor
            .execute(
                "create_node",
                json!({
                    "node_type": "incident",
                    "content": title,
                    "field_values": {"state": state, "severity": "high"}
                }),
            )
            .await
            .expect("create_node must run");
        assert!(
            !created.is_error,
            "create_node failed: {:?}",
            created.result
        );
        let uri = created.result["id"]
            .as_str()
            .expect("create_node returns an id");
        let id = uri.strip_prefix("nodespace://").unwrap_or(uri).to_string();
        embedding_service
            .embed_root_node(&id)
            .await
            .expect("incident must embed");
        let incident = Incident { id, title };
        if state == "resolved" {
            resolved.push(incident);
        } else {
            open.push(incident);
        }
    }

    Fixture {
        node_service,
        embedding_service,
        executor,
        resolved,
        open,
        _tmp: tmp,
    }
}

/// Runs one production turn; returns the executed tool calls and the reply.
async fn run_turn(
    engine: Arc<LlamaChatInferenceEngine>,
    fx: &Fixture,
    message: &str,
) -> (Vec<(String, Value)>, String) {
    let service = LocalAgentService::new_with_assembler(
        engine,
        fx.executor.clone(),
        Some(Arc::new(PromptAssembler::new(fx.node_service.clone()))),
    );
    let session = service.create_session(None, Vec::new()).await;
    // The daemon wraps this call with two extras, both inert here: an
    // injector for schemas created in the last few minutes (the fixture
    // embeds its schema, so semantic retrieval already finds it) and the
    // mentioned-entities duplicate-create guard (no turn here creates).
    // A variant that skips embedding the schema would need the injector.
    let ctx = build_workspace_context(
        &fx.node_service,
        Some(&fx.embedding_service),
        Some(message),
        Some(message),
    )
    .await
    .expect("workspace context must build");
    service
        .set_session_context(&session, ctx.format_for_prompt(4000))
        .await;

    let result = service
        .send_message(&session, message, |_| {}, |_| {})
        .await
        .expect("turn must complete");

    let calls = service
        .get_session(&session)
        .await
        .expect("session must survive the turn")
        .tool_executions
        .iter()
        .map(|t| (t.name.clone(), t.args.clone()))
        .collect();
    (calls, result.response)
}

async fn exists(ns: &NodeService, id: &str) -> bool {
    matches!(ns.get_node(id).await, Ok(Some(_)))
}

/// Ids every `delete_node` call targeted, with any `nodespace://` prefix
/// stripped to match store ids.
fn deleted_ids(calls: &[(String, Value)]) -> Vec<String> {
    calls
        .iter()
        .filter(|(name, _)| name == "delete_node")
        .filter_map(|(_, args)| args["id"].as_str().or_else(|| args["node_id"].as_str()))
        .map(|id| id.strip_prefix("nodespace://").unwrap_or(id).to_string())
        .collect()
}

fn called(calls: &[(String, Value)], tool: &str) -> bool {
    calls.iter().any(|(name, _)| name == tool)
}

/// Passes when the turn acts on the incidents: it never reads the conflict
/// journal, deletes nothing but resolved incidents, leaves the open incident,
/// and either deletes both resolved incidents or names exactly those two back
/// to the user.
///
/// A reply that stops to confirm the deletion also passes. Whether an agent
/// delete runs without an explicit yes is a separate decision from which
/// records it targets; this test pins only the targeting.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads the locked GGUF and the embedding model"]
async fn delete_the_resolved_incidents_targets_the_incidents_not_the_conflict_journal() {
    let engine = load_engine();
    let embedder = load_embedder();

    let mut passes = 0;
    for rep in 0..REPS {
        let fx = fixture(engine.clone(), embedder.clone()).await;
        let (calls, reply) = run_turn(engine.clone(), &fx, "delete the resolved incidents").await;

        let mut resolved_gone = true;
        for i in &fx.resolved {
            resolved_gone &= !exists(&fx.node_service, &i.id).await;
        }
        let mut open_kept = true;
        for i in &fx.open {
            open_kept &= exists(&fx.node_service, &i.id).await;
        }
        let named_exactly_the_resolved = fx.resolved.iter().all(|i| reply.contains(i.title))
            && fx.open.iter().all(|i| !reply.contains(i.title));

        let deleted_only_resolved = deleted_ids(&calls)
            .iter()
            .all(|id| fx.resolved.iter().any(|i| &i.id == id));

        let ok = !called(&calls, "list_conflicts")
            && deleted_only_resolved
            && open_kept
            && (resolved_gone || named_exactly_the_resolved);
        passes += usize::from(ok);
        eprintln!(
            "rep {rep}: {} resolved_gone={resolved_gone} open_kept={open_kept} deleted_only_resolved={deleted_only_resolved} calls={calls:?}\n  reply={reply:?}",
            if ok { "PASS" } else { "FAIL" }
        );
    }
    assert_eq!(
        passes, REPS,
        "{passes}/{REPS} reps acted on exactly the resolved incidents"
    );
}

/// The guard's other side: requests that are about the conflict journal must
/// still reach `list_conflicts`, in the same workspace.
///
/// The question form reaches it only because Stage 1 is told which skills
/// exist (`stage1_system_prompt`); without that it asked what kind of
/// conflicts were meant.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads the locked GGUF and the embedding model"]
async fn conflict_journal_requests_still_call_list_conflicts() {
    let engine = load_engine();
    let embedder = load_embedder();

    let mut failures = Vec::new();
    for message in [
        "show me the resolved conflicts",
        "list the open conflicts",
        "are there any unresolved conflicts?",
    ] {
        for rep in 0..REPS {
            let fx = fixture(engine.clone(), embedder.clone()).await;
            let (calls, reply) = run_turn(engine.clone(), &fx, message).await;
            let ok = called(&calls, "list_conflicts");
            eprintln!(
                "{message:?} rep {rep}: {} calls={calls:?}\n  reply={reply:?}",
                if ok { "PASS" } else { "FAIL" }
            );
            if !ok {
                failures.push(format!("{message:?} rep {rep}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "conflict-journal requests that never called list_conflicts: {failures:?}"
    );
}
