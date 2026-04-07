//! Integration tests for the Ollama backend.
//!
//! These tests connect to a real Ollama daemon at `http://127.0.0.1:11434`.
//! Each test gracefully skips if Ollama is not running — no failures, just a
//! printed message. Run with `cargo test -p nodespace-agent --test ollama_integration`.

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelManager, Role, StreamingChunk,
    ToolDefinition,
};
use nodespace_agent::local_agent::composite_model_manager::CompositeModelManager;
use nodespace_agent::local_agent::model_manager::GgufModelManager;
use nodespace_agent::local_agent::ollama_inference::OllamaInferenceEngine;
use nodespace_agent::local_agent::ollama_model_manager::OllamaModelManager;
use std::sync::Arc;

/// Returns true if Ollama is reachable at localhost:11434.
async fn ollama_running() -> bool {
    reqwest::Client::new()
        .get("http://127.0.0.1:11434/api/tags")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok()
}

/// Returns the name of the first available Ollama model, if any.
async fn first_ollama_model() -> Option<String> {
    let manager = OllamaModelManager::new();
    let models = manager.list().await.ok()?;
    models.into_iter().next().map(|m| m.id)
}

// ---------------------------------------------------------------------------
// OllamaModelManager integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_is_available() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_is_available: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    assert!(
        manager.is_available().await,
        "is_available() should return true"
    );
}

#[tokio::test]
async fn test_ollama_list_models() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_list_models: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    let models = manager.list().await.expect("list() should succeed");
    // Just check it returns without error — may be empty if no models pulled yet
    eprintln!("Ollama models found: {}", models.len());
    for m in &models {
        eprintln!("  - {} ({} bytes)", m.id, m.size_bytes);
    }
}

#[tokio::test]
async fn test_composite_list_includes_ollama_prefix() {
    if !ollama_running().await {
        eprintln!(
            "SKIP test_composite_list_includes_ollama_prefix: Ollama not running at localhost:11434"
        );
        return;
    }
    let gguf = Arc::new(GgufModelManager::new().expect("GgufModelManager::new()"));
    let ollama = Arc::new(OllamaModelManager::new());
    let composite = CompositeModelManager::new(gguf, ollama);

    let models = composite.list().await.expect("list() should succeed");

    // GGUF models should have no prefix
    let gguf_models: Vec<_> = models
        .iter()
        .filter(|m| !CompositeModelManager::is_ollama(&m.id))
        .collect();
    // Ollama models should have "ollama:" prefix
    let ollama_models: Vec<_> = models
        .iter()
        .filter(|m| CompositeModelManager::is_ollama(&m.id))
        .collect();

    eprintln!(
        "Composite list: {} GGUF + {} Ollama models",
        gguf_models.len(),
        ollama_models.len()
    );

    // All Ollama-prefixed models should start with "ollama:"
    for m in &ollama_models {
        assert!(
            m.id.starts_with("ollama:"),
            "Ollama model ID should start with 'ollama:': {}",
            m.id
        );
    }
}

#[tokio::test]
async fn test_ollama_recommended_model() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_recommended_model: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    let rec = manager
        .recommended_model()
        .await
        .expect("recommended_model()");
    assert!(
        !rec.is_empty(),
        "recommended_model() should return non-empty string"
    );
    eprintln!("Recommended Ollama model: {rec}");
}

// ---------------------------------------------------------------------------
// OllamaInferenceEngine integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_inference_generate() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_inference_generate: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_inference_generate: No Ollama models available");
        return;
    };
    eprintln!("Using model: {model_name}");

    let engine = OllamaInferenceEngine::new(model_name.clone());
    let request = InferenceRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: "Reply with exactly the word 'pong'. No other text.".to_string(),
            tool_call_id: None,
            name: None,
        }],
        tools: None,
        temperature: Some(0.0),
        max_tokens: Some(10),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = chunks.clone();

    let usage = engine
        .generate(
            request,
            Box::new(move |chunk| {
                chunks_clone.lock().unwrap().push(chunk);
            }),
        )
        .await
        .expect("generate() should succeed");

    let collected = chunks.lock().unwrap();
    eprintln!("Chunks received: {}", collected.len());
    eprintln!(
        "Usage: {} prompt + {} completion tokens",
        usage.prompt_tokens, usage.completion_tokens
    );

    // Should have received at least one token chunk
    let has_token = collected
        .iter()
        .any(|c| matches!(c, StreamingChunk::Token { .. }));
    assert!(has_token, "Should receive at least one Token chunk");
}

#[tokio::test]
async fn test_ollama_inference_with_tools() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_inference_with_tools: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_inference_with_tools: No Ollama models available");
        return;
    };
    eprintln!("Using model for tool test: {model_name}");

    let engine = OllamaInferenceEngine::new(model_name);
    let tool = ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get the current weather for a city".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        }),
    };

    let request = InferenceRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: "What's the weather in London?".to_string(),
            tool_call_id: None,
            name: None,
        }],
        tools: Some(vec![tool]),
        temperature: Some(0.0),
        max_tokens: Some(100),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = chunks.clone();

    let _usage = engine
        .generate(
            request,
            Box::new(move |chunk| {
                chunks_clone.lock().unwrap().push(chunk);
            }),
        )
        .await
        .expect("generate() with tools should succeed");

    let collected = chunks.lock().unwrap();
    eprintln!("Tool test chunks received: {}", collected.len());

    // Model may or may not call the tool — both are valid responses.
    // Not all models support tool calling; some return no chunks in that case.
    // Verify only that no Error chunks were emitted.
    let has_error = collected
        .iter()
        .any(|c| matches!(c, StreamingChunk::Error { .. }));
    assert!(!has_error, "Should not receive Error chunks");
    eprintln!(
        "Tool call chunks: {} (model may not support tool calling)",
        collected.len()
    );
}

#[tokio::test]
async fn test_ollama_model_info() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_model_info: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_model_info: No Ollama models available");
        return;
    };

    let engine = OllamaInferenceEngine::new(model_name.clone());
    let info = engine
        .model_info()
        .await
        .expect("model_info() should not error");

    eprintln!("model_info() for {model_name}: {:?}", info);
    // model_info returns None if /api/show fails — that's acceptable
    if let Some(spec) = info {
        assert_eq!(spec.model_id, model_name);
        assert!(spec.context_window > 0);
    }
}
