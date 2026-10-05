//! One-time routing-reliability probe for served (OpenAI-compatible) models.
//!
//! `tests/it/live_openai_compat_routing.rs` established, against a local Ollama,
//! that Stage-2 candidate injection is safe on most served models but not
//! all: it is a **per-model property**, not a native-vs-served split — the
//! locked native model and most measured served models route cleanly, while
//! one measured model (`mistral:7b`) loses tool-calling outright as soon as
//! any candidate block is present, independent of the block's content.
//!
//! ADR-038 names this as the load-bearing open question the routing design carried;
//! this probe is Option C from the decision that closed it: run one synthetic
//! routed turn per model load, cache the verdict, and skip Stage-2 injection
//! for a model that fails it — rather than paying a retry on every ambiguous
//! turn (Option B) or leaving every served model exposed until a user
//! notices (Option A).
//!
//! The probe is deliberately narrower than the live matrix: it runs exactly
//! the `routed_names` arm (a candidate block stripped to a bare skill name,
//! nothing else) because that arm is the strict subset of what `routed_full`
//! injects, and the matrix found the failure content-independent — a model
//! that survives the smaller block is not guaranteed to survive a larger one,
//! but a model that fails the smaller one will also fail the larger one, so
//! testing the minimal block is the conservative direction to probe in
//! without doubling the cost of every model load.
//!
//! It does, however, run the same Stage-1 pre-generation the matrix's routed
//! arms pay, matching the shape of request `agent_loop::run_turn` actually
//! sends — see [`probe_routing_ok`]'s doc comment.

use crate::agent_types::{ChatInferenceEngine, InferenceRequest, SkillCandidate, StreamingChunk};
use crate::local_agent::agent_loop::{stage1_system_prompt, stage1_type_names, STAGE1_MAX_TOKENS};
use crate::local_agent::routing;
use nodespace_nlp_engine::chat::types::{ChatMessage, Role};
use std::sync::{Arc, Mutex};

/// The request the probe sends. Phrased so a correct response is unambiguous
/// — mirrors `USER_MESSAGE` in the live matrix test, which this probe is a
/// production-path subset of.
const PROBE_USER_MESSAGE: &str = "Find my notes about the Q3 budget.";

/// A minimal system prompt standing in for the assembled one.
///
/// Deliberately short, for the same reason the live matrix test's
/// `SYSTEM_PROMPT` is: the production prompt would be a second uncontrolled
/// variable in what this probe isolates.
const PROBE_SYSTEM_PROMPT: &str =
    "You are NodeSpace's assistant. Use the available tools to fulfil the user's request.";

/// The one tool the probe offers, matching the live matrix test's `search_tool`.
fn probe_tool() -> crate::agent_types::ToolDefinition {
    crate::agent_types::ToolDefinition {
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

/// A candidate reduced to its name — the conservative arm; see module docs.
fn probe_candidate() -> SkillCandidate {
    SkillCandidate {
        id: "skill-research".to_string(),
        name: "Research".to_string(),
        description: String::new(),
        score: 0.9,
        tools: vec!["search_nodes".to_string()],
        instructions: String::new(),
        schema_metadata: serde_json::json!([]),
        schemas_linked: false,
        pinned: false,
    }
}

/// Whether Stage-2 candidate injection suppresses tool-calling on `engine`.
///
/// Sends one turn with a bare-name candidate block injected and reports
/// whether the model still calls `search_nodes`. `Ok(true)` means routing is
/// safe to enable; `Ok(false)` means the probe observed suppression.
///
/// Runs the real Stage-1 generation first, exactly as `agent_loop::run_turn`
/// always does before Stage 2. A production routed turn is never Stage-2-only
/// — Stage 1 always precedes it on the same request sequence — so a probe
/// that skipped Stage 1 would be measuring a shape of request the shipped
/// loop never actually sends, whether or not the two are causally linked on a
/// given transport. This mirrors the live matrix test's own routed arms,
/// which pay the same Stage-1 cost for the same reason (see that module's
/// docs on `Stage1Only`).
///
/// Errors (unreachable endpoint, malformed response) are surfaced rather than
/// folded into `Ok(false)` — an unmeasured model must never be cached as a
/// verdict, the same distinction `live_openai_compat_routing.rs`'s
/// `ArmResult::Errored` exists to preserve. The caller should treat an error
/// as "could not probe" and leave the model's cached verdict untouched (or
/// unset) rather than disabling routing on a guess. A Stage-1 failure here is
/// not such an error: the shipped loop warns and continues unrouted on a
/// failed Stage 1, so the probe mirrors that rather than aborting.
pub async fn probe_routing_ok(
    engine: &dyn ChatInferenceEngine,
) -> Result<bool, crate::agent_types::InferenceError> {
    let stage1 = InferenceRequest {
        messages: vec![
            // Names the probe's one candidate, so the request has the same
            // shape as production's: a registry is never empty there.
            ChatMessage::text(
                Role::System,
                stage1_system_prompt(
                    &[probe_candidate().name],
                    &stage1_type_names([], PROBE_USER_MESSAGE),
                ),
            ),
            ChatMessage::text(Role::User, PROBE_USER_MESSAGE.to_string()),
        ],
        tools: Some(routing::stage1_tool_definitions()),
        temperature: Some(0.1),
        max_tokens: Some(STAGE1_MAX_TOKENS),
    };
    // Not aborted on failure (see the doc comment above), but not silent
    // either: if Stage 1 is failing consistently, the verdict below is being
    // formed on a request sequence that diverges from what it means to
    // measure, and that should be diagnosable rather than invisible.
    if let Err(e) = engine.generate(stage1, Box::new(|_| {})).await {
        tracing::debug!(error = %e, "routing probe: Stage 1 pre-generation failed; continuing as production does");
    }

    let block = routing::render_candidates_for_prompt(&[probe_candidate()])
        .expect("the probe candidate's fixed score clears both score bars");
    let system_content = format!("{PROBE_SYSTEM_PROMPT}\n\n{block}");

    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, system_content),
            ChatMessage::text(Role::User, PROBE_USER_MESSAGE.to_string()),
        ],
        tools: Some(vec![probe_tool()]),
        temperature: Some(0.0),
        max_tokens: None,
    };

    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);

    engine
        .generate(
            request,
            Box::new(move |chunk| {
                if let StreamingChunk::ToolCallStart { name, .. } = chunk {
                    sink.lock().expect("sink not poisoned").push(name);
                }
            }),
        )
        .await?;

    let observed = calls.lock().expect("sink not poisoned").clone();
    Ok(observed.iter().any(|c| c == "search_nodes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::{InferenceError, InferenceUsage};
    use async_trait::async_trait;

    /// Engine stub that reports whichever tool calls the test configures,
    /// or errors, without a real model. Mirrors the shape of test doubles
    /// already used in `agent_loop.rs`'s own test module.
    struct StubEngine {
        result: Result<Vec<&'static str>, String>,
    }

    #[async_trait]
    impl ChatInferenceEngine for StubEngine {
        async fn generate(
            &self,
            _request: InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            match &self.result {
                Ok(names) => {
                    for name in names {
                        on_chunk(StreamingChunk::ToolCallStart {
                            id: "call-1".to_string(),
                            name: name.to_string(),
                            provider_extra: None,
                        });
                    }
                    Ok(InferenceUsage::default())
                }
                Err(e) => Err(InferenceError::Engine(e.clone())),
            }
        }

        async fn model_info(
            &self,
        ) -> Result<Option<crate::agent_types::ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, _text: &str) -> Result<u32, InferenceError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn a_model_that_fires_under_injection_probes_ok() {
        let engine = StubEngine {
            result: Ok(vec!["search_nodes"]),
        };
        assert!(probe_routing_ok(&engine).await.expect("probe succeeds"));
    }

    #[tokio::test]
    async fn a_model_that_produces_no_tool_call_probes_suppressed() {
        let engine = StubEngine { result: Ok(vec![]) };
        assert!(!probe_routing_ok(&engine).await.expect("probe succeeds"));
    }

    #[tokio::test]
    async fn a_model_that_calls_something_else_probes_suppressed() {
        // Not the tool the probe asked for — same "did not do the routed
        // thing" outcome as silence, for this probe's purposes.
        let engine = StubEngine {
            result: Ok(vec!["get_node"]),
        };
        assert!(!probe_routing_ok(&engine).await.expect("probe succeeds"));
    }

    #[tokio::test]
    async fn an_engine_error_is_not_a_suppression_verdict() {
        let engine = StubEngine {
            result: Err("connection refused".to_string()),
        };
        assert!(
            probe_routing_ok(&engine).await.is_err(),
            "an unreachable engine must not be reported as Ok(false) — that would cache a \
             guess as though it were a measurement"
        );
    }
}
