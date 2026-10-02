//! Validates that a cardinality-one eviction survives the REAL history
//! pipeline (`completed_writes_from` + `node_history_from_messages`) well
//! enough for a later turn to act on it.
//!
//! `create_relationship` enforces a `cardinality: one` end by replacing the
//! prior edge, and reports the evicted edge in its result's `replaced` list.
//! History rebuilt for the next turn replaces the assistant's prose with terse
//! write facts, so whatever the model said about the eviction is gone; only
//! the completed-write record remains. This file builds the turn where
//! "assign the launch checklist to Bob" evicted Alice, runs it through the
//! real pipeline, and asks the next turn to give the task back. The previous
//! holder's id appears nowhere in history except the eviction record, so
//! restoring her is only possible if that record reached the rendered history.
//!
//! The history is production's; the prompt around it is not. The system
//! prompt is a single sentence and the tool definitions are restated by hand,
//! so this isolates what history carries — it does not validate the resident
//! prompt or skill guidance.
//!
//! `rendered_history_names_the_evicted_holder` runs with the ordinary suite.
//! The model test is ignored by default — it loads the 5GB locked native GGUF.
//! Run it explicitly:
//! ```text
//! cargo test -p nodespace-daemon --test it golden_reassignment_restore_real_pipeline:: -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolDefinition, ToolExecutionRecord,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_core::models::AiChatMessageRole;
use nodespace_daemon::services::chat_messages::{ResolvedEntity, StoredMessage};
use nodespace_daemon::services::local_agent_service::{
    completed_writes_from, node_history_from_messages,
};
use nodespace_nlp_engine::chat::ChatConfig;

const TASK: &str = "nodespace://task-7c1e";
const BOB: &str = "nodespace://person-b42";
const ALICE: &str = "nodespace://person-a9f";

/// Independent generations per prompt. Temperature is low but not zero, so a
/// single run is not decision-grade.
const TRIALS: usize = 3;

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

/// Production's `create_relationship` definition, restated: the agent crate
/// keeps `Tool::definition` crate-private.
fn create_relationship_tool() -> ToolDefinition {
    ToolDefinition {
        name: "create_relationship".into(),
        description: "Record a named relationship between two existing records, given both \
            ids. Use this whenever the user describes one record standing in a relation to \
            another — superseding, blocking, belonging to — rather than writing that relation \
            into a text field."
            .into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "from_id": {"type": "string", "description": "Id of the record that acts. Copied exactly from a tool result."},
                "to_id": {"type": "string", "description": "Id of the record acted upon. Copied exactly from a tool result."},
                "relationship_type": {"type": "string", "description": "The relation's name, lowercase snake_case."}
            },
            "required": ["from_id", "to_id", "relationship_type"]
        }),
    }
}

fn search_nodes_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search_nodes".into(),
        description: "Find records by a text query. Returns matching records with their ids."
            .into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
    }
}

fn message(role: AiChatMessageRole, content: &str) -> StoredMessage {
    StoredMessage {
        role,
        content: content.to_string(),
        timestamp: None,
        reasoning: None,
        completed_writes: Vec::new(),
        resolved_entities: Vec::new(),
        id: String::new(),
        options: Vec::new(),
        pending_deletions: Vec::new(),
        outcome: None,
    }
}

/// Turn 1 as the real agent loop records it: the lookups that resolved Bob
/// and the task, then the reassignment whose result names Alice's evicted
/// edge. Alice was never looked up, so her id exists only in `replaced`.
fn reassignment_turn() -> Vec<StoredMessage> {
    let writes = completed_writes_from(&[ToolExecutionRecord {
        tool_call_id: "tc_create_relationship".into(),
        name: "create_relationship".into(),
        args: serde_json::json!({
            "from_id": BOB, "to_id": TASK, "relationship_type": "tasks"
        }),
        result: serde_json::json!({
            "from_id": BOB, "to_id": TASK, "type": "tasks", "created": true,
            "replaced": [{"from_id": ALICE, "to_id": TASK, "type": "tasks"}],
            "note": "This relationship allows only one link, so creating it removed the \
                relationship(s) listed in `replaced`. Tell the user which link was removed."
        }),
        is_error: false,
        duration_ms: 1,
    }]);

    let mut assistant = message(
        AiChatMessageRole::Assistant,
        "Done — the launch checklist is now assigned to Bob. It was previously assigned to \
         Alice, so she has been unassigned.",
    );
    assistant.completed_writes = writes;
    assistant.resolved_entities = vec![
        ResolvedEntity {
            tool: "search_nodes".to_string(),
            node_id: BOB.into(),
            title: Some("Bob".into()),
            node_type: Some("person".into()),
        },
        ResolvedEntity {
            tool: "search_nodes".to_string(),
            node_id: TASK.into(),
            title: Some("Launch checklist".into()),
            node_type: Some("task".into()),
        },
    ];

    vec![
        message(
            AiChatMessageRole::User,
            "Assign the launch checklist to Bob",
        ),
        assistant,
    ]
}

/// The rendered history must carry Alice's id — the precondition for any
/// later turn restoring her.
#[test]
fn rendered_history_names_the_evicted_holder() {
    let history = node_history_from_messages(reassignment_turn());
    for (i, m) in history.iter().enumerate() {
        println!("[{i}] {:?}: {}", m.role, m.content);
    }
    let assistant = history
        .iter()
        .find(|m| matches!(m.role, Role::Assistant))
        .expect("assistant turn rendered");
    assert!(
        assistant.content.contains(ALICE),
        "the terse fact must name the evicted edge, got: {:?}",
        assistant.content
    );
    assert!(
        !assistant.content.contains("Alice"),
        "narrative prose must not leak through, got: {:?}",
        assistant.content
    );
}

struct Outcome {
    tool: Option<String>,
    args: String,
    text: String,
}

async fn run_turn(engine: &LlamaChatInferenceEngine, user: &str) -> Outcome {
    let system = "You are a graph-editing assistant. Tasks are assigned to people with the \
        'tasks' relationship (person -> task); a task has at most one assignee. Use ids \
        exactly as they appear in the conversation.";
    let mut messages = vec![ChatMessage::text(Role::System, system.to_string())];
    messages.extend(node_history_from_messages(reassignment_turn()));
    messages.push(ChatMessage::text(Role::User, user.to_string()));

    let request = InferenceRequest {
        messages,
        tools: Some(vec![create_relationship_tool(), search_nodes_tool()]),
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
    Outcome {
        tool: collected.iter().find_map(|c| match c {
            StreamingChunk::ToolCallStart { name, .. } => Some(name.clone()),
            _ => None,
        }),
        args: collected
            .iter()
            .filter_map(|c| match c {
                StreamingChunk::ToolCallArgs { args_json, .. } => Some(args_json.as_str()),
                _ => None,
            })
            .collect(),
        text: collected
            .iter()
            .filter_map(|c| match c {
                StreamingChunk::Token { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    }
}

/// Whether the call restores Alice's assignment: `create_relationship` from
/// her id to the task.
fn restores_previous_holder(o: &Outcome) -> bool {
    if o.tool.as_deref() != Some("create_relationship") {
        return false;
    }
    let Ok(args) = serde_json::from_str::<serde_json::Value>(&o.args) else {
        return false;
    };
    let id = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_start_matches("nodespace://").to_string())
    };
    id("from_id").as_deref() == Some(ALICE.trim_start_matches("nodespace://"))
        && id("to_id").as_deref() == Some(TASK.trim_start_matches("nodespace://"))
}

/// Asking for the previous holder back after a reassignment must restore her.
/// Her id is reachable only through the eviction record in the rendered
/// history: without it, every trial searched for "whoever had it before".
///
/// A bare "Undo that." is deliberately not asserted here. It fails 3/3 even
/// when the fact states the reassignment outright ("reassigned from A to B")
/// — E4B re-issues the new edge instead — so its failure is independent of
/// what history carries, and no fact wording measured moved it.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn putting_it_back_restores_previous_holder() {
    let engine = load_engine();
    println!("=== REAL PIPELINE HISTORY ===");
    for m in node_history_from_messages(reassignment_turn()) {
        println!("{:?}: {}", m.role, m.content);
    }

    let prompts = ["Actually, put it back on whoever had it before."];
    let mut failures = Vec::new();
    for prompt in prompts {
        for trial in 0..TRIALS {
            let o = run_turn(&engine, prompt).await;
            let ok = restores_previous_holder(&o);
            match &o.tool {
                Some(n) => println!("[{prompt:?} #{trial}] {} {n}({})", ok_mark(ok), o.args),
                None => println!("[{prompt:?} #{trial}] {} text: {:?}", ok_mark(ok), o.text),
            }
            if !ok {
                failures.push(format!("{prompt:?} #{trial}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "turns that did not restore the previous holder: {failures:?}"
    );
}

fn ok_mark(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "FAIL"
    }
}
