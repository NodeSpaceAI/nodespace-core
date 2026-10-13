//! A chat bound to a type: the first reply for each option reaches the
//! type-changing skill, with the type in what the model was sent, and writes
//! nothing until the user has agreed (ADR-100 §3 to §5).
//!
//! Drives the production stack in-process the way the daemon does for a bound
//! chat: the seeded skill registry, a real `GraphToolExecutor` over a real
//! `SqliteStore`, the skill pinned beside whatever retrieval finds, the type
//! named to Stage 1 as the subject, and the opening message the chat was
//! opened on as history. Outcomes are scored from the STORE and the executed
//! calls.
//!
//! Set `NODESPACE_PROMPT_DUMP=<file>` to capture every prompt the model saw.
//!
//! Ignored by default: it loads the 5GB locked GGUF and the embedding model.
//! ```text
//! NODESPACE_PROMPT_DUMP=/tmp/type-chat.jsonl cargo nextest run --release -p nodespace-agent \
//!   --test it live_type_chat_options:: --run-ignored only -j 1 --no-capture
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{AgentToolExecutor, PinnedSubject};
use nodespace_agent::local_agent::agent_loop::LocalAgentService;
use nodespace_agent::local_agent::tools::{pinned_skill_candidates, GraphToolExecutor};
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, SCHEMA_CHANGING_SKILL_ID};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::ops::context_ops::build_workspace_context;
use nodespace_core::services::node_service::CreateNodeParams;
use nodespace_core::services::{
    InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::chat::{ChatConfig, ChatMessage, Role};
use nodespace_nlp_engine::inference::local::LlamaChatInferenceEngine;
use nodespace_nlp_engine::ModelFamily;
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Single-run results on this stack are not decision-grade.
const REPS: usize = 3;

const OPENING: &str =
    "Here is what [Ticket](nodespace://ticket) is. A customer support request.\n\n\
It is your own type, so its fields can be added, changed and removed.\n\n\
It has 2 fields.\n\
- **summary** (text): What went wrong.\n\
- **state** (enum): Values: open, closed.\n\n\
What would you like to change?";

const OPTIONS: [&str; 5] = [
    "Explain this type",
    "Add a field",
    "Change a field",
    "Add a value to a list",
    "Remove a field",
];

fn model_path() -> String {
    let home = std::env::var("HOME").expect("HOME must be set");
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

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
        LlamaChatInferenceEngine::load(
            &model_path(),
            ModelFamily::Gemma4,
            config,
            nodespace_agent::local_agent::model_manager::expected_sha256_for_path(&model_path()),
        )
        .expect("model must load from the standard catalog path"),
    )
}

struct Fixture {
    node_service: Arc<NodeService>,
    embedding_service: Arc<NodeEmbeddingService>,
    executor: Arc<GraphToolExecutor>,
    _tmp: TempDir,
}

/// A fresh DB with the seeded skills, a `Ticket` type, and a saved query that
/// filters on its `state`.
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
                "name": "Ticket",
                "description": "A customer support request.",
                "fields": [
                    {"name": "summary", "type": "text", "description": "What went wrong."},
                    {"name": "state", "type": "enum", "coreValues": [
                        {"value": "open", "label": "Open"},
                        {"value": "closed", "label": "Closed"}
                    ]}
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
        .embed_root_node("ticket")
        .await
        .expect("ticket schema must embed");

    node_service
        .create_node(nodespace_core::models::Node::new(
            "query".to_string(),
            "Open tickets".to_string(),
            json!({
                "target_type": "ticket",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "state", "value": "open"
                }]
            }),
        ))
        .await
        .expect("query must create");

    Fixture {
        node_service,
        embedding_service,
        executor,
        _tmp: tmp,
    }
}

/// One turn of the bound chat, as the daemon runs it: history is the opening
/// message, the skill and the type are pinned, the options are offered.
async fn run_turn(
    engine: Arc<LlamaChatInferenceEngine>,
    fx: &Fixture,
    message: &str,
) -> (Vec<(String, Value)>, String) {
    run_turn_after(engine, fx, Vec::new(), message).await
}

/// [`run_turn`] with `earlier` turns between the opening message and
/// `message`.
async fn run_turn_after(
    engine: Arc<LlamaChatInferenceEngine>,
    fx: &Fixture,
    earlier: Vec<ChatMessage>,
    message: &str,
) -> (Vec<(String, Value)>, String) {
    let service = LocalAgentService::new_with_assembler(
        engine,
        fx.executor.clone(),
        Some(Arc::new(PromptAssembler::new(fx.node_service.clone()))),
    );
    let mut history = vec![ChatMessage::text(Role::Assistant, OPENING)];
    history.extend(earlier);
    let session = service.create_session(None, history).await;

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

    let skills = pinned_skill_candidates(&fx.node_service, &[SCHEMA_CHANGING_SKILL_ID.to_string()])
        .await
        .expect("the pinned skill must load");
    service
        .set_session_pins(
            &session,
            skills,
            vec![PinnedSubject {
                title: Some("Ticket".to_string()),
                node_type: "schema".to_string(),
            }],
        )
        .await;
    service
        .set_session_offered_options(&session, OPTIONS.iter().map(|o| o.to_string()).collect())
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

fn called(calls: &[(String, Value)], tool: &str) -> bool {
    calls.iter().any(|(name, _)| name == tool)
}

/// The names of the type's fields as stored now.
async fn field_names(fx: &Fixture) -> Vec<String> {
    fx.node_service
        .get_schema_node("ticket")
        .await
        .expect("schema read")
        .expect("ticket exists")
        .fields
        .into_iter()
        .map(|f| f.name)
        .collect()
}

/// Picking an option answers or asks, and writes nothing: the first reply for
/// each of the five options leaves the type as it was and replies in words.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads the locked GGUF and the embedding model"]
async fn the_first_reply_for_each_option_writes_nothing() {
    let engine = load_engine();
    let embedder = load_embedder();

    let mut failures = Vec::new();
    for option in OPTIONS {
        for rep in 0..REPS {
            let fx = fixture(engine.clone(), embedder.clone()).await;
            let before = field_names(&fx).await;
            let (calls, reply) = run_turn(engine.clone(), &fx, option).await;
            let after = field_names(&fx).await;

            let wrote = called(&calls, "update_schema") || called(&calls, "delete_node");
            let ok = !wrote && before == after && !reply.trim().is_empty();
            eprintln!(
                "{} {option:?} rep {rep}: calls={:?}\n  reply={reply:?}",
                if ok { "PASS" } else { "FAIL" },
                calls.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            );
            if !ok {
                failures.push(format!("{option} rep {rep}"));
            }
        }
    }
    assert!(failures.is_empty(), "wrote or said nothing: {failures:?}");
}

/// A rename the user agrees to still names what it would break before it
/// writes: the model asked, the user said yes, and the write is refused with
/// the saved query that filters on the field, which the reply then tells the
/// user. The field is unchanged afterwards.
///
/// The model's own first reply to a bare "rename" is not held to this: on the
/// locked model it answers without reading the type, and says nothing names
/// the field. What holds is the refusal, which is the tool's and does not
/// depend on which skill led the turn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads the locked GGUF and the embedding model"]
async fn an_agreed_rename_names_the_saved_query_before_it_writes() {
    let engine = load_engine();
    let embedder = load_embedder();

    let earlier = || {
        vec![
            ChatMessage::text(Role::User, "Rename state to status"),
            ChatMessage::text(
                Role::Assistant,
                "I can rename the field `state` to `status`. Would you like me to go ahead?",
            ),
        ]
    };
    let mut passes = 0;
    for rep in 0..REPS {
        let fx = fixture(engine.clone(), embedder.clone()).await;
        let before = field_names(&fx).await;
        let (calls, reply) = run_turn_after(engine.clone(), &fx, earlier(), "Yes, go ahead").await;
        let after = field_names(&fx).await;

        let attempted = called(&calls, "update_schema");
        let names_the_query = reply.contains("Open tickets");
        let ok = attempted && before == after && names_the_query;
        passes += usize::from(ok);
        eprintln!(
            "{} rep {rep}: attempted={attempted} unchanged={} names_the_query={names_the_query} calls={calls:?}\n  reply={reply:?}",
            if ok { "PASS" } else { "FAIL" },
            before == after,
        );
    }
    assert_eq!(passes, REPS, "{passes}/{REPS} reps named the query first");
}
