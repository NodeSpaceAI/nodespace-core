//! Live check for #1909: does Stage 1 (ADR-038) have a correct move for a
//! compound, unambiguous request — one message carrying two or more distinct
//! intents, none of which is unclear on its own?
//!
//! `routing.rs` offers exactly two Stage-1 tools: `route_query` (one
//! capability, one search string) and `route_clarify` (the request is too
//! ambiguous to describe). Neither has an obviously correct answer for "log
//! this expense and remind me to follow up Friday" — nothing is ambiguous
//! about either half, there are just two of them. This is a structural gap
//! identified by reading the code, not yet a measured defect; this test is
//! the live measurement #1909 asks for before proposing a fix.
//!
//! Fires production's exact Stage-1 request — `stage1_system_prompt`,
//! `stage1_tool_definitions()`, temperature 0.1, `STAGE1_MAX_TOKENS` — against
//! the **locked native model, loaded in-process** (ADR-056: Gemma 4 E4B via
//! llama.cpp), for a wider set of compound-but-unambiguous prompts than the
//! first pass (#1909's PR #1916 shipped 5; this widens to cover varied
//! phrasing, 3-intent requests, and mutating/read-only combinations, per the
//! issue's own follow-up criterion — n=5 confirmed the structural gap exists
//! but could not size how often the model degrades a merged query). Records
//! the raw tool call plus the parsed `RouteDecision`, and tallies a
//! degradation rate across the run. No retrieval, no Stage 2: this isolates
//! Stage 1's own choice, which is what the issue is about.
//!
//! Deliberately native, not OpenAI-compatible/HTTP — `routing_latency.rs`
//! documents why a served stand-in (even a served copy of the same weights)
//! measures a different stack and does not describe what ships. This file
//! reuses that module's `resolve_backend` pattern so the two probes cannot
//! drift on how the locked model is loaded.
//!
//! Ignored by default — loads a ~9.6GB GGUF and runs many live generations,
//! too slow for the default `test:all` run. Skips gracefully (rather than
//! failing) when the locked model is not downloaded. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test it live_stage1_compound_intent:: -- --ignored --nocapture
//! ```
//!
//! **A single run is still not decision-grade** (see #1862's standing lesson
//! on single-run matrix numbers). This test reports a rate for one run; if
//! the rate matters for `route_multi`'s design, repeat the run and compare
//! before treating any single number as ground truth.

use nodespace_agent::agent_types::{
    ChatInferenceEngine, InferenceRequest, ModelFamily, StreamingChunk,
};
use nodespace_agent::local_agent::agent_loop::{stage1_system_prompt, STAGE1_MAX_TOKENS};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::model_manager::GgufModelManager;
use nodespace_agent::local_agent::routing::{
    self, RouteDecision, ROUTE_CLARIFY_TOOL, ROUTE_MULTI_TOOL, ROUTE_QUERY_TOOL,
};
use nodespace_nlp_engine::chat::types::{ChatMessage, Role};
use nodespace_nlp_engine::ChatConfig;
use std::sync::{Arc, Mutex};

/// The skill names production's Stage-1 prompt carries on a freshly seeded
/// registry — what `GraphToolExecutor::skill_names` returns there.
fn seeded_skill_names() -> Vec<String> {
    nodespace_agent::local_agent::agent_loop::stage1_skill_names(
        nodespace_agent::skill_pipeline::seed_skill_nodes()
            .into_iter()
            .map(|t| t.title),
    )
}

/// The locked native agent model (ADR-056). This probe measures only this
/// model — a routing result on anything else does not describe what ships.
const LOCKED_NATIVE_MODEL: &str = "gemma-4-e4b-q4km";

/// Load the locked native model in-process, the way NodeSpace runs it.
///
/// Mirrors `routing_latency.rs`'s `resolve_backend` exactly: there is
/// deliberately no OpenAI-compatible/HTTP path here, for the same reason that
/// file documents — measuring through a served stand-in measures a different
/// stack, on whatever model that server happens to have loaded, and has
/// already produced a suppression result on this project that turned out not
/// to apply to the shipped model.
async fn resolve_locked_model() -> Option<Arc<dyn ChatInferenceEngine>> {
    let gguf = GgufModelManager::new().ok()?;
    let path = gguf.model_path(LOCKED_NATIVE_MODEL).ok()?;
    if !path.exists() {
        return None;
    }
    let path_str = path.to_string_lossy().to_string();
    let engine = tokio::task::spawn_blocking(move || {
        LlamaChatInferenceEngine::load(&path_str, ModelFamily::Gemma4, ChatConfig::default())
    })
    .await
    .ok()?
    .ok()?;
    Some(Arc::new(engine) as Arc<dyn ChatInferenceEngine>)
}

/// A compound, unambiguous prompt: a label, the prompt text, and the keyword
/// pair used to flag whether a merged query kept both intents. Keeping the
/// keyword pair alongside the prompt (rather than a separate lookup keyed on
/// the label) makes the pairing unmissable by construction — a prompt added
/// here without keywords is a compile error, not a silently dropped trace
/// hint from a `match` with a `_ => None` fallback.
struct CompoundPrompt {
    label: &'static str,
    prompt: &'static str,
    /// Substrings expected to survive into a faithful merged query, one per
    /// intent. Two intents get two keywords; three get three. A crude
    /// heuristic, not a verdict — see [`mentions_all`].
    keywords: &'static [&'static str],
}

/// Compound, unambiguous prompts: two or more distinct operations, no
/// ambiguity about any of them. Widened past the original 5 (#1909's PR
/// #1916) to cover varied phrasing, 3-intent requests, and mutating/
/// read-only combinations, so a degradation rate has more than one data
/// point per shape to rest on.
const COMPOUND_PROMPTS: &[CompoundPrompt] = &[
    // -- original 5, phrasing unchanged for comparability across runs --
    CompoundPrompt {
        label: "expense + reminder",
        prompt: "Log a $42 lunch expense and remind me to follow up with Sarah on Friday.",
        keywords: &["expense", "remind"],
    },
    CompoundPrompt {
        label: "search + create",
        prompt: "Find my notes about the Q3 budget and create a task to review them tomorrow.",
        keywords: &["budget", "task"],
    },
    CompoundPrompt {
        label: "two tasks",
        prompt: "Add a task to call the plumber and add another task to pay the electric bill.",
        keywords: &["plumber", "electric"],
    },
    CompoundPrompt {
        label: "note + task",
        prompt: "Write down that the client meeting moved to 3pm and add a task to update the \
                  calendar invite.",
        keywords: &["meeting", "calendar"],
    },
    CompoundPrompt {
        label: "delete + search",
        prompt: "Delete the draft invoice for Acme Corp and find all other invoices from last \
                  month.",
        keywords: &["delete", "invoice"],
    },
    // -- widened set: varied phrasing --
    CompoundPrompt {
        label: "create + create (varied phrasing)",
        prompt: "I need a new task for renewing the passport, plus jot down that the dentist \
                  moved my appointment to next Tuesday.",
        keywords: &["passport", "dentist"],
    },
    CompoundPrompt {
        label: "search + delete (varied phrasing)",
        prompt: "Can you pull up everything tagged 'onboarding' and also get rid of the old \
                  draft proposal for Meridian?",
        keywords: &["onboarding", "proposal"],
    },
    // -- widened set: 3-intent requests --
    CompoundPrompt {
        label: "three tasks",
        prompt: "Add a task to renew my passport, add a task to book the flight, and add a task \
                  to email the hotel about early check-in.",
        keywords: &["passport", "flight", "hotel"],
    },
    CompoundPrompt {
        label: "search + create + reminder",
        prompt: "Find my notes on the vendor contract, create a task to review the payment \
                  terms, and remind me to send feedback by Wednesday.",
        keywords: &["vendor", "payment", "wednesday"],
    },
    // -- widened set: mutating + read-only combinations --
    CompoundPrompt {
        label: "delete + create (mutating x2)",
        prompt: "Delete the cancelled Northwind order and add a task to notify the warehouse.",
        keywords: &["northwind", "warehouse"],
    },
    CompoundPrompt {
        label: "search + search (read-only x2)",
        prompt: "Find my notes about the marketing budget and also find any tasks tagged \
                  'urgent'.",
        keywords: &["marketing", "urgent"],
    },
];

/// A single-intent control for each 2-intent compound prompt above (one
/// control per intent), same domain and phrasing style, so a comparison can
/// tell "this model routes badly in general" apart from "this model
/// specifically mishandles compounding".
///
/// The 3-intent prompts (`"three tasks"`, `"search + create + reminder"`)
/// deliberately have no controls here: their purpose is to test whether a
/// *third* intent changes Stage 1's structural behaviour (does `route_clarify`
/// ever fire with more intents in play, does the model drop more than one),
/// not to re-establish specificity loss — the 2-intent controls already
/// isolate that comparison, and per-intent controls for every 3-intent prompt
/// would triple this list for a question the existing controls already answer.
const SINGLE_INTENT_CONTROLS: &[(&str, &str)] = &[
    // -- expense + reminder --
    ("expense only", "Log a $42 lunch expense."),
    (
        "reminder only",
        "Remind me to follow up with Sarah on Friday.",
    ),
    // -- search + create --
    ("budget search only", "Find my notes about the Q3 budget."),
    (
        "review task only",
        "Create a task to review the Q3 budget notes tomorrow.",
    ),
    // -- two tasks --
    ("plumber task only", "Add a task to call the plumber."),
    ("electric task only", "Add a task to pay the electric bill."),
    // -- note + task --
    (
        "meeting note only",
        "Write down that the client meeting moved to 3pm.",
    ),
    (
        "calendar task only",
        "Add a task to update the calendar invite.",
    ),
    // -- delete + search --
    (
        "invoice delete only",
        "Delete the draft invoice for Acme Corp.",
    ),
    (
        "invoice search only",
        "Find all other invoices from last month.",
    ),
    // -- create + create (varied phrasing) --
    (
        "passport task only",
        "I need a new task for renewing the passport.",
    ),
    (
        "dentist note only",
        "Jot down that the dentist moved my appointment to next Tuesday.",
    ),
    // -- search + delete (varied phrasing) --
    (
        "onboarding search only",
        "Can you pull up everything tagged 'onboarding'?",
    ),
    (
        "proposal delete only",
        "Get rid of the old draft proposal for Meridian.",
    ),
    // -- delete + create (mutating x2) --
    (
        "northwind delete only",
        "Delete the cancelled Northwind order.",
    ),
    ("warehouse task only", "Add a task to notify the warehouse."),
    // -- search + search (read-only x2) --
    (
        "marketing search only",
        "Find my notes about the marketing budget.",
    ),
    ("urgent search only", "Find any tasks tagged 'urgent'."),
];

/// What Stage 1 actually did on one prompt.
#[derive(Debug, Clone)]
enum Stage1Outcome {
    /// A routing tool was called and parsed cleanly.
    Decided(RouteDecision),
    /// A routing tool was called but `parse_route_decision` rejected it
    /// (blank query, blank question, or malformed JSON).
    CalledButUnparseable { tool: String, args: String },
    /// The model called neither Stage-1 tool.
    NoToolCall,
    /// More than one tool call in a single Stage-1 turn — itself notable,
    /// since `stage1_system_prompt` says "call exactly one tool".
    MultipleToolCalls(Vec<(String, String)>),
    /// The generation itself failed. Not a finding about routing.
    Errored(String),
}

/// Fires one Stage-1 turn on `user_message` alone.
///
/// Production's real call site (`agent_loop.rs`'s `stage1_query`) blends
/// prior conversation turns into this message before routing. There is no
/// session here — each prompt below is a fresh, single-turn request — so that
/// blending has nothing to do and is skipped rather than reimplemented; this
/// is a deliberate simplification for an isolated probe, not an oversight.
async fn run_stage1(engine: &Arc<dyn ChatInferenceEngine>, user_message: &str) -> Stage1Outcome {
    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, stage1_system_prompt(&seeded_skill_names())),
            ChatMessage::text(Role::User, user_message.to_string()),
        ],
        tools: Some(routing::stage1_tool_definitions()),
        temperature: Some(0.1),
        max_tokens: Some(STAGE1_MAX_TOKENS),
    };

    // Collect (id, name, args_json) per tool call, preserving call order.
    let calls: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);

    let generated = engine
        .generate(
            request,
            Box::new(move |chunk| {
                let mut g = sink.lock().expect("sink not poisoned");
                match chunk {
                    StreamingChunk::ToolCallStart { id, name, .. } => {
                        g.push((id, name, String::new()));
                    }
                    StreamingChunk::ToolCallArgs { id, args_json } => {
                        if let Some(entry) = g.iter_mut().find(|(cid, _, _)| *cid == id) {
                            entry.2.push_str(&args_json);
                        }
                    }
                    _ => {}
                }
            }),
        )
        .await;

    if let Err(e) = generated {
        return Stage1Outcome::Errored(e.to_string());
    }

    let observed = calls.lock().expect("sink not poisoned").clone();
    match observed.len() {
        0 => Stage1Outcome::NoToolCall,
        1 => {
            let (_, name, args) = &observed[0];
            match routing::parse_route_decision(name, args) {
                Some(decision) => Stage1Outcome::Decided(decision),
                None => Stage1Outcome::CalledButUnparseable {
                    tool: name.clone(),
                    args: args.clone(),
                },
            }
        }
        _ => {
            Stage1Outcome::MultipleToolCalls(observed.into_iter().map(|(_, n, a)| (n, a)).collect())
        }
    }
}

fn display(outcome: &Stage1Outcome) -> String {
    match outcome {
        Stage1Outcome::Decided(RouteDecision::Query(q)) => {
            format!("route_query(\"{q}\")")
        }
        Stage1Outcome::Decided(RouteDecision::Multi(qs)) => {
            format!("route_multi({qs:?})")
        }
        Stage1Outcome::Decided(RouteDecision::Clarify { question, options }) => {
            format!("route_clarify(\"{question}\", options={options:?})")
        }
        Stage1Outcome::CalledButUnparseable { tool, args } => {
            format!("UNPARSEABLE {tool}({args})")
        }
        Stage1Outcome::NoToolCall => "NO TOOL CALL".to_string(),
        Stage1Outcome::MultipleToolCalls(calls) => {
            format!("MULTIPLE CALLS {calls:?}")
        }
        Stage1Outcome::Errored(e) => format!("ERROR {e}"),
    }
}

/// Whether a query decision mentions every intent's keyword (crude substring
/// check on hand-picked keywords per prompt) rather than silently dropping
/// one.
///
/// This is a heuristic, not a verdict: a query can legitimately paraphrase.
/// It exists to flag the intent-silently-dropped failure mode for human
/// inspection in the printed trace, and to tally a degradation rate across
/// the run — not to assert pass/fail on any single prompt. That judgement
/// belongs to a human reading the raw traces, per the issue's own acceptance
/// criteria; the tally exists so that reading has a rate to work from instead
/// of only anecdotes.
fn mentions_all(query: &str, keywords: &[&str]) -> bool {
    let lower = query.to_ascii_lowercase();
    keywords
        .iter()
        .all(|kw| lower.contains(&kw.to_ascii_lowercase()))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a local inference backend; loads the locked native GGUF and runs many live generations"]
async fn stage1_on_compound_unambiguous_requests() {
    let Some(engine) = resolve_locked_model().await else {
        eprintln!(
            "locked native model ({LOCKED_NATIVE_MODEL}) not downloaded — \
             skipping compound-intent probe"
        );
        return;
    };

    println!("\nStage-1 compound-intent probe — {LOCKED_NATIVE_MODEL} (native, in-process)\n");

    let mut report = String::new();
    report.push_str(&format!("== {LOCKED_NATIVE_MODEL} (native) ==\n"));

    println!("-- single-intent controls --");
    for (label, prompt) in SINGLE_INTENT_CONTROLS {
        let outcome = run_stage1(&engine, prompt).await;
        let line = format!("  [{label:<24}] {prompt}\n      -> {}", display(&outcome));
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }

    println!("-- compound, unambiguous prompts --");
    // Tally rather than eyeball: a rate is what the issue's follow-up
    // criterion asks for, and it can't be read off individual printed lines
    // without a human doing the counting by hand.
    let mut clarify_calls = 0usize;
    let mut merged_and_faithful = 0usize;
    let mut merged_and_lossy = 0usize;
    let mut multi_and_faithful = 0usize;
    let mut multi_and_lossy = 0usize;
    let mut other_outcomes = 0usize;

    for cp in COMPOUND_PROMPTS {
        let outcome = run_stage1(&engine, cp.prompt).await;
        let mut line = format!(
            "  [{:<34}] {}\n      -> {}",
            cp.label,
            cp.prompt,
            display(&outcome)
        );
        match &outcome {
            Stage1Outcome::Decided(RouteDecision::Query(q)) => {
                let faithful = mentions_all(q, cp.keywords);
                line.push_str(&format!(
                    "\n      (mentions all {} intent keyword(s): {faithful})",
                    cp.keywords.len()
                ));
                if faithful {
                    merged_and_faithful += 1;
                } else {
                    merged_and_lossy += 1;
                }
            }
            Stage1Outcome::Decided(RouteDecision::Multi(qs)) => {
                // Faithful here means every intent keyword shows up SOMEWHERE
                // across the per-intent queries combined — route_multi is
                // free to split them across elements, unlike route_query
                // which must fit everything into one string.
                let combined = qs.join(" ");
                let faithful = mentions_all(&combined, cp.keywords);
                line.push_str(&format!(
                    "\n      (mentions all {} intent keyword(s) across {} queries: {faithful})",
                    cp.keywords.len(),
                    qs.len()
                ));
                if faithful {
                    multi_and_faithful += 1;
                } else {
                    multi_and_lossy += 1;
                }
            }
            Stage1Outcome::Decided(RouteDecision::Clarify { .. }) => clarify_calls += 1,
            _ => other_outcomes += 1,
        }
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }

    let total = COMPOUND_PROMPTS.len();
    let summary = format!(
        "\n-- summary (n={total}) --\n\
         route_clarify called: {clarify_calls}/{total}\n\
         route_query, faithful (mentions all intent keywords): {merged_and_faithful}/{total}\n\
         route_query, lossy (dropped or fabricated a detail by this heuristic): {merged_and_lossy}/{total}\n\
         route_multi, faithful (mentions all intent keywords across queries): {multi_and_faithful}/{total}\n\
         route_multi, lossy: {multi_and_lossy}/{total}\n\
         other (no call / unparseable / multiple calls / error): {other_outcomes}/{total}\n\
         \n\
         Single-run rate. Per #1862's standing lesson, treat this as directional, not decision-grade,\n\
         without at least one repeated run to compare against.\n"
    );
    println!("{summary}");
    report.push_str(&summary);

    match std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("stage1-compound-intent-latest.txt"),
        &report,
    ) {
        Ok(()) => println!("raw trace written to tests/stage1-compound-intent-latest.txt"),
        Err(e) => println!("could not write trace artifact: {e}"),
    }

    // No assertions on model behaviour: per the issue, this test exists to
    // produce raw traces (and now a same-run tally) for a human decision, not
    // to encode a pass/fail bar for a question not yet fully answered. The
    // only hard failure is a broken run.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ROUTE_QUERY_TOOL`/`ROUTE_CLARIFY_TOOL` are re-exported so this file's
    /// dependency on the wire names cannot silently drift from `routing.rs`.
    #[test]
    fn tool_name_constants_match_stage1_tool_definitions() {
        let defs = routing::stage1_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![ROUTE_QUERY_TOOL, ROUTE_CLARIFY_TOOL, ROUTE_MULTI_TOOL]
        );
    }

    /// 2-intent compound prompts get exactly 2 controls each (one per
    /// intent); 3-intent prompts deliberately get none — see the doc comment
    /// on `SINGLE_INTENT_CONTROLS`. This guards that ratio mechanically
    /// rather than trusting whoever edits either list next to keep the count
    /// in sync by hand.
    #[test]
    fn two_intent_prompts_have_matching_control_counts() {
        let two_intent_prompts = COMPOUND_PROMPTS
            .iter()
            .filter(|cp| cp.keywords.len() == 2)
            .count();
        assert_eq!(
            SINGLE_INTENT_CONTROLS.len(),
            two_intent_prompts * 2,
            "expected 2 controls per 2-intent compound prompt"
        );
    }

    /// Every compound prompt carries at least one keyword — enforced by
    /// `CompoundPrompt`'s shape for the field itself, but not for it being
    /// non-empty, which would silently make `mentions_all` vacuously true.
    #[test]
    fn every_compound_prompt_has_at_least_one_keyword() {
        for cp in COMPOUND_PROMPTS {
            assert!(
                !cp.keywords.is_empty(),
                "compound prompt {:?} has no keywords; mentions_all would be vacuously true",
                cp.label
            );
        }
    }

    #[test]
    fn mentions_all_requires_every_keyword_present() {
        assert!(mentions_all(
            "log expense and set reminder",
            &["expense", "remind"]
        ));
        assert!(!mentions_all("log expense", &["expense", "remind"]));
        assert!(
            mentions_all("anything", &[]),
            "vacuous case, guarded against above"
        );
    }
}
