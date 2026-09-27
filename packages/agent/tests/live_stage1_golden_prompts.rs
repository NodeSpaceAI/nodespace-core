//! Golden-prompt harness for Stage-1 routing (ADR-038) — step 1 of the
//! deterministic prompt-assembly snapshot-test deliverable tracked on #1917.
//!
//! This is deliberately the SMALLEST possible call: build the exact
//! `stage1_system_prompt` + `stage1_tool_definitions()` request
//! `agent_loop.rs`'s `route` function sends, call `LlamaChatInferenceEngine`
//! directly (in-process, no daemon, no chat-node lifecycle, no DB), and parse
//! the resulting tool call. No retrieval, no Stage 2, no tool execution.
//!
//! Purpose: separate "does this exact prompt text get the right tool call"
//! (answerable here, in seconds, no daemon restart or DB purge) from "does the
//! real pipeline assemble that exact prompt from real inputs" (a distinct,
//! deterministic, zero-model-call question — the snapshot tests this golden
//! set feeds, not yet written). The full live-model matrix (`bun run
//! eval:agent`) stays reserved for the end-to-end gate before merging a change
//! to shared surface, per this session's own rule 3 — not for iterating on
//! individual hypotheses, which is what made every prior checkpoint expensive.
//!
//! Ignored by default — loads a 5GB GGUF and needs the model on disk at the
//! standard NodeSpace catalog path. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test live_stage1_golden_prompts -- --ignored --nocapture
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{ChatInferenceEngine, ChatMessage, InferenceRequest, Role};
use nodespace_agent::local_agent::agent_loop::{stage1_system_prompt, STAGE1_MAX_TOKENS};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::routing::{
    parse_route_decision, stage1_tool_definitions, RouteDecision,
};
use nodespace_nlp_engine::chat::ChatConfig;

/// The skill names production's Stage-1 prompt carries on a freshly seeded
/// registry — what `GraphToolExecutor::skill_names` returns there.
fn seeded_skill_names() -> Vec<String> {
    let mut names: Vec<String> = nodespace_agent::skill_pipeline::seed_skill_nodes()
        .into_iter()
        .map(|t| t.title)
        .collect();
    names.sort();
    names
}

/// Standard on-disk path for the locked native model (ADR-056), matching
/// `model_manager.rs`'s catalog filename under the NodeSpace home directory.
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
    LlamaChatInferenceEngine::load(
        &model_path(),
        nodespace_agent::agent_types::ModelFamily::Gemma4,
        config,
    )
    .expect("model must load from the standard catalog path — see model_path()")
}

/// Run the exact Stage-1 request `agent_loop.rs::route` sends for `message`,
/// with no prior turns. Returns the raw generated tool-call arguments so a
/// caller can inspect the reformulated query verbatim, not just the decision.
async fn run_stage1(engine: &LlamaChatInferenceEngine, message: &str) -> Option<RouteDecision> {
    run_stage1_with_history(engine, &[], message).await
}

/// Same as `run_stage1`, but blends `prior_turns` into the routing query the
/// same way `agent_loop.rs::stage1_query` does for a real multi-turn
/// conversation, via `agent_loop::stage1_query_from_turns` — the exact
/// function production calls (including the PRIOR CONTEXT / CURRENT REQUEST
/// framing added for #1909, not just the raw `build_retrieval_query` blend).
/// Needed for golden cases (like scenario 6) whose discriminating detail only
/// exists relative to earlier turns.
async fn run_stage1_with_history(
    engine: &LlamaChatInferenceEngine,
    prior_turns: &[&str],
    message: &str,
) -> Option<RouteDecision> {
    let routing_query =
        nodespace_agent::local_agent::agent_loop::stage1_query_from_turns(prior_turns, message);
    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, stage1_system_prompt(&seeded_skill_names())),
            ChatMessage::text(Role::User, routing_query),
        ],
        tools: Some(stage1_tool_definitions()),
        temperature: Some(0.1),
        max_tokens: Some(STAGE1_MAX_TOKENS),
    };

    let chunks: Arc<std::sync::Mutex<Vec<_>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
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
        .expect("Stage-1 generation must complete");

    let collected = chunks.lock().expect("chunk mutex").clone();
    let name = collected.iter().find_map(|c| match c {
        nodespace_agent::agent_types::StreamingChunk::ToolCallStart { name, .. } => {
            Some(name.clone())
        }
        _ => None,
    })?;
    let args_json: String = collected
        .iter()
        .filter_map(|c| match c {
            nodespace_agent::agent_types::StreamingChunk::ToolCallArgs { args_json, .. } => {
                Some(args_json.as_str())
            }
            _ => None,
        })
        .collect();
    parse_route_decision(&name, &args_json)
}

/// Golden case for the 8a-cascade root cause (#1917 checkpoints 2-6): Stage 1
/// reformulates "Start tracking albums I mean to listen to" into a query that
/// discards the "new kind of thing" intent, which then retrieves
/// Organization/Node Creation instead of Schema Creation. This test pins what
/// Stage 1 ACTUALLY generates for this exact message today, so any future
/// prompt-content fix (a change to stage1_system_prompt or
/// stage1_tool_definitions' descriptions) can be iterated against this in
/// seconds, and the change verified here BEFORE spending a full matrix gate.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn stage1_reformulation_for_start_tracking_albums() {
    let engine = load_engine();
    let decision = run_stage1(&engine, "Start tracking albums I mean to listen to").await;

    match decision {
        Some(RouteDecision::Query(q)) => {
            println!("GOLDEN[8a] route_query(\"{q}\")");
            // Documents current (defective) behavior rather than asserting a
            // fix: this is the golden-set BASELINE capture, not a pass/fail
            // gate. A future fix attempt re-runs this test and diffs the
            // printed query against this comment's recorded value:
            //   as of main@db708a90: "create listening queue or watchlist for music albums"
            //   (embeds at 0.90 against Organization/Node Creation/Bulk Import,
            //   never surfaces Schema Creation — see #1912's refutation and
            //   #1917 checkpoint 2 for the full retrieval trace)
        }
        Some(RouteDecision::Clarify { question, .. }) => {
            println!("GOLDEN[8a] route_clarify(\"{question}\")");
        }
        Some(RouteDecision::Multi(qs)) => println!("GOLDEN[8a] route_multi({qs:?})"),
        None => println!("GOLDEN[8a] no valid tool call parsed"),
    }
}

/// Control case: the sibling prompt that DOES route correctly today, so a
/// content fix can be checked against both in the same cheap pass — a fix
/// that "solves" 8a by accident regressing 8b is exactly the failure shape
/// #1904 shipped and #1911 caught only via a full matrix run. This test lets
/// that regression be caught in seconds instead.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn stage1_reformulation_for_venue_tracker_control() {
    let engine = load_engine();
    let decision = run_stage1(&engine, "I also need a tracker for the venues I book").await;

    match decision {
        Some(RouteDecision::Query(q)) => {
            println!("GOLDEN[8b control] route_query(\"{q}\")");
            // as of main@db708a90: "venue booking tracker" (embeds at 0.77,
            // correctly surfaces Schema Creation)
        }
        Some(RouteDecision::Clarify { question, .. }) => {
            println!("GOLDEN[8b control] route_clarify(\"{question}\")");
        }
        Some(RouteDecision::Multi(qs)) => println!("GOLDEN[8b control] route_multi({qs:?})"),
        None => println!("GOLDEN[8b control] no valid tool call parsed"),
    }
}

/// Golden case for scenario 4's fragility (regressed under 2 of 4 tried
/// retrieval-merge variants — #1917 checkpoint 15). Capturing Stage 1's
/// reformulation here lets a future fix be checked against this specific
/// sensitivity without a full matrix run.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn stage1_reformulation_for_instance_creation_scenario_4() {
    let engine = load_engine();
    let decision = run_stage1(
        &engine,
        "Log a laser cutter checked out on the 12th, replacement cost 2400",
    )
    .await;

    match decision {
        Some(RouteDecision::Query(q)) => println!("GOLDEN[4] route_query(\"{q}\")"),
        Some(RouteDecision::Clarify { question, .. }) => {
            println!("GOLDEN[4] route_clarify(\"{question}\")");
        }
        Some(RouteDecision::Multi(qs)) => println!("GOLDEN[4] route_multi({qs:?})"),
        None => println!("GOLDEN[4] no valid tool call parsed"),
    }
}

/// Golden case for scenario 6's real root cause (#1922, corrected): Stage 1's
/// reformulated query drops the update/state-change intent and the
/// discriminating value ("2400", "returned"), producing a generic listing-
/// style description that never routes to Graph Editing (the skill that
/// whitelists `resolve_query`). Confirmed against the actual daemon.log
/// behind #1917's 9.0/12 matrix run: real Stage-1 input for this turn
/// (blended per `build_retrieval_query`, matching production exactly)
/// produced `route_query("equipment items on the books")` — the same shape
/// as scenario 5's own list query, with "2400"/"returned" gone entirely.
///
/// Prior-turn text below is trimmed to what `build_retrieval_query` actually
/// sees (the reply is truncated per `MAX_CHARS_PER_BLENDED_TURN`, but full
/// text is harmless here since it's well under that cap for this case).
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn stage1_reformulation_for_scenario_6_update() {
    let engine = load_engine();
    let prior_turns = [
        "Log a laser cutter checked out on the 12th, replacement cost 2400",
        "I've logged the laser cutter as checked out on the 12th, with a replacement cost of 2400.",
    ];
    let decision = run_stage1_with_history(
        &engine,
        &prior_turns,
        "The 2400 one came back — set it to returned",
    )
    .await;

    match decision {
        Some(RouteDecision::Query(q)) => {
            println!("GOLDEN[6] route_query(\"{q}\")");
            // Pre-fix (main@6321f3d5, before this test's own commit): reliably
            // reproduced "equipment items on the books" or similar generic
            // listing phrasing — the failure this test exists to catch. Assert
            // the fix's actual claim (route_query's description now preserves
            // action/intent alongside the subject noun) rather than only
            // printing the outcome for a human to eyeball — an un-asserted
            // golden test stays green through a regression back to the
            // pre-fix behavior, which is exactly the failure mode this test
            // exists to catch. Confirmed live against the real model on
            // 2026-08-02: post-fix output was "set laser cutter to returned
            // with replacement cost 2400" — both markers present.
            let lower = q.to_lowercase();
            assert!(
                lower.contains("2400"),
                "route_query dropped the discriminating value (2400): {q:?}"
            );
            assert!(
                lower.contains("return") || lower.contains("update") || lower.contains("set"),
                "route_query dropped the update/state-change intent: {q:?}"
            );
        }
        Some(RouteDecision::Clarify { question, .. }) => {
            panic!(
                "GOLDEN[6] expected route_query (preserving update intent), got \
                 route_clarify(\"{question}\") instead — Stage 1 should be able to \
                 describe this request as a capability, not ask for clarification."
            );
        }
        Some(RouteDecision::Multi(qs)) => {
            panic!(
                "GOLDEN[6] expected route_query for this single-intent update, got \
                 route_multi({qs:?}) instead — this turn has one intent, not several."
            );
        }
        None => panic!("GOLDEN[6] Stage 1 called no tool or emitted unparseable arguments"),
    }
}
