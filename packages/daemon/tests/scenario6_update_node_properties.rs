//! Diagnoses and locks the scenario-6 residual gap: after `resolve_query`
//! correctly resolves "the 2400 one", the follow-on `update_node` call must
//! carry the requested state change in `field_values` — not just the `id`.
//!
//! The observed failure was `update_node({"id": "..."})` with no
//! `field_values`, so the requested change silently no-ops. The node is right
//! and the tool is right; only the payload is empty.
//!
//! The write parameter is named `field_values`, not `properties`: a
//! tool parameter literally named `properties` collides with JSON Schema's own
//! `properties` keyword and is silently dropped by the Gemma-4 chat template
//! before the model ever sees it.
//!
//! Everything upstream of the failing step is REAL, so a pass here is evidence
//! about production rather than about a hand-authored fixture:
//!   * turn-1/turn-2 history is rendered by the real
//!     `completed_writes_from` + `node_history_from_messages` pipeline
//!     (same construction as `golden_scenario6_real_pipeline.rs`),
//!   * the `resolve_query` tool result is the real `exec_resolve_query`
//!     success payload shape (`resolved/id/title/type/properties`) serialized
//!     through the real `prompt_templates::format_tool_result`,
//!   * the assistant tool-call turn and tool-result turn are appended with the
//!     real `ChatMessage::assistant_with_tool_calls` / `::tool_result`
//!     constructors the agent loop uses,
//!   * the `update_node` tool definition is the REAL production
//!     `ToolDefinition` from the agent's tool registry — not a hand-authored
//!     copy — so a schema regression fails this test.
//!
//! Ignored by default — loads the 5GB locked native GGUF. Run explicitly:
//! ```text
//! cargo test -p nodespace-daemon --test scenario6_update_node_properties -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolCallRaw, ToolDefinition, ToolExecutionRecord,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::prompt_templates;
use nodespace_agent::local_agent::tools::all_tool_definitions;
use nodespace_core::models::AiChatMessage;
use nodespace_daemon::services::local_agent_service::{
    completed_writes_from, node_history_from_messages,
};
use nodespace_nlp_engine::chat::ChatConfig;

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

fn exec(name: &str, args: serde_json::Value, result: serde_json::Value) -> ToolExecutionRecord {
    ToolExecutionRecord {
        tool_call_id: format!("tc_{name}"),
        name: name.to_string(),
        args,
        result,
        is_error: false,
        duration_ms: 1,
    }
}

/// Turns 1 and 2 of the golden sequence, in their real persisted shape, using
/// the actual outputs recorded for the golden sequence.
fn turn1_and_turn2_messages() -> Vec<AiChatMessage> {
    let turn1_writes = completed_writes_from(&[exec(
        "create_schema",
        serde_json::json!({
            "name": "Equipment Checkout Record",
            "fields": [
                {"name": "isReturned", "type": "boolean"},
                {"name": "replacementCost", "type": "number"}
            ]
        }),
        serde_json::json!({
            "schema_id": "equipment_checkout_record",
            "is_core": false,
            "version": 1
        }),
    )]);

    let turn2_writes = completed_writes_from(&[exec(
        "create_node",
        serde_json::json!({
            "node_type": "equipment_checkout_record",
            "content": "Laser Cutter",
            "field_values": {"isReturned": false, "replacementCost": 2400}
        }),
        serde_json::json!({"id": "nodespace://laser-cutter-node"}),
    )]);

    vec![
        AiChatMessage {
            role: "user".to_string(),
            content: "I want to keep a record of the equipment my team checks out, whether \
                it's been returned, and what each item costs to replace"
                .to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        },
        AiChatMessage {
            role: "assistant".to_string(),
            content: "I've created an Equipment Checkout Record type for you.".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: turn1_writes,
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        },
        AiChatMessage {
            role: "user".to_string(),
            content: "Log a laser cutter checked out on the 12th, replacement cost 2400"
                .to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        },
        AiChatMessage {
            role: "assistant".to_string(),
            content: "Logged it — Laser Cutter, replacement cost $2,400.".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: turn2_writes,
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        },
    ]
}

/// The REAL production `update_node` definition, pulled from the agent's tool
/// registry. If its schema regresses, this test regresses with it.
fn production_update_node_tool() -> ToolDefinition {
    all_tool_definitions()
        .into_iter()
        .find(|d| d.name == "update_node")
        .expect("update_node must be a registered tool")
}

/// The success payload `exec_resolve_query` returns on a unique match, for the
/// laser-cutter node: `resolved/id/title/type/properties`.
fn resolve_query_result() -> serde_json::Value {
    serde_json::json!({
        "resolved": true,
        "id": "nodespace://laser-cutter-node",
        "title": "Laser Cutter",
        "type": "equipment_checkout_record",
        "properties": {"isReturned": false, "replacementCost": 2400}
    })
}

/// Build the exact message stack the production agent loop holds at the moment
/// of the failing step: real history, the user's turn-3 request, the assistant's
/// `resolve_query` tool call, and that call's real result.
fn messages_at_failing_step() -> Vec<ChatMessage> {
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    let mut messages = vec![ChatMessage::text(Role::System, system.to_string())];
    messages.extend(node_history_from_messages(turn1_and_turn2_messages()));
    messages.push(ChatMessage::text(
        Role::User,
        "The 2400 one came back — set it to returned".to_string(),
    ));

    // The assistant's resolve_query call, appended the way the loop appends it
    // (empty content, tool call carried in `tool_calls`).
    messages.push(ChatMessage::assistant_with_tool_calls(
        String::new(),
        vec![ToolCallRaw {
            id: "tc_resolve_query".to_string(),
            function_name: "resolve_query".to_string(),
            arguments_json: serde_json::json!({
                "request": "The 2400 one came back — set it to returned",
                "node_type": "equipment_checkout_record"
            })
            .to_string(),
            provider_extra: None,
        }],
    ));

    // Its result, serialized by the real formatter the loop uses.
    messages.push(ChatMessage::tool_result(
        prompt_templates::format_tool_result("resolve_query", &resolve_query_result(), false),
        "tc_resolve_query".to_string(),
        "resolve_query".to_string(),
    ));

    messages
}

struct Turn {
    name: Option<String>,
    args: String,
    raw_text: String,
}

async fn run_step(engine: &LlamaChatInferenceEngine, messages: Vec<ChatMessage>) -> Turn {
    let request = InferenceRequest {
        messages,
        tools: Some(vec![production_update_node_tool()]),
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
    Turn {
        name: collected.iter().find_map(|c| match c {
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
        raw_text: collected
            .iter()
            .filter_map(|c| match c {
                StreamingChunk::Token { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    }
}

/// The gap itself: given a correctly resolved node, the follow-on `update_node`
/// call must actually carry the state change. Asserting only the tool name and
/// id would pass on the exact bug this test exists to catch.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn update_node_after_resolve_carries_the_changed_property() {
    let engine = load_engine();

    // Three trials: a single sample cannot distinguish a fix from a lucky draw
    // on a small model, and this cluster's prior findings were reported as k/3.
    let mut carried = 0;
    for trial in 1..=3 {
        let turn = run_step(&engine, messages_at_failing_step()).await;
        match &turn.name {
            Some(n) => println!("UPDATE-STEP[trial {trial}] {n}({})", turn.args),
            None => println!(
                "UPDATE-STEP[trial {trial}] no tool call parsed, raw: {:?}",
                turn.raw_text
            ),
        }

        assert_eq!(
            turn.name.as_deref(),
            Some("update_node"),
            "trial {trial}: the resolved node must be acted on with update_node"
        );

        let args: serde_json::Value = serde_json::from_str(&turn.args).unwrap_or_else(|e| {
            panic!(
                "trial {trial}: args must be valid JSON ({e}): {}",
                turn.args
            )
        });

        assert_eq!(
            args.get("id").and_then(|v| v.as_str()),
            Some("nodespace://laser-cutter-node"),
            "trial {trial}: must target the resolved node id, got: {args}"
        );

        // The actual gap: `field_values` must be present AND must set the
        // return state. An empty object, or one that only echoes
        // replacementCost, leaves the user's request unperformed.
        let returned = args
            .get("field_values")
            .and_then(|p| p.get("isReturned"))
            .cloned();
        if returned == Some(serde_json::Value::Bool(true)) {
            carried += 1;
        } else {
            println!(
                "UPDATE-STEP[trial {trial}] MISSING state change; field_values = {:?}",
                args.get("field_values")
            );
        }
    }

    assert_eq!(
        carried, 3,
        "update_node must carry field_values.isReturned = true on every trial \
         (got {carried}/3) — resolving the right node but sending no field_values \
         silently drops the user's requested change"
    );
}
