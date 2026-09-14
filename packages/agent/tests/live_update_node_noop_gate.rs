//! Live verification of the `update_node` no-op gate against the locked model.
//!
//! The in-crate unit tests for this gate (`tools.rs`,
//! `mod update_node_noop_gate`) prove the gate behaves correctly *given* a
//! content-only call. They cannot show what this issue actually turned on: that
//! the locked model, sampling freely, produces such a call in the first place,
//! and what the loop does when it happens. #1931's guidance was verified 3/3 in
//! isolation and still failed live — so a fix on this surface is not established
//! by unit tests alone.
//!
//! Unlike `golden_scenario6_*.rs`, this drives the REAL production surface:
//! `all_tool_definitions()` and a real `GraphToolExecutor` over a real
//! `SqliteStore`, not hand-authored tool schemas. The assertion is on what the
//! STORE holds afterwards, never on what the tool returned — the entire defect
//! was a call that returned success while persisting nothing.
//!
//! Ignored by default — loads the 5GB locked native GGUF. Run explicitly:
//! ```text
//! cargo test -p nodespace-agent --test live_update_node_noop_gate -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolDefinition,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::tools::{all_tool_definitions, GraphToolExecutor};
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::NodeService;
use nodespace_nlp_engine::chat::ChatConfig;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Number of independent repetitions. Single-run results on this stack are not
/// decision-grade — identical code has scored differently across repeated runs
/// at temperature 0.1 — so the gate is exercised repeatedly rather than once.
const REPS: usize = 3;

fn model_path() -> String {
    let home = std::env::var("HOME").expect("HOME must be set");
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

fn load_engine() -> LlamaChatInferenceEngine {
    let config = ChatConfig {
        n_ctx: 32768,
        default_temperature: 0.1,
        ..Default::default()
    };
    LlamaChatInferenceEngine::load(&model_path(), ModelFamily::Gemma4, config)
        .expect("model must load from the standard catalog path")
}

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("live.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// Only the tools this scenario needs. The full set is ~a dozen; narrowing to
/// the write path keeps the turn focused without hand-authoring any schema —
/// these are the REAL production definitions, filtered, not rewritten.
fn update_tool_only() -> Vec<ToolDefinition> {
    all_tool_definitions()
        .into_iter()
        .filter(|t| t.name == "update_node")
        .collect()
}

/// Runs one turn and returns the parsed (tool_name, args_json), if any.
async fn run_turn(
    engine: &LlamaChatInferenceEngine,
    system_prompt: &str,
    tools: Vec<ToolDefinition>,
    user_message: &str,
) -> (Option<(String, String)>, String) {
    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, system_prompt.to_string()),
            ChatMessage::text(Role::User, user_message.to_string()),
        ],
        tools: Some(tools),
        temperature: Some(0.1),
        max_tokens: Some(512),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = chunks.clone();
    engine
        .generate(
            request,
            Box::new(move |c| {
                if let Ok(mut g) = sink.lock() {
                    g.push(c);
                }
            }),
        )
        .await
        .expect("generation must complete");

    let collected = chunks.lock().expect("chunk mutex").clone();
    let name = collected.iter().find_map(|c| match c {
        StreamingChunk::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    let args: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::ToolCallArgs { args_json, .. } => Some(args_json.as_str()),
            _ => None,
        })
        .collect();
    let raw: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (name.map(|n| (n, args)), raw)
}

/// The reproducing scenario, end to end, on the real surface.
///
/// Asserts the outcome that matters and nothing about HOW the model gets there:
/// after the turn, either the requested property is in the store, or the call
/// was rejected. What must never happen — and what happened in production — is
/// the third outcome: a call that reports success while the store is unchanged.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the locked native GGUF on disk"]
async fn state_change_either_persists_or_is_rejected_never_silently_succeeds() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. Act on the user's request immediately \
        using the tools available. Do not ask the user to confirm.";

    let mut persisted = 0usize;
    let mut rejected = 0usize;

    for rep in 0..REPS {
        let (executor, _ns, _tmp) = make_executor().await;
        let title = "Schedule chip upgrade on the Polestar";

        // Fixture task via the real create_node path.
        let created = executor
            .execute(
                "create_node",
                json!({
                    "content": title,
                    "node_type": "task",
                    "field_values": {"status": "in_progress"},
                }),
            )
            .await
            .expect("fixture creation must succeed");
        let node_id = created.result["id"]
            .as_str()
            .unwrap()
            .trim_start_matches("nodespace://")
            .to_string();

        let prompt = format!(
            "For the task with id {node_id} titled \"{title}\", set the deadline to 6-August-2026."
        );
        let (call, raw) = run_turn(&engine, system, update_tool_only(), &prompt).await;

        let Some((name, args_json)) = call else {
            println!("LIVE[rep {rep}] no tool call parsed, raw: {raw:?}");
            continue;
        };
        println!("LIVE[rep {rep}] {name}({args_json})");
        assert_eq!(name, "update_node", "rep {rep}: unexpected tool");

        let args: serde_json::Value = match serde_json::from_str(&args_json) {
            Ok(v) => v,
            Err(e) => {
                println!("LIVE[rep {rep}] unparseable args ({e}), skipping");
                continue;
            }
        };

        let outcome = executor.execute("update_node", args).await;

        // Read the store back — the only trustworthy signal. `modifiedAt` is
        // NOT usable here: the production no-op write bumped it while
        // persisting nothing.
        let stored = executor
            .execute("get_node", json!({ "id": node_id }))
            .await
            .expect("get_node must succeed");
        let props = &stored.result["properties"];
        let has_new_property = props
            .as_object()
            .is_some_and(|o| o.keys().any(|k| k != "status"));

        match outcome {
            Err(e) => {
                // Rejected. Acceptable: the user sees an error and can retry.
                // What must NOT happen is a rejection that still wrote nothing
                // while reporting success.
                println!("LIVE[rep {rep}] rejected: {e}");
                rejected += 1;
            }
            Ok(result) => {
                assert!(
                    !result.is_error,
                    "rep {rep}: unexpected tool error result: {result:?}"
                );
                // The core invariant. A success result must never coexist with
                // an unchanged store.
                assert!(
                    has_new_property,
                    "rep {rep}: update_node reported success but the store holds no new \
                     property — this is exactly the silent-data-loss shape #1937 reported. \
                     result={} stored_properties={}",
                    result.result, props
                );
                // And the honest-result rule: never `updated: true` beside a
                // zero property count.
                if result.result["property_count"] == json!(0) {
                    assert!(
                        result.result.get("updated").is_none(),
                        "rep {rep}: result pairs `updated: true` with `property_count: 0`: {}",
                        result.result
                    );
                }
                persisted += 1;
            }
        }
    }

    println!("LIVE SUMMARY: persisted={persisted} rejected={rejected} of {REPS} reps");
    // Every rep must land in one of the two honest outcomes. The assertions in
    // the loop already fail on the dishonest one; this catches a run where the
    // model never produced a usable call at all, which would make the reps
    // vacuous rather than passing.
    assert!(
        persisted + rejected > 0,
        "no rep produced a usable update_node call — the run proves nothing"
    );
}
