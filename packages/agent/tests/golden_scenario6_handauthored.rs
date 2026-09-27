//! Hand-authored golden prompt for agent-matrix scenario 6, built independent
//! of ANY NodeSpace prompt-assembly source (no `stage1_system_prompt`, no
//! `skill_pipeline.rs`, no `agent_guidance.rs` constants). Every string below
//! is authored fresh in this file.
//!
//! Methodology per the standing instruction on #1917: assemble a sequence of
//! literal prompt strings, validate directly against llama.cpp with zero
//! NodeSpace plumbing involved, until a version is found that reliably gets
//! the correct tool call. THAT becomes the target the real assembly pipeline
//! is engineered toward — not the other way around.
//!
//! Prior finding this experiment is built on (#1922, subagent investigation):
//! the model resolves "the 2400 one" -> resolve_query correctly with NO prior
//! history, 3/3. It refuses and asks for a literal node id the moment ANY
//! prior turn is present, 3/3, across 5 independent 3-rep trials on 4
//! different prompt channels (guidance reorder, injected write-record facts,
//! tool-description carve-outs, history composition). The scenario's real
//! conversation history is ~8,322 tokens of mostly-boilerplate wrapped around
//! a 10-word request -- consistent with this project's own resident-prompt
//! dilution finding (10,493 chars scored 50% vs 445 chars at 73% on cases
//! targeting the SAME prompt's own rules).
//!
//! This experiment's question: is the defect "any history at all" (unfixable
//! by prompt engineering, a real capability boundary) or "history above some
//! size/shape" (fixable by compacting what's carried forward)? Tests a
//! COMPACT hand-written summary of the same facts scenario 6's real 3-turn
//! history carries, instead of the full turn-by-turn reconstruction.
//!
//! Ignored by default — loads the 5GB locked native GGUF. Run explicitly:
//! ```text
//! cargo test -p nodespace-agent --test golden_scenario6_handauthored -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolDefinition,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
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

/// Hand-authored `resolve_query` tool -- NOT `def_resolve_query()` from
/// `tools.rs`. Deliberately minimal: enough for the model to understand the
/// tool's purpose, none of the accumulated production prose.
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

/// Run a hand-authored turn and print/return the tool call the model made.
async fn run_turn(
    engine: &LlamaChatInferenceEngine,
    system_prompt: &str,
    history: Vec<ChatMessage>,
    user_message: &str,
) -> Option<(String, String)> {
    let mut messages = vec![ChatMessage::text(Role::System, system_prompt.to_string())];
    messages.extend(history);
    messages.push(ChatMessage::text(Role::User, user_message.to_string()));

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
    })?;
    let args: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::ToolCallArgs { args_json, .. } => Some(args_json.as_str()),
            _ => None,
        })
        .collect();
    Some((name, args))
}

/// Baseline control: zero history, minimal hand-authored prompt. Expected
/// (per #1922's finding) to call resolve_query correctly -- confirms this
/// harness's minimal tools/prompt reproduce the known-good case before
/// testing the compact-history variant below.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_zero_history_control() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    let result = run_turn(
        &engine,
        system,
        vec![],
        "The 2400 one came back — set it to returned",
    )
    .await;

    match &result {
        Some((name, args)) => println!("GOLDEN[zero-history] {name}({args})"),
        None => println!("GOLDEN[zero-history] no tool call parsed"),
    }
    assert_eq!(
        result.as_ref().map(|(n, _)| n.as_str()),
        Some("resolve_query"),
        "zero-history control must call resolve_query — if this fails, the harness itself \
         (not scenario 6) has a problem, since #1922 already confirmed this case works"
    );
}

/// The actual experiment: a COMPACT hand-written summary of scenario 6's real
/// history (schema created with a replacement_cost field; one equipment item
/// logged with cost 2400) instead of the full multi-turn reconstruction with
/// tool-call records, write-receipts, and routing candidate blocks. ~120
/// words vs the real turn's ~8,322 tokens.
///
/// If this passes reliably, the defect is prompt SIZE/SHAPE (fixable by
/// compaction) not "any history" (a hard capability boundary). If it fails
/// the same way as the full history, size/shape is refuted and the boundary
/// really is presence-of-any-history.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_compact_history_variant() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    let compact_history = vec![ChatMessage::text(
        Role::System,
        "Context: the user has an 'equipment' type with a replacement_cost field. They logged \
         one equipment item with replacement_cost 2400."
            .to_string(),
    )];

    let result = run_turn(
        &engine,
        system,
        compact_history,
        "The 2400 one came back — set it to returned",
    )
    .await;

    match &result {
        Some((name, args)) => println!("GOLDEN[compact-history] {name}({args})"),
        None => println!("GOLDEN[compact-history] no tool call parsed"),
    }
    // Not asserted pass/fail — this is the discriminating experiment itself.
    // The printed result IS the finding.
}

/// Calibration point on the size/dilution curve between the ~150-token
/// compact golden prompt (passes 3/3) and the real ~7,500-8,500-token full
/// reconstruction (fails 3/3 -- see #1922). Targets roughly the same order of
/// magnitude as ADR-064's own measured resident-prompt ablation (10,493 chars
/// / ~2,600 tokens scored 50% vs a 445-char/~110-token arm at 73%), to check
/// whether scenario 6's degradation follows the same curve shape or breaks at
/// a different scale specific to conversation history.
///
/// Built by padding the compact history with realistic-but-verbose assistant
/// narration (the actual style seen in production traces -- multi-paragraph,
/// numbered options, a question back to the user) repeated/extended to roughly
/// 2,000-2,500 tokens of history, while keeping the same underlying facts
/// (equipment type, replacement_cost field, item logged at 2400) as the only
/// signal the model needs to resolve "the 2400 one".
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_medium_history_calibration() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    // ~2,000-2,500 tokens: realistic verbose assistant narration (matching the
    // actual style captured in production traces this session), padded to
    // land in the same order of magnitude as ADR-064's measured ablation
    // point, well short of the real scenario's ~7,500-8,500 tokens.
    let verbose_turn_3 = "I've created the schema for **Equipment Item** (equipment_item) to \
        track what you have. For tracking checkouts, I also tried to create a second type \
        called \"Equipment Checkout Record,\" but it seems like creating multiple schemas in \
        one go is restricted by my current tools.\n\nTo best capture your needs -- tracking an \
        item *and* its checkout status/cost -- we should use just one type for now:\n1. \
        **`equipment_item`**: To store details about each piece of equipment and its \
        replacement cost.\n\nThe schema now has a single field: replacement_cost (number). You \
        can log items against it whenever you're ready, and we can always add more fields \
        later such as a status field for checked-out vs returned, a location field, or a \
        purchase-date field if that would be useful for your team's tracking needs going \
        forward.";
    let write_record_3 = "Record of graph writes already completed in the previous turn. \
        These are done -- do not repeat them:\n- create_schema \"Equipment Item\" -> \
        nodespace://a1b2c3d4-1111-2222-3333-444455556666";
    let verbose_turn_4 = "I've logged the laser cutter as a new Equipment Item with a \
        replacement cost of 2400. The record now exists in your graph and you can reference it \
        going forward -- for example, if it's ever checked back in, damaged, or needs a cost \
        update, just let me know and I can look it up and make the change for you directly.";
    let write_record_4 = "Record of graph writes already completed in the previous turn. \
        These are done -- do not repeat them:\n- create_node \"Laser Cutter\" -> \
        nodespace://b2c3d4e5-2222-3333-4444-555566667777";

    // Repeat the pattern a few times with slightly varied filler to reach the
    // target token range without changing the underlying facts.
    let history = vec![
        ChatMessage::text(
            Role::User,
            "I want to keep a record of the equipment my team checks out and what each item costs \
             to replace"
                .to_string(),
        ),
        ChatMessage::text(Role::Assistant, verbose_turn_3.to_string()),
        ChatMessage::text(Role::System, write_record_3.to_string()),
        ChatMessage::text(
            Role::User,
            "Log a laser cutter checked out on the 12th, replacement cost 2400".to_string(),
        ),
        ChatMessage::text(Role::Assistant, verbose_turn_4.to_string()),
        ChatMessage::text(Role::System, write_record_4.to_string()),
    ];

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    println!(
        "GOLDEN[medium-history] history char count: {total_chars} (~{} tokens est.)",
        total_chars / 4
    );

    let result = run_turn(
        &engine,
        system,
        history,
        "The 2400 one came back — set it to returned",
    )
    .await;

    match &result {
        Some((name, args)) => println!("GOLDEN[medium-history] {name}({args})"),
        None => println!("GOLDEN[medium-history] no tool call parsed"),
    }
    // Not asserted pass/fail — this is the discriminating experiment itself.
}

/// Isolates SIZE from STYLE at the same token count as
/// `golden_medium_history_calibration` (which failed at ~412 tokens with
/// verbose, narrative assistant turns). This uses TERSE, non-narrative
/// content padded to the same approximate token count -- repeated short
/// factual statements instead of paragraphs, lists, and questions back to
/// the user -- to check whether the ~412-token break point is genuinely a
/// function of size alone, or specifically triggered by conversational/
/// narrative shape (which the real production traces also exhibit, so this
/// matters for whether "compact but still narrative" summarization would
/// actually help, or whether summaries need to be terse/factual specifically).
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_medium_history_terse_control() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    // Terse, repeated factual statements -- no narration, no lists, no
    // questions back to the user -- padded to roughly the same token count
    // (~400-450) as the failing verbose calibration point.
    let mut history = Vec::new();
    let facts = [
        "Fact: an 'equipment' schema was created with fields: replacement_cost (number).",
        "Fact: no other schemas were created in this conversation.",
        "Fact: one equipment_item node was created.",
        "Fact: that node's replacement_cost is 2400.",
        "Fact: that node's title is 'Laser Cutter'.",
        "Fact: no other equipment_item nodes exist yet.",
        "Fact: the equipment schema has exactly one field: replacement_cost.",
        "Fact: the laser cutter was logged as checked out, no status field exists yet.",
        "Fact: node ids are assigned automatically and are not known to the user.",
        "Fact: the user refers to items by their replacement_cost value, not by id.",
    ];
    for f in facts {
        history.push(ChatMessage::text(Role::System, f.to_string()));
    }

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    println!(
        "GOLDEN[terse-control] history char count: {total_chars} (~{} tokens est.)",
        total_chars / 4
    );

    let result = run_turn(
        &engine,
        system,
        history,
        "The 2400 one came back — set it to returned",
    )
    .await;

    match &result {
        Some((name, args)) => println!("GOLDEN[terse-control] {name}({args})"),
        None => println!("GOLDEN[terse-control] no tool call parsed"),
    }
    // Not asserted pass/fail — this is the discriminating experiment itself.
}

/// Isolates SIZE from STYLE at the SAME token count as
/// `golden_medium_history_calibration` (~412 tokens, verbose/narrative,
/// FAILED single-rep). This uses TERSE, non-narrative content -- the same
/// style as `golden_medium_history_terse_control` (149 tokens, PASSED) --
/// but padded with additional (still terse, still true) facts to land in the
/// same ~400-450 token range as the failing verbose calibration point.
///
/// This is the controlled probe: token count matched to the FAILING case,
/// style matched to the PASSING case. If this fails, size is implicated as a
/// driver (matched style, still breaks at this size). If it passes, style is
/// implicated (matched size, terse style still succeeds) -- meaning the
/// verbose calibration point's failure was driven by narrative shape, not
/// raw token count, which has direct design implications for how history
/// should be compacted (terse facts, not narrative summaries).
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn golden_terse_matched_size_probe() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. When a request refers to something \
        indirectly (a bare value, a description) rather than by name, call resolve_query to \
        find it. Never ask the user to supply a node id yourself.";

    // Terse, repeated factual statements -- same style as the terse control,
    // extended with additional true facts (not filler repetition) to reach
    // the ~400-450 token range that matches golden_medium_history_calibration.
    let mut history = Vec::new();
    let facts = [
        "Fact: an equipment schema was created with fields: replacement_cost (number).",
        "Fact: no other schemas were created in this conversation.",
        "Fact: one equipment_item node was created.",
        "Fact: that node's replacement_cost is 2400.",
        "Fact: that node's title is 'Laser Cutter'.",
        "Fact: no other equipment_item nodes exist yet.",
        "Fact: the equipment schema has exactly one field: replacement_cost.",
        "Fact: the laser cutter was logged as checked out, no status field exists yet.",
        "Fact: node ids are assigned automatically and are not known to the user.",
        "Fact: the user refers to items by their replacement_cost value, not by id.",
        "Fact: the schema was created in response to the user's request to track equipment.",
        "Fact: the replacement_cost field is of type number, not string.",
        "Fact: the equipment_item node was created after the schema was created.",
        "Fact: the user's team checks out and returns equipment items over time.",
        "Fact: only one node currently exists under the equipment_item type.",
        "Fact: the node's creation was confirmed successfully with no errors.",
        "Fact: the schema creation was confirmed successfully with no errors.",
        "Fact: the user has not yet added a status property to the schema.",
        "Fact: the assistant has not renamed or deleted any existing fields.",
        "Fact: the conversation so far concerns exactly one equipment item and one schema.",
        "Fact: the equipment_item type id is 'equipment_item', lowercase with an underscore.",
        "Fact: the replacement_cost value 2400 was provided directly by the user.",
        "Fact: the laser cutter is the only item logged so far in this conversation.",
        "Fact: the schema and node were both created successfully in prior turns.",
    ];
    for f in facts {
        history.push(ChatMessage::text(Role::System, f.to_string()));
    }

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    println!(
        "GOLDEN[terse-matched-size] history char count: {total_chars} (~{} tokens est.)",
        total_chars / 4
    );

    let result = run_turn(
        &engine,
        system,
        history,
        "The 2400 one came back — set it to returned",
    )
    .await;

    match &result {
        Some((name, args)) => println!("GOLDEN[terse-matched-size] {name}({args})"),
        None => println!("GOLDEN[terse-matched-size] no tool call parsed"),
    }
    // Not asserted pass/fail — this is the discriminating experiment itself.
}
