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
//! cargo test -p nodespace-agent --test it live_stage1_golden_prompts:: -- --ignored --nocapture
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{ChatInferenceEngine, ChatMessage, InferenceRequest, Role};
use nodespace_agent::local_agent::agent_loop::{
    stage1_system_prompt, stage1_type_names, STAGE1_MAX_TOKENS,
};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::routing::{
    parse_route_decision, stage1_tool_definitions, RouteDecision,
};
use nodespace_nlp_engine::chat::ChatConfig;

/// The skill names production's Stage-1 prompt carries on a freshly seeded
/// registry — what `GraphToolExecutor::skill_names` returns there.
fn seeded_skill_names() -> Vec<String> {
    nodespace_agent::local_agent::agent_loop::stage1_skill_names(
        nodespace_agent::skill_pipeline::seed_skill_nodes()
            .into_iter()
            .map(|t| t.title),
    )
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
    // The prompt production sends in a workspace with no types of the user's
    // own: the skills, and the built-in types the message names.
    let system_prompt =
        stage1_system_prompt(&seeded_skill_names(), &stage1_type_names([], message));
    let routing_query =
        nodespace_agent::local_agent::agent_loop::stage1_query_from_turns(prior_turns, message);
    run_stage1_request(engine, system_prompt, routing_query)
        .await
        .0
}

/// Send one Stage-1 request and return the decision with what it cost: the
/// token counts and the generation's wall-clock time.
async fn run_stage1_request(
    engine: &LlamaChatInferenceEngine,
    system_prompt: String,
    routing_query: String,
) -> (
    Option<RouteDecision>,
    nodespace_agent::agent_types::InferenceUsage,
    std::time::Duration,
) {
    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, system_prompt),
            ChatMessage::text(Role::User, routing_query),
        ],
        tools: Some(stage1_tool_definitions()),
        temperature: Some(0.1),
        max_tokens: Some(STAGE1_MAX_TOKENS),
    };

    let chunks: Arc<std::sync::Mutex<Vec<_>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = chunks.clone();
    let started = std::time::Instant::now();
    let usage = engine
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
    let elapsed = started.elapsed();

    let collected = chunks.lock().expect("chunk mutex").clone();
    let name = collected.iter().find_map(|c| match c {
        nodespace_agent::agent_types::StreamingChunk::ToolCallStart { name, .. } => {
            Some(name.clone())
        }
        _ => None,
    });
    let args_json: String = collected
        .iter()
        .filter_map(|c| match c {
            nodespace_agent::agent_types::StreamingChunk::ToolCallArgs { args_json, .. } => {
                Some(args_json.as_str())
            }
            _ => None,
        })
        .collect();
    let decision = name.and_then(|name| parse_route_decision(&name, &args_json));
    (decision, usage, elapsed)
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

/// A Stage-1 decision, by which routing tool was called.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Decision {
    Query,
    Multi,
    Clarify,
}

/// Why a case is in the type-line measurement. Every case names a type, so
/// its prompt carries the line.
#[derive(Clone, Copy, PartialEq)]
enum Group {
    /// Asks about records of a type. Should route.
    KnownType,
    /// A request whose routing never was in doubt, or one that uses a type's
    /// name as an ordinary word. The line must change neither what Stage 1
    /// decides for it nor which types its query mentions.
    Unrelated,
}

/// One message that names a type, and the decision expected of it where
/// there is one.
struct TypeLineCase {
    group: Group,
    message: &'static str,
    expect: Option<Decision>,
}

const fn case(group: Group, message: &'static str, expect: Option<Decision>) -> TypeLineCase {
    TypeLineCase {
        group,
        message,
        expect,
    }
}

/// The types the user has defined in the measured workspace.
const USER_TYPES: &[&str] = &[
    "Invoice",
    "Customer",
    "Vendor",
    "Incident Report",
    "Feature Writeup",
];

/// The words Stage 1 can be told name a built-in type.
const BUILT_IN_TYPE_WORDS: &[&str] = &[
    "text",
    "date",
    "task",
    "project",
    "person",
    "people",
    "collection",
    "query",
];

const HOW_MANY_TASKS: &str = "Could you tell me how many tasks we have here?";

const TYPE_LINE_CASES: &[TypeLineCase] = &[
    case(Group::KnownType, HOW_MANY_TASKS, Some(Decision::Query)),
    case(
        Group::KnownType,
        "how many people do we have?",
        Some(Decision::Query),
    ),
    case(
        Group::KnownType,
        "what projects are there?",
        Some(Decision::Query),
    ),
    case(
        Group::KnownType,
        "how many invoices are still open?",
        Some(Decision::Query),
    ),
    case(
        Group::KnownType,
        "tell me about the vendors",
        Some(Decision::Query),
    ),
    case(
        Group::KnownType,
        "show me my collections",
        Some(Decision::Query),
    ),
    case(
        Group::KnownType,
        "which tasks are overdue?",
        Some(Decision::Query),
    ),
    // The prompts of the routing eval (`scripts/eval/fixtures/routing.ts`)
    // that name a type, with the Stage-1 decision each expectation implies.
    case(
        Group::Unrelated,
        "Create a new task called 'Review the sync protocol spec'",
        Some(Decision::Query),
    ),
    case(
        Group::Unrelated,
        "Add a task to rotate the staging API keys, and also pull up my notes on the auth redesign",
        Some(Decision::Multi),
    ),
    case(
        Group::Unrelated,
        "Create a task to move the notifications service onto the new queue tomorrow morning, make it high priority, and note on it that the old queue shuts down at noon on Friday",
        Some(Decision::Query),
    ),
    case(
        Group::Unrelated,
        "add a task to call the bank tomorrow",
        Some(Decision::Query),
    ),
    case(
        Group::Unrelated,
        "raise an invoice for Acme and log an incident report about the outage",
        Some(Decision::Multi),
    ),
    // A type's name used as an ordinary word. These get the line too.
    case(
        Group::Unrelated,
        "find the text about onboarding in my notes",
        Some(Decision::Query),
    ),
    case(
        Group::Unrelated,
        "set the due date of the budget review to Friday",
        Some(Decision::Query),
    ),
    case(
        Group::Unrelated,
        "what's the date today?",
        None,
    ),
    case(
        Group::Unrelated,
        "search for notes about the Apollo project kickoff",
        Some(Decision::Query),
    ),
];

/// Lowercase words of `text`, split at anything that is not a letter or digit.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The type words in `queries` that `message` does not use: a type the
/// prompt listed, or could have, that Stage 1 worked into what retrieval
/// will embed. A plural in "s" counts as its singular.
fn type_words_added(message: &str, queries: &[String]) -> Vec<String> {
    let singular = |w: &String| w.strip_suffix('s').unwrap_or(w).to_string();
    let said: Vec<String> = words(message).iter().map(singular).collect();
    let type_words: Vec<String> = BUILT_IN_TYPE_WORDS
        .iter()
        .map(|w| w.to_string())
        .chain(USER_TYPES.iter().flat_map(|name| words(name)))
        .collect();
    let mut added: Vec<String> = queries
        .iter()
        .flat_map(|q| words(q))
        .map(|w| singular(&w))
        .filter(|w| type_words.contains(w) && !said.contains(w))
        .collect();
    added.sort();
    added.dedup();
    added
}

/// Measures what naming a request's types to Stage 1 changes, against the
/// prompt without them, on the same loaded model.
///
/// The two arms differ only in the type line, and take turns going first.
/// Stage 1 is told of a type only when the message names it, so every case
/// here names one. A message that names none sends the prompt it always did;
/// `a_message_naming_no_type_gets_no_type_line` pins that without a model.
///
/// Asserted, in the arm with the line:
/// - every [`Group::KnownType`] case routes on every rep;
/// - every [`Group::Unrelated`] case with an expectation meets it on every
///   rep, with the query it wrote without the line: a type's name read as a
///   record type shows as a reworded query;
/// - no [`Group::Unrelated`] query mentions a type the message did not
///   ([`type_words_added`]), which is what listing a type the message does
///   not name does.
///
/// Every decision is printed, with each arm's prompt size. Latency is
/// compared only over runs where both arms made the same decision, and never
/// over a case's first rep: a clarify call is a question with options,
/// several times the output of a route call, so a mean over all runs measures
/// which arm clarified more.
///
/// As measured on the locked model, four reps per case, every rep of a case
/// deciding alike. Of the seven requests about records of a known type, four
/// routed without the line and all seven with it: "how many people do we
/// have?", "what projects are there?" and "tell me about the vendors" moved
/// from clarify to route. The eight unrelated requests with an expectation
/// met it in both arms, with the same queries. "what's the date today?",
/// which has none, clarified without the line and called no routing tool
/// with it, which the loop treats as a query on the raw message. Mean prompt
/// 792 → 803 tokens; mean generation 2313 ms → 2397 ms over 36 runs, 23
/// tokens generated in each arm.
#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn stage1_type_line_routes_requests_that_name_a_known_type() {
    // Latency leaves rep 0 out, and over the other three the arm with the
    // line goes first twice. Going first after the same arm's own run costs a
    // full prompt evaluation, so the comparison counts against the line.
    const REPS: usize = 4;
    const ARMS: [&str; 2] = ["without", "with"];

    let engine = load_engine();
    let skills = seeded_skill_names();

    // Per case, per arm: the reps that met the case's expectation.
    let mut met = vec![[0usize; 2]; TYPE_LINE_CASES.len()];
    // Per case, per arm: the reps that routed (query or multi).
    let mut routed = vec![[0usize; 2]; TYPE_LINE_CASES.len()];
    // Per case: the type words the with-line arm's queries added.
    let mut added: Vec<Vec<String>> = vec![Vec::new(); TYPE_LINE_CASES.len()];
    // Per case: the reps where the two arms wrote different queries.
    let mut reworded = vec![0usize; TYPE_LINE_CASES.len()];
    let mut prompt_tokens = [0u64; 2];
    let mut same_decision_elapsed = [std::time::Duration::ZERO; 2];
    let mut same_decision_completion_tokens = [0u64; 2];
    let mut same_decision_runs = 0u32;
    let mut runs = 0u64;

    for (index, case) in TYPE_LINE_CASES.iter().enumerate() {
        let types = stage1_type_names(USER_TYPES.iter().map(|t| t.to_string()), case.message);
        let prompts = [
            stage1_system_prompt(&skills, &[]),
            stage1_system_prompt(&skills, &types),
        ];
        assert_ne!(
            prompts[0], prompts[1],
            "{:?} must name a type, or the arms send the same prompt",
            case.message
        );

        for rep in 0..REPS {
            let mut decisions = [None; 2];
            let mut wrote: [Vec<String>; 2] = [Vec::new(), Vec::new()];
            let mut took = [std::time::Duration::ZERO; 2];
            let mut generated = [0u64; 2];
            let order = if rep % 2 == 0 { [0, 1] } else { [1, 0] };
            for arm in order {
                let (decision, usage, elapsed) =
                    run_stage1_request(&engine, prompts[arm].clone(), case.message.to_string())
                        .await;
                prompt_tokens[arm] += u64::from(usage.prompt_tokens);
                took[arm] = elapsed;
                generated[arm] = u64::from(usage.completion_tokens);

                let (kind, queries) = match decision {
                    Some(RouteDecision::Query(q)) => (Some(Decision::Query), vec![q]),
                    Some(RouteDecision::Multi(qs)) => (Some(Decision::Multi), qs),
                    Some(RouteDecision::Clarify { question, .. }) => {
                        println!(
                            "TYPES[{}] rep{rep} {:?} types={types:?} -> clarify({question:?})",
                            ARMS[arm], case.message,
                        );
                        (Some(Decision::Clarify), Vec::new())
                    }
                    None => {
                        println!(
                            "TYPES[{}] rep{rep} {:?} types={types:?} -> no valid tool call",
                            ARMS[arm], case.message,
                        );
                        (None, Vec::new())
                    }
                };
                if !queries.is_empty() {
                    println!(
                        "TYPES[{}] rep{rep} {:?} types={types:?} -> {kind:?}({queries:?}) in {} ms",
                        ARMS[arm],
                        case.message,
                        elapsed.as_millis(),
                    );
                    routed[index][arm] += 1;
                    if arm == 1 {
                        added[index].extend(type_words_added(case.message, &queries));
                    }
                }
                wrote[arm] = queries;
                decisions[arm] = kind;
                if case.expect.is_some() && kind == case.expect {
                    met[index][arm] += 1;
                }
            }
            runs += 1;
            if wrote[0] != wrote[1] {
                reworded[index] += 1;
            }
            // A case's first run without the line follows the previous case's
            // run without it: the same system prompt, already evaluated. That
            // run takes a quarter of the time of any other, in one arm only.
            if rep > 0 && decisions[0].is_some() && decisions[0] == decisions[1] {
                same_decision_runs += 1;
                for arm in [0, 1] {
                    same_decision_elapsed[arm] += took[arm];
                    same_decision_completion_tokens[arm] += generated[arm];
                }
            }
        }
    }

    for (group, label) in [
        (Group::KnownType, "known type, routed"),
        (Group::Unrelated, "unrelated, decided as expected"),
    ] {
        let in_group = || {
            TYPE_LINE_CASES
                .iter()
                .enumerate()
                .filter(move |(_, c)| c.group == group && c.expect.is_some())
        };
        let meeting = |arm: usize| in_group().filter(|(i, _)| met[*i][arm] == REPS).count();
        println!(
            "TYPES SUMMARY {label}: without {}/{n}, with {}/{n} (cases meeting it on every rep)",
            meeting(0),
            meeting(1),
            n = in_group().count(),
        );
    }
    for (case, added) in TYPE_LINE_CASES.iter().zip(&added) {
        if !added.is_empty() {
            println!(
                "TYPES SUMMARY type words added to the query of {:?}: {added:?}",
                case.message
            );
        }
    }
    println!(
        "TYPES SUMMARY mean prompt: without {} tokens, with {} tokens",
        prompt_tokens[0] / runs.max(1),
        prompt_tokens[1] / runs.max(1),
    );
    println!(
        "TYPES SUMMARY mean generation over the {same_decision_runs} runs where both arms \
         decided alike: without {} ms ({} tokens generated), with {} ms ({} tokens generated)",
        same_decision_elapsed[0].as_millis() / u128::from(same_decision_runs.max(1)),
        same_decision_completion_tokens[0] / u64::from(same_decision_runs.max(1)),
        same_decision_elapsed[1].as_millis() / u128::from(same_decision_runs.max(1)),
        same_decision_completion_tokens[1] / u64::from(same_decision_runs.max(1)),
    );

    for (index, case) in TYPE_LINE_CASES.iter().enumerate() {
        match case.group {
            Group::KnownType => assert_eq!(
                routed[index][1], REPS,
                "{:?} names a known type and must route with the type named",
                case.message
            ),
            Group::Unrelated => {
                if case.expect.is_some() {
                    assert_eq!(
                        met[index][1], REPS,
                        "{:?} was never in doubt and must decide as expected with its type \
                         named (without the line: {} of {REPS})",
                        case.message, met[index][0]
                    );
                    assert_eq!(
                        reworded[index], 0,
                        "{:?} was never in doubt, and naming its type changed the query \
                         Stage 1 wrote for it",
                        case.message
                    );
                }
                assert!(
                    added[index].is_empty(),
                    "naming a type to Stage 1 put {:?} into the query for {:?}, which does \
                     not mention it",
                    added[index],
                    case.message
                );
            }
        }
    }
}
