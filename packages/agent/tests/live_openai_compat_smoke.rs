//! Live smoke test against a locally-running OpenAI-compatible server.
//!
//! Guards the claim this whole path rests on: that Ollama's `/v1` endpoint
//! serves both model discovery and tool-calling inference, so the native
//! Ollama client is genuinely redundant rather than merely similar.
//!
//! Scope: this covers the transport and an **unrouted** tool-calling turn. It
//! deliberately does not exercise ADR-038's two-stage routing, and cannot
//! detect a served model that loses tool-calling once Stage 2 injects its
//! candidate block — a real failure on at least one model reachable by this
//! path. `live_openai_compat_routing.rs` is the check for that.
//!
//! Ignored by default — it needs a real server. Run explicitly with Ollama (or
//! any OpenAI-compatible server) listening on [`BASE_URL`]:
//!
//! ```text
//! cargo test -p nodespace-agent --test live_openai_compat_smoke -- --ignored --nocapture
//! ```

use nodespace_agent::agent_types::{
    ChatInferenceEngine, InferenceRequest, StreamingChunk, ToolDefinition,
};
use nodespace_agent::local_agent::openai_compat_discovery::discover_models;
use nodespace_agent::local_agent::openai_compat_inference::OpenAiCompatInferenceEngine;
use nodespace_nlp_engine::chat::types::{ChatMessage, Role};
use std::sync::{Arc, Mutex};

const BASE_URL: &str = "http://127.0.0.1:11434/v1";

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server"]
async fn discovers_models_and_completes_a_tool_calling_turn() {
    // 1. Discovery: the /models listing that replaced the native manager.
    let models = discover_models(BASE_URL, "")
        .await
        .expect("discovery should reach the endpoint");
    assert!(!models.is_empty(), "expected at least one model");
    println!("discovered {} model(s): {models:?}", models.len());

    // Prefer a model known to support tools; fall back to whatever is served.
    let model = models
        .iter()
        .find(|m| m.starts_with("gemma4"))
        .cloned()
        .unwrap_or_else(|| models[0].clone());
    println!("using model: {model}");

    // 2. Inference: a streamed turn that must produce a tool call. This is the
    //    capability Ollama's OpenAI-compat layer has historically been shaky
    //    on, so plain completion working is not sufficient evidence.
    let engine =
        OpenAiCompatInferenceEngine::new(BASE_URL.to_string(), String::new(), model.clone());

    let request = InferenceRequest {
        messages: vec![ChatMessage::text(
            Role::User,
            "What is the weather in Paris right now? Use the tool.",
        )],
        tools: Some(vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get current weather for a city".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }]),
        temperature: Some(0.0),
        max_tokens: None,
    };

    let tool_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&tool_calls);

    let usage = engine
        .generate(
            request,
            Box::new(move |chunk| {
                if let StreamingChunk::ToolCallStart { name, .. } = chunk {
                    sink.lock().expect("sink not poisoned").push(name);
                }
            }),
        )
        .await
        .expect("generation succeeds");

    let observed = tool_calls.lock().expect("sink not poisoned").clone();
    println!("tool calls: {observed:?}, usage: {usage:?}");

    assert_eq!(
        observed,
        vec!["get_weather".to_string()],
        "model should have requested get_weather exactly once"
    );
}
