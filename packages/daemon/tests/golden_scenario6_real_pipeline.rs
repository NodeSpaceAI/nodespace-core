//! Validates the REAL history-rendering pipeline
//! (`nodespace_daemon::services::local_agent_service::{completed_writes_from,
//! node_history_from_messages}`) against the confirmed golden sequence for
//! agent-matrix scenario 6.
//!
//! `packages/agent/tests/golden_scenario6_sequence.rs` established, with
//! hand-authored terse "Fact: ..." history strings, that turn 3 of the
//! sequence reliably calls `resolve_query` when given turn 1's and turn 2's
//! actual outputs summarized as terse facts (CONFIRMED 3/3 in both the
//! isolated and chained forms). That file deliberately reuses no production
//! assembly code — every string there is hand-authored, so it validates the
//! *shape* history needs to have, not that the real pipeline produces it.
//!
//! This file closes that gap: it builds the SAME turn-1/turn-2
//! `ToolExecutionRecord`s the real agent loop would have produced (matching
//! golden turn 1's and turn 2's actual recorded outputs — schema id
//! `equipment_checkout_record`, fields `isReturned`/`replacementCost`, one
//! `create_node` at `replacementCost: 2400`), runs them through the REAL
//! `completed_writes_from` + `node_history_from_messages`, and feeds the
//! resulting `ChatMessage` history into turn 3 against bare llama.cpp — same
//! harness pattern as `golden_scenario6_sequence.rs`, no daemon involved.
//!
//! Ignored by default — loads the 5GB locked native GGUF. Run explicitly:
//! ```text
//! cargo test -p nodespace-daemon --test golden_scenario6_real_pipeline -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolDefinition, ToolExecutionRecord,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
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

fn hand_authored_resolve_query_tool() -> ToolDefinition {
    ToolDefinition {
        name: "resolve_query".into(),
        description: "Resolve an indirect reference (a bare value, a relative date, a \
            description) to the single node it refers to. Returns the node directly -- do not \
            call search_nodes afterward."
            .into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "request": {"type": "string", "description": "The user's request, verbatim."},
                "node_type": {"type": "string", "description": "The target node type id."}
            },
            "required": ["request", "node_type"]
        }),
    }
}

fn hand_authored_update_node_tool() -> ToolDefinition {
    ToolDefinition {
        name: "update_node".into(),
        description: "Update an existing node's properties, given its id.".into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "properties": {"type": "object"}
            },
            "required": ["id"]
        }),
    }
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

/// Build the real, persisted-shape history for turns 1 and 2 of the golden
/// sequence, using their ACTUAL confirmed outputs (recorded in
/// `golden_scenario6_sequence.rs`'s doc comments):
///   turn 1: create_schema(name="Equipment Checkout Record",
///     fields=[isReturned: boolean, replacementCost: number])
///     -> schemaId "equipment_checkout_record"
///   turn 2: create_node(node_type="equipment_checkout_record",
///     field_values={isReturned: false, replacementCost: 2400})
///     -> nodespace://laser-cutter-node
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
        // camelCase, matching what `create_schema` actually puts on the wire:
        // `CreateSchemaOutput` carries `#[serde(rename_all = "camelCase")]`.
        // This fixture previously spelled these keys in snake_case, which no
        // production result ever uses — and `completed_writes_from` read the
        // schema id under the same wrong spelling, so the two agreed with each
        // other and disagreed with production. The test passed while the real
        // pipeline silently rendered the id-less phrasing of the fact. Keeping
        // the fixture in the real wire shape is what makes this test's
        // assertion about the schema id mean anything.
        serde_json::json!({
            "schemaId": "equipment_checkout_record",
            "isCore": false,
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
            content: "I've created an Equipment Checkout Record type for you, with fields to \
                track whether an item has been returned and what it costs to replace. You can \
                now log individual pieces of equipment against it whenever your team checks \
                something out."
                .to_string(),
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
            content: "Logged it — I've added a Laser Cutter checkout record with a replacement \
                cost of $2,400. Let me know if you'd like to record when it's returned."
                .to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: turn2_writes,
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        },
    ]
}

/// Print exactly what the real pipeline renders for turns 1-2, so the
/// comment-log evidence for this validation shows actual output, not just a
/// pass/fail.
#[test]
fn real_pipeline_renders_terse_facts_not_narrative_prose() {
    let history = node_history_from_messages(turn1_and_turn2_messages());
    for (i, m) in history.iter().enumerate() {
        println!("[{i}] {:?}: {}", m.role, m.content);
    }

    // The two assistant turns must be rendered as terse facts, not their
    // original narrative content -- this is the whole fix.
    let assistant_msgs: Vec<&ChatMessage> = history
        .iter()
        .filter(|m| matches!(m.role, Role::Assistant))
        .collect();
    assert_eq!(
        assistant_msgs.len(),
        2,
        "expected exactly 2 assistant turns"
    );
    assert!(
        assistant_msgs[0].content.starts_with("Fact:"),
        "turn 1 assistant content must be a terse fact, got: {:?}",
        assistant_msgs[0].content
    );
    assert!(
        assistant_msgs[0]
            .content
            .contains("equipment_checkout_record"),
        "turn 1 fact must carry the derived schema id, got: {:?}",
        assistant_msgs[0].content
    );
    assert!(
        !assistant_msgs[0].content.contains("I've created"),
        "narrative prose must not leak through, got: {:?}",
        assistant_msgs[0].content
    );
    assert!(
        assistant_msgs[1].content.starts_with("Fact:"),
        "turn 2 assistant content must be a terse fact, got: {:?}",
        assistant_msgs[1].content
    );
    assert!(
        assistant_msgs[1].content.contains("2400"),
        "turn 2 fact must carry the replacement cost, got: {:?}",
        assistant_msgs[1].content
    );
}

/// The actual golden-sequence check: feed the REAL pipeline's rendered
/// history for turns 1+2 into turn 3, against bare llama.cpp, and confirm it
/// reproduces the golden sequence's `resolve_query` call.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_turn3_given_real_pipeline_history() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    let prior_history = node_history_from_messages(turn1_and_turn2_messages());
    println!("=== REAL PIPELINE HISTORY FED INTO TURN 3 ===");
    for m in &prior_history {
        println!("{:?}: {}", m.role, m.content);
    }

    let mut messages = vec![ChatMessage::text(Role::System, system.to_string())];
    messages.extend(prior_history);
    messages.push(ChatMessage::text(
        Role::User,
        "The 2400 one came back — set it to returned".to_string(),
    ));

    let request = InferenceRequest {
        messages,
        tools: Some(vec![
            hand_authored_resolve_query_tool(),
            hand_authored_update_node_tool(),
        ]),
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
    let raw_text: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    match &name {
        Some(n) => println!("REAL-PIPELINE[turn3] {n}({args})"),
        None => println!("REAL-PIPELINE[turn3] no tool call parsed, raw: {raw_text:?}"),
    }

    assert_eq!(
        name.as_deref(),
        Some("resolve_query"),
        "turn 3 must call resolve_query given the REAL pipeline's rendered turn1+turn2 history"
    );
}
