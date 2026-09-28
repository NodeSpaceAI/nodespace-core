//! Live check that `routing_probe::probe_routing_ok` agrees with the full
//! four-arm matrix (`live_openai_compat_routing.rs`) on a real server.
//!
//! The matrix is the source of truth for what "safe" and "suppressed" mean;
//! this test exists only to confirm the production probe function — the one
//! `local_agent_service.rs` actually calls on model load — reaches the same
//! verdict the matrix's `routed_names` arm did, against a live model rather
//! than the unit-level string comparison in
//! `live_openai_compat_routing.rs`'s `the_probes_block_matches_the_routed_names_arm`.
//!
//! Ignored by default — it needs a real server. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test it live_routing_probe:: -- --ignored --nocapture
//! ```
//!
//! **A degraded Ollama can fail both tests without either being wrong.**
//! Sustained heavy use (the four-arm matrix run repeatedly, back to back, for
//! an extended session) was observed to push a served model into returning
//! `{"created":-62135596800,"model":"","choices":[{"message":{"role":"",
//! "content":""}}]}` — HTTP 200, but a zeroed, garbage response — for *every*
//! request to that model, including a bare "say hello" with no tools and no
//! candidate block at all. That is a server/runtime fault, not a suppression
//! finding, and it will make this test fail (empty tool calls look identical
//! to a real suppression from the outside). Before treating a failure here as
//! a probe or production regression: `curl` the endpoint directly with a
//! trivial prompt for the model in question and check for exactly this
//! shape of degenerate response; if you see it, restart the server rather
//! than debugging this test's code.

use nodespace_agent::local_agent::openai_compat_inference::OpenAiCompatInferenceEngine;
use nodespace_agent::local_agent::routing_probe::probe_routing_ok;

const BASE_URL: &str = "http://127.0.0.1:11434/v1";

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server"]
async fn probe_agrees_with_the_matrix_on_a_clean_model() {
    let engine = OpenAiCompatInferenceEngine::new(
        BASE_URL.to_string(),
        String::new(),
        "llama3.1:8b".to_string(),
    );
    let ok = probe_routing_ok(&engine)
        .await
        .expect("probe should complete against a reachable server");
    assert!(
        ok,
        "llama3.1:8b measured clean on routed_names in the four-arm matrix; the probe must agree"
    );
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server"]
async fn probe_agrees_with_the_matrix_on_a_suppressed_model() {
    let engine = OpenAiCompatInferenceEngine::new(
        BASE_URL.to_string(),
        String::new(),
        "mistral:7b".to_string(),
    );
    let ok = probe_routing_ok(&engine)
        .await
        .expect("probe should complete against a reachable server");
    assert!(
        !ok,
        "mistral:7b measured SUPPRESSED under routed_names in the four-arm matrix; the probe \
         must agree, not report it as safe"
    );
}
