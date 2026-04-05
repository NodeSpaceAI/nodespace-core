//! Bridge between `nlp-engine::ChatEngine` and the `ChatInferenceEngine` trait.
//!
//! Adapts the nlp-engine's `ChatEngine` (which speaks its own `ChatChunk`,
//! `ChatMessageInput`, `ToolSpec` types) to the app-crate's
//! `ChatInferenceEngine` trait (which uses `StreamingChunk`, `ChatMessage`,
//! `ToolDefinition`). This is the same adapter pattern used for embeddings.

use std::sync::Arc;

use async_trait::async_trait;
use nodespace_nlp_engine::chat::{ChatChunk, ChatConfig, ChatEngine, ChatMessageInput, ToolSpec};

use crate::agent_types::{
    ChatInferenceEngine, ChatModelSpec, InferenceError, InferenceRequest, InferenceUsage, Role,
    StreamingChunk, ToolDefinition,
};

/// Chat inference engine backed by llama.cpp via `nlp-engine::ChatEngine`.
///
/// Thread-safe: the underlying `ChatEngine` serializes inference requests
/// via a tokio Mutex, preventing Metal command-buffer collisions.
pub struct LlamaChatInferenceEngine {
    engine: Arc<ChatEngine>,
    context_window: u32,
    default_temperature: f32,
}

impl LlamaChatInferenceEngine {
    /// Create a new engine, load the GGUF model, and return the bridge.
    ///
    /// This is a blocking operation (model load + Metal kernel compilation)
    /// and should be called from a context that can tolerate latency.
    pub fn load(model_path: &str, config: ChatConfig) -> Result<Self, InferenceError> {
        let context_window = config.n_ctx;
        let default_temperature = config.default_temperature;

        let engine = ChatEngine::new(config)
            .map_err(|e| InferenceError::Engine(format!("Failed to create ChatEngine: {e}")))?;

        engine
            .load_model(model_path)
            .map_err(|e| InferenceError::Engine(format!("Failed to load model: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
            context_window,
            default_temperature,
        })
    }
}

#[async_trait]
impl ChatInferenceEngine for LlamaChatInferenceEngine {
    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError> {
        // Convert ChatMessage → ChatMessageInput
        let messages: Vec<ChatMessageInput> = request
            .messages
            .iter()
            .map(|m| ChatMessageInput {
                role: match m.role {
                    Role::System => "system".to_string(),
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
                call_id: m.tool_call_id.clone(),
            })
            .collect();

        // Convert ToolDefinition → ToolSpec
        let tools: Option<Vec<ToolSpec>> = request.tools.map(|defs| {
            defs.into_iter()
                .map(|d| ToolSpec {
                    name: d.name,
                    description: d.description,
                    parameters_schema: d.parameters_schema,
                })
                .collect()
        });

        let temperature = request.temperature.unwrap_or(self.default_temperature);
        let max_tokens = request.max_tokens.unwrap_or(4096);

        // Bridge ChatChunk → StreamingChunk
        let usage_result = self
            .engine
            .generate_streaming(messages, tools, temperature, max_tokens, move |chunk| {
                match chunk {
                    ChatChunk::Token(text) => {
                        on_chunk(StreamingChunk::Token { text });
                    }
                    ChatChunk::ToolCallStart { id, name } => {
                        on_chunk(StreamingChunk::ToolCallStart { id, name });
                    }
                    ChatChunk::ToolCallArgs { id, json } => {
                        on_chunk(StreamingChunk::ToolCallArgs {
                            id,
                            args_json: json,
                        });
                    }
                    ChatChunk::Done => {
                        // Done is handled by the return value, not a chunk
                    }
                    ChatChunk::Error(msg) => {
                        tracing::error!("Inference error chunk: {}", msg);
                    }
                }
            })
            .await
            .map_err(|e| InferenceError::Engine(e.to_string()))?;

        Ok(InferenceUsage {
            prompt_tokens: usage_result.prompt_tokens,
            completion_tokens: usage_result.completion_tokens,
        })
    }

    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        Ok(Some(ChatModelSpec {
            model_id: self
                .engine
                .model_info()
                .map(|info| info.model_path.clone())
                .unwrap_or_default(),
            context_window: self.context_window,
            default_temperature: self.default_temperature,
        }))
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        self.engine
            .token_count(text)
            .map_err(|e| InferenceError::Engine(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Convert ToolDefinition → ToolSpec (utility for external callers)
// ---------------------------------------------------------------------------

/// Convert app-crate `ToolDefinition` to nlp-engine `ToolSpec`.
pub fn to_tool_spec(def: &ToolDefinition) -> ToolSpec {
    ToolSpec {
        name: def.name.clone(),
        description: def.description.clone(),
        parameters_schema: def.parameters_schema.clone(),
    }
}
