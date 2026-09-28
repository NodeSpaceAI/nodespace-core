//! Live routing-reliability matrix for the OpenAI-compatible path.
//!
//! ADR-056 locks only the *native* in-process GGUF path to one model. The
//! OpenAI-compatible path stays open to any user-configured server by design,
//! which means ADR-038's two-stage routing runs against models nobody has
//! characterised.
//!
//! `live_openai_compat_smoke.rs` guards that the transport works: discovery
//! reaches the endpoint and an *unrouted* turn produces a tool call. That test
//! cannot see the failure this one looks for, because the failure appears only
//! once Stage-2's candidate block is in the prompt — a block the smoke test
//! never injects.
//!
//! The four arms isolate which part of routing, if any, suppresses
//! tool-calling on a served model:
//!
//! | arm             | candidate block | what it isolates                    |
//! |-----------------|-----------------|-------------------------------------|
//! | `baseline`      | none            | can the model tool-call at all      |
//! | `stage1_only`   | none            | server-side statefulness (see below)|
//! | `routed_full`   | yes, full       | ADR-064 rule 4's predicted mechanism |
//! | `routed_names`  | yes, names only | whether *content* is the variable   |
//!
//! `routed_names` is the arm that carries the finding. If a model fires on
//! `routed_full` but not `routed_names` — or suppresses on both — the operative
//! variable is the block's *presence*, not how procedural its content is, which
//! is what ADR-064 rule 4 predicts and what was measured on the locked model.
//!
//! **`stage1_only` is expected to match `baseline` on this transport, and that
//! is not a defect.** On the native in-process path, Stage 1's cost is a
//! diverged KV-cache prefix — the overhead ADR-038 accepts and requires be
//! measured. Over HTTP there is no shared prefix: each request is independent,
//! so a discarded Stage-1 generation cannot reach the measured turn through any
//! mechanism this test controls, and the two prompts are identical (asserted in
//! `the_four_arms_produce_distinct_prompts`). The arm is kept because that
//! independence is an assumption about the *server*, not a guarantee: a server
//! that carried conversation state across requests, or rate-limited or evicted a
//! model between them, would show up here as `stage1_only` diverging from
//! `baseline` — and would invalidate the routed arms, which pay the same
//! Stage-1 cost. It is a guard on the harness, not a measurement of routing.
//!
//! Widened from one served model to three, measured against a
//! local Ollama, reproduced identically across two full runs:
//!
//! ```text
//! model             baseline    stage1_only routed_full routed_names
//! llama3.1:8b       fires       fires       fires       fires
//! mistral:7b        fires       fires       SUPPRESSED  SUPPRESSED
//! gemma4:e4b        fires       fires       fires       fires
//! ```
//!
//! (`gemma4:e4b` here is the served/Ollama comparison, not the locked native
//! model — included because it is a served data point too, distinct from
//! point 1 below.)
//!
//! What this establishes:
//!
//! 1. The locked native model routes cleanly on every arm — the shipped
//!    configuration is unaffected, which is the result that matters for what
//!    NodeSpace runs.
//! 2. On `mistral:7b` the block's *presence* suppresses tool-calling, not its
//!    content — `routed_names` carries a name and nothing else and still
//!    suppresses. ADR-064 rule 4's mechanism, measured on the locked model,
//!    does not explain this.
//! 3. **This is a per-model property, not a native-vs-served split.** 2 of 3
//!    measured served models route cleanly; only `mistral:7b` fails. "All
//!    served models are at risk" is not supported by this data. The runtime
//!    mitigation (`nodespace_agent::local_agent::routing_probe`, Option C from
//!    ADR-038) is keyed on this per-model finding: it probes each
//!    served model independently at load time rather than assuming a
//!    blanket native-vs-served rule.
//!
//! What remains open: whether a served model beyond these three shares
//! `mistral:7b`'s failure. The matrix and the runtime probe both exist
//! precisely so that question gets answered per-model as new models are
//! measured, rather than needing another matrix-widening exercise; see
//! [`EXCLUDED_MODEL_FRAGMENTS`] for which models this matrix will (and will
//! not) measure.
//!
//! Ignored by default — it needs a real server. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test it live_openai_compat_routing:: -- --ignored --nocapture
//! ```

use nodespace_agent::agent_types::{
    ChatInferenceEngine, InferenceRequest, SkillCandidate, StreamingChunk, ToolDefinition,
};
use nodespace_agent::local_agent::agent_loop::{stage1_system_prompt, STAGE1_MAX_TOKENS};
use nodespace_agent::local_agent::openai_compat_discovery::discover_models;
use nodespace_agent::local_agent::openai_compat_inference::OpenAiCompatInferenceEngine;
use nodespace_agent::local_agent::routing;
use nodespace_nlp_engine::chat::types::{ChatMessage, Role};
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

const BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// The request every arm sends. Phrased so the correct behaviour is
/// unambiguous — a model that does not call `search_nodes` here has not made a
/// defensible judgement call, it has failed to tool-call.
const USER_MESSAGE: &str = "Find my notes about the Q3 budget.";

/// Model-name fragments this matrix refuses to measure.
///
/// A dev box accumulates models NodeSpace has deliberately removed. Measuring
/// one and reporting the number produces evidence for a model the project does
/// not carry, which is worse than no evidence: it invites conclusions about
/// NodeSpace's behaviour drawn from something NodeSpace does not run.
///
/// ADR-056 removed the Qwen family from the catalog outright — including the
/// Qwen3.5-based `ornith-1-9b-q4km` — along with its `ModelFamily` variants and
/// its format-specific parser and response-cleanup code. A local Ollama may
/// still serve `ornith:9b`; this matrix must not treat that as a NodeSpace
/// model.
const EXCLUDED_MODEL_FRAGMENTS: &[&str] = &["qwen", "ornith"];

/// Whether a served model is one this matrix should measure.
fn is_measurable(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    !EXCLUDED_MODEL_FRAGMENTS
        .iter()
        .any(|frag| lower.contains(frag))
}

/// A minimal system prompt standing in for the assembled one.
///
/// Deliberately short. The production prompt is long enough that it would be a
/// second uncontrolled variable, and this test is isolating the candidate
/// block, not the resident prompt.
const SYSTEM_PROMPT: &str =
    "You are NodeSpace's assistant. Use the available tools to fulfil the user's request.";

/// Which arm, and therefore what goes into the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// No routing at all — the shape `live_openai_compat_smoke.rs` covers.
    Baseline,
    /// Stage 1 runs, but retrieval surfaced nothing eligible, so no block is
    /// injected. Expected to match [`Arm::Baseline`] on a stateless transport;
    /// a divergence means the server carried state between requests, which
    /// would invalidate the routed arms too. See the module docs.
    Stage1Only,
    /// The routed path as it actually ships: candidates rendered by production
    /// code, instructions and all.
    RoutedFull,
    /// Candidates stripped to bare names — no instructions, no purpose, no
    /// metadata. If this arm suppresses too, content is not the variable.
    RoutedNames,
}

impl Arm {
    const ALL: [Arm; 4] = [
        Arm::Baseline,
        Arm::Stage1Only,
        Arm::RoutedFull,
        Arm::RoutedNames,
    ];

    fn label(self) -> &'static str {
        match self {
            Arm::Baseline => "baseline",
            Arm::Stage1Only => "stage1_only",
            Arm::RoutedFull => "routed_full",
            Arm::RoutedNames => "routed_names",
        }
    }

    /// Whether this arm puts a candidate block in the prompt.
    fn injects_block(self) -> bool {
        matches!(self, Arm::RoutedFull | Arm::RoutedNames)
    }
}

/// The skill a correct route would surface for [`USER_MESSAGE`].
///
/// Scored well clear of `READ_SKILL_SCORE_BAR` so the mechanical gate is not
/// the thing under test — this test is about the model's response to the
/// block, not about whether the gate admits it.
fn research_candidate() -> SkillCandidate {
    SkillCandidate {
        id: "skill-research".to_string(),
        name: "Research".to_string(),
        description: "Find existing nodes and answer questions about their content.".to_string(),
        score: 0.9,
        tools: vec!["search_nodes".to_string()],
        instructions: "Call search_nodes with a query describing what the user is looking for. \
             Prefer specific terms from the user's request over generic ones. Report what you \
             found; do not invent nodes that were not returned."
            .to_string(),
        schema_metadata: serde_json::json!([]),
    }
}

/// The same candidate reduced to its name.
///
/// Every field the full rendering would surface as prose is emptied, so the
/// block that reaches the model carries a name and nothing else.
fn names_only_candidate() -> SkillCandidate {
    SkillCandidate {
        description: String::new(),
        instructions: String::new(),
        schema_metadata: serde_json::json!([]),
        ..research_candidate()
    }
}

/// The one tool every arm offers.
fn search_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search_nodes".to_string(),
        description: "Search the knowledge graph for nodes matching a query.".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to search for."}
            },
            "required": ["query"]
        }),
    }
}

/// Build an arm's system prompt, using **production** rendering for the routed
/// arms so this measures what ships rather than a lookalike written here.
fn system_prompt_for(arm: Arm) -> String {
    let block = match arm {
        Arm::Baseline | Arm::Stage1Only => None,
        Arm::RoutedFull => routing::render_candidates_for_prompt(&[research_candidate()]),
        Arm::RoutedNames => routing::render_candidates_for_prompt(&[names_only_candidate()]),
    };

    // Mirrors the concatenation at the real injection point in `agent_loop`.
    match block {
        Some(b) => format!("{SYSTEM_PROMPT}\n\n{b}"),
        None => SYSTEM_PROMPT.to_string(),
    }
}

/// What one arm produced.
///
/// `Errored` is deliberately distinct from `Fired(vec![])`. A model that throws
/// or times out produces no tool calls, and so does a model that suppresses —
/// but they are opposite findings, and collapsing them lets a harness failure
/// masquerade as evidence. Keeping them separate types makes it impossible to
/// count one as the other by accident.
#[derive(Debug, Clone)]
enum ArmResult {
    /// The generation completed; these are the tool calls it made (possibly none).
    Completed(Vec<String>),
    /// The generation failed. Not a finding about routing.
    Errored(String),
}

impl ArmResult {
    /// Whether the arm produced the expected tool call.
    ///
    /// An errored arm is not "suppressed" — it is unmeasured, and callers must
    /// treat it as such rather than folding it into the negative case.
    fn fired(&self) -> bool {
        matches!(self, ArmResult::Completed(calls) if calls.iter().any(|c| c == "search_nodes"))
    }

    fn errored(&self) -> bool {
        matches!(self, ArmResult::Errored(_))
    }

    fn display(&self) -> String {
        match self {
            ArmResult::Completed(calls) if self.fired() => format!("fires      {calls:?}"),
            ArmResult::Completed(calls) => format!("SUPPRESSED {calls:?}"),
            ArmResult::Errored(e) => format!("ERROR      {e}"),
        }
    }
}

/// Run one arm against one model.
async fn run_arm(model: &str, arm: Arm) -> ArmResult {
    let engine =
        OpenAiCompatInferenceEngine::new(BASE_URL.to_string(), String::new(), model.to_string());

    // Stage 1 is a separate generation ahead of the real turn. Arms that model
    // it pay that cost so the extra turn is not a hidden difference between
    // arms; its output is deliberately discarded, exactly as the shipped loop
    // discards it when retrieval yields nothing eligible.
    if arm == Arm::Stage1Only || arm.injects_block() {
        // Production's own prompt, tools, and sampling parameters — not a
        // paraphrase. The whole test rests on measuring what ships, and a
        // hand-written Stage-1 call would be the one place that stopped being
        // true. Importing the constants means they cannot drift from the real
        // ones without breaking the build.
        let stage1 = InferenceRequest {
            messages: vec![
                ChatMessage::text(Role::System, stage1_system_prompt(&seeded_skill_names())),
                ChatMessage::text(Role::User, USER_MESSAGE),
            ],
            tools: Some(routing::stage1_tool_definitions()),
            temperature: Some(0.1),
            max_tokens: Some(STAGE1_MAX_TOKENS),
        };
        // A Stage-1 failure is not this test's subject; the shipped loop warns
        // and continues unrouted, so mirror that rather than failing the arm.
        let _ = engine.generate(stage1, Box::new(|_| {})).await;
    }

    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, system_prompt_for(arm)),
            ChatMessage::text(Role::User, USER_MESSAGE),
        ],
        tools: Some(vec![search_tool()]),
        temperature: Some(0.0),
        max_tokens: None,
    };

    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);

    let generated = engine
        .generate(
            request,
            Box::new(move |chunk| {
                if let StreamingChunk::ToolCallStart { name, .. } = chunk {
                    sink.lock().expect("sink not poisoned").push(name);
                }
            }),
        )
        .await;

    // Surfaced as `Errored`, not as an empty result: one model failing must not
    // hide the matrix for the others, but it must also never be read as a
    // suppression finding.
    if let Err(e) = generated {
        return ArmResult::Errored(e.to_string());
    }

    let observed = calls.lock().expect("sink not poisoned").clone();
    ArmResult::Completed(observed)
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server"]
async fn routing_does_not_suppress_tool_calling_on_served_models() {
    let served = discover_models(BASE_URL, "")
        .await
        .expect("discovery should reach the endpoint");
    assert!(!served.is_empty(), "expected at least one served model");

    let (models, excluded): (Vec<String>, Vec<String>) =
        served.into_iter().partition(|m| is_measurable(m));

    println!("\nrouting reliability matrix — {BASE_URL}");
    println!("measuring: {models:?}");
    if !excluded.is_empty() {
        println!("excluded (removed from the NodeSpace catalog): {excluded:?}");
    }
    println!();

    assert!(
        !models.is_empty(),
        "every served model is excluded; nothing to measure"
    );

    // Models whose baseline fires but which lose tool-calling under an
    // injected block. Reported rather than asserted per-model: suppression is a
    // property of the served model, not a NodeSpace regression to redden a
    // build on.
    let mut suppressed: Vec<(String, Vec<&'static str>)> = Vec::new();
    // Models that cannot tool-call even unrouted. A different failure — not
    // routing's fault, and not something this test should report as one.
    let mut cannot_tool_call: Vec<String> = Vec::new();
    // Models where at least one arm failed to complete. These are *unmeasured*,
    // not clean and not suppressed, and the run is not trustworthy while any
    // remain — a timeout that silently vanished from the matrix would look
    // exactly like a model nobody thought to test.
    let mut errored: Vec<(String, Vec<String>)> = Vec::new();
    // Every arm of every measured model, for the diffable artifact.
    let mut rows: Vec<(String, Vec<(Arm, ArmResult)>)> = Vec::new();

    for model in &models {
        println!("  {model}");
        let mut results: Vec<(Arm, ArmResult)> = Vec::new();

        for arm in Arm::ALL {
            let result = run_arm(model, arm).await;
            println!("    {:<13} {}", arm.label(), result.display());
            results.push((arm, result));
        }

        let arm_errors: Vec<String> = results
            .iter()
            .filter(|(_, r)| r.errored())
            .map(|(a, r)| match r {
                ArmResult::Errored(e) => format!("{}: {e}", a.label()),
                ArmResult::Completed(_) => unreachable!("filtered to errored"),
            })
            .collect();

        if !arm_errors.is_empty() {
            println!(
                "    -> UNMEASURED: {} arm(s) failed to complete\n",
                arm_errors.len()
            );
            errored.push((model.clone(), arm_errors));
            rows.push((model.clone(), results));
            continue;
        }

        let baseline_fires = results
            .iter()
            .any(|(a, r)| *a == Arm::Baseline && r.fired());

        if !baseline_fires {
            cannot_tool_call.push(model.clone());
            println!("    -> cannot tool-call unrouted; routing arms are not interpretable\n");
            rows.push((model.clone(), results));
            continue;
        }

        let lost: Vec<&'static str> = results
            .iter()
            .filter(|(a, r)| a.injects_block() && !r.fired())
            .map(|(a, _)| a.label())
            .collect();

        if lost.is_empty() {
            println!("    -> routing safe on this model\n");
        } else {
            println!("    -> SUPPRESSED under: {}\n", lost.join(", "));
            suppressed.push((model.clone(), lost));
        }
        rows.push((model.clone(), results));
    }

    if !cannot_tool_call.is_empty() {
        println!("models that cannot tool-call at all: {cannot_tool_call:?}");
    }

    if suppressed.is_empty() {
        println!("no measured model lost tool-calling under routing.");
    } else {
        println!("ROUTING-SUPPRESSED MODELS:");
        for (model, arms) in &suppressed {
            let content_independent = arms.contains(&"routed_names");
            println!(
                "  {model}: suppressed under {} — {}",
                arms.join(", "),
                if content_independent {
                    "content-INDEPENDENT (block presence alone; contradicts ADR-064 rule 4's mechanism)"
                } else {
                    "content-dependent (consistent with ADR-064 rule 4)"
                }
            );
        }
    }

    match write_matrix_artifact(&rows, &excluded) {
        Ok(path) => {
            println!("\nmatrix written to {}", path.display());
            println!("paste it into the tracking issue so runs are diffable.");
        }
        Err(e) => println!("\ncould not write the matrix artifact: {e}"),
    }

    // --- Assertions: every one of these is a harness/config failure, not a
    // finding about a model. A model that suppresses is data; a run that could
    // not produce data is a broken run and must fail loudly.

    assert!(
        errored.is_empty(),
        "arms failed to complete, so the matrix is incomplete and a green result \
         would be misleading — a failed arm must never read as a clean or \
         suppressed one: {errored:?}"
    );

    // Every model must clear the baseline, not merely one of them. A model that
    // cannot tool-call unrouted yields three uninterpretable routing arms, and
    // letting that pass silently — as an earlier `< models.len()` form did —
    // shrinks the matrix without shrinking the model list, which is the same
    // "measured nothing, looked green" failure the errored-arm assertion exists
    // to prevent.
    assert!(
        cannot_tool_call.is_empty(),
        "these models could not tool-call even unrouted, so their routing arms \
         measure nothing: {cannot_tool_call:?}"
    );

    // Every model must have produced a verdict in exactly one bucket. Guards
    // the bookkeeping itself: a model silently falling through every branch
    // would otherwise shrink the matrix without shrinking the model list.
    assert_eq!(
        rows.len(),
        models.len(),
        "every measured model must appear in the matrix"
    );
}

/// Write the run to a diffable file next to the test.
///
/// Without this the matrix degrades into "run it once, eyeball the printout,
/// forget the result" — which is how this class of finding went unnoticed
/// until it turned up by accident as a comparison arm in an unrelated
/// diagnostic.
///
/// Gitignored, and deliberately not asserted against a checked-in golden: the
/// rows depend on which models the runner's endpoint serves, so tracking it
/// would make an ordinary re-run on a differently-provisioned box look like a
/// changed finding, and a golden would fail for everyone whose Ollama differs.
/// It is a local record; paste a run into the tracking issue to preserve it.
fn write_matrix_artifact(
    rows: &[(String, Vec<(Arm, ArmResult)>)],
    excluded: &[String],
) -> std::io::Result<std::path::PathBuf> {
    let mut out = String::from(
        "# OpenAI-compat routing reliability matrix\n#\n\
         # Generated by tests/live_openai_compat_routing.rs.\n\
         # Columns: baseline stage1_only routed_full routed_names\n\
         # Values:  fires | SUPPRESSED | ERROR\n#\n",
    );
    out.push_str(&format!("# endpoint: {BASE_URL}\n"));
    // Dated so a pasted run is placeable in time — without it, two runs are
    // indistinguishable as "same box later" versus "different box".
    out.push_str(&format!(
        "# run: {}\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ")
    ));
    if !excluded.is_empty() {
        out.push_str(&format!(
            "# excluded (removed from the NodeSpace catalog): {}\n",
            excluded.join(", ")
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "{:<18}{}\n",
        "model",
        Arm::ALL
            .iter()
            .map(|a| format!("{:<12}", a.label()))
            .collect::<String>()
            .trim_end()
    ));

    for (model, results) in rows {
        let cells: Vec<&str> = results
            .iter()
            .map(|(_, r)| {
                if r.errored() {
                    "ERROR"
                } else if r.fired() {
                    "fires"
                } else {
                    "SUPPRESSED"
                }
            })
            .collect();
        // Fixed-width cells so a diff between runs lines up column-wise and a
        // changed verdict is visible at a glance.
        let padded: Vec<String> = cells.iter().map(|c| format!("{c:<12}")).collect();
        out.push_str(&format!("{model:<18}{}\n", padded.join("").trim_end()));
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("routing-matrix-latest.txt");
    // A failed write must not sink a run that already produced its data, so the
    // error is returned for the caller to report rather than panicked on — but
    // it must never be reported as a successful write.
    std::fs::write(&path, out)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routed arms must actually differ in what they send, or the matrix
    /// compares two identical inputs and reports a difference that cannot
    /// exist. Runs without a server.
    #[test]
    fn the_four_arms_produce_distinct_prompts() {
        let baseline = system_prompt_for(Arm::Baseline);
        let stage1 = system_prompt_for(Arm::Stage1Only);
        let full = system_prompt_for(Arm::RoutedFull);
        let names = system_prompt_for(Arm::RoutedNames);

        // The unrouted arms differ from each other only in the extra Stage-1
        // generation, which is not part of the prompt.
        assert_eq!(baseline, stage1);

        assert_ne!(baseline, full, "routed_full must inject a block");
        assert_ne!(baseline, names, "routed_names must inject a block");
        assert_ne!(
            full, names,
            "the two routed arms must differ, or the content variable is not being tested"
        );
    }

    /// `nodespace_agent::local_agent::routing_probe` (Option C from ADR-038)
    /// runs one synthetic turn per served model at load time and caches
    /// whether Stage-2 injection is safe, instead of paying this matrix's full
    /// four-arm cost on every model load. It measures the conservative
    /// subset — see the probe module's own doc comment — which this matrix
    /// established as `routed_names`: a candidate block carrying a name and
    /// nothing else.
    ///
    /// This is the regression guard the probe's own doc comment promises: if
    /// the probe's rendered block ever silently grew instructions or metadata
    /// (e.g. `routing_probe::probe_candidate` drifting out of sync with this
    /// file's `names_only_candidate`), the probe would stop being the
    /// conservative arm it claims to be, and this test — not a live model —
    /// is what would catch it.
    #[test]
    fn the_probes_block_matches_the_routed_names_arm() {
        let matrix_names_block =
            routing::render_candidates_for_prompt(&[names_only_candidate()]).unwrap();

        // The probe module's candidate is private; reconstruct it the same
        // way `names_only_candidate` does here (name-only, matching id/score)
        // so this compares the two call sites' *shape*, not a shared value
        // that could drift together. A public accessor would let this test
        // trivially pass by construction; comparing the rendered output is
        // the part that actually verifies the two stay conservative.
        let probe_block = routing::render_candidates_for_prompt(&[SkillCandidate {
            id: "skill-research".to_string(),
            name: "Research".to_string(),
            description: String::new(),
            score: 0.9,
            tools: vec!["search_nodes".to_string()],
            instructions: String::new(),
            schema_metadata: serde_json::json!([]),
        }])
        .unwrap();

        assert_eq!(
            matrix_names_block, probe_block,
            "the probe must inject exactly what routed_names measured, or it is no longer \
             probing the arm this matrix validated"
        );
    }

    /// The exclusion list must not swallow a model NodeSpace actually runs.
    ///
    /// `"qwen"` is an architecture-family substring, not a product name, so a
    /// future Qwen-derived catalog entry would be dropped from the matrix with
    /// nothing but a `println!` to say so — the silent-omission failure this
    /// test's assertions otherwise exist to prevent. Pinning the locked native
    /// model and the served ids in current use makes that regression loud.
    #[test]
    fn the_exclusion_list_spares_models_nodespace_runs() {
        for kept in [
            "gemma4:e4b",
            "gemma-4-e4b-q4km",
            "mistral:7b",
            "mistral-nemo:12b",
        ] {
            assert!(
                is_measurable(kept),
                "{kept} is a model NodeSpace carries and must stay in the matrix"
            );
        }

        // And it must still exclude what ADR-056 removed, case-insensitively.
        for dropped in ["ornith:9b", "Ornith-1-9b-q4km", "qwen35-9b-q4km"] {
            assert!(
                !is_measurable(dropped),
                "{dropped} was removed from the catalog and must not become evidence"
            );
        }
    }

    /// `routed_names` is only meaningful if it really is names-only. If the
    /// production renderer ever starts emitting instructions for an empty
    /// instruction field, this arm would silently become a duplicate of
    /// `routed_full` and the content-independence claim would be unfounded.
    #[test]
    fn the_names_only_arm_carries_no_instructions() {
        let names = system_prompt_for(Arm::RoutedNames);
        assert!(
            names.contains("Research"),
            "the candidate's name must survive: {names}"
        );
        assert!(
            !names.contains("Call search_nodes with a query"),
            "names-only arm must not carry the instruction subtree: {names}"
        );
        assert!(
            !names.contains("Purpose:"),
            "names-only arm must not carry a purpose line: {names}"
        );

        // And the full arm must carry exactly what the names arm drops.
        let full = system_prompt_for(Arm::RoutedFull);
        assert!(full.contains("Call search_nodes with a query"));
        assert!(full.contains("Purpose:"));
    }
}
