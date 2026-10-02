//! Bridge between `nlp-engine::ChatEngine` and the `ChatInferenceEngine` trait.
//!
//! Adapts the nlp-engine's `ChatEngine` (which speaks its own `ChatChunk` and
//! `ToolSpec` types) to the app-crate's `ChatInferenceEngine` trait (which uses
//! `StreamingChunk` and `ToolDefinition`). `ChatMessage` is the canonical type
//! shared by both crates — no conversion needed on the message path.

use std::sync::Arc;

use async_trait::async_trait;
use nodespace_nlp_engine::chat::{ChatChunk, ChatConfig, ChatEngine, ToolSpec};

use crate::agent_types::{
    ChatInferenceEngine, ChatModelSpec, InferenceError, InferenceRequest, InferenceUsage,
    ModelFamily, StreamingChunk, ToolDefinition,
};

/// Chat inference engine backed by llama.cpp via `nlp-engine::ChatEngine`.
///
/// Thread-safe: the underlying `ChatEngine` serializes inference requests
/// via a tokio Mutex, preventing Metal command-buffer collisions.
pub struct LlamaChatInferenceEngine {
    engine: Arc<ChatEngine>,
    family: ModelFamily,
    context_window: u32,
    default_temperature: f32,
}

impl LlamaChatInferenceEngine {
    /// Create a new engine, load the GGUF model, and return the bridge.
    ///
    /// This is a blocking operation (model load + Metal kernel compilation)
    /// and should be called from a context that can tolerate latency.
    pub fn load(
        model_path: &str,
        family: ModelFamily,
        config: ChatConfig,
    ) -> Result<Self, InferenceError> {
        let context_window = config.n_ctx;
        let default_temperature = config.default_temperature;

        let engine = ChatEngine::new(config)
            .map_err(|e| InferenceError::Engine(format!("Failed to create ChatEngine: {e}")))?;

        // Look up the pinned digest for this model file so the load-time integrity
        // gate refuses a swapped-on-disk GGUF before native llama.cpp parses it. A
        // path outside the catalog (a user-supplied model) resolves to None and
        // loads without the gate — there is nothing to verify against.
        let expected_sha256 = std::path::Path::new(model_path)
            .file_name()
            .and_then(|f| f.to_str())
            .and_then(super::model_manager::expected_sha256_for_filename);

        engine
            .load_model(model_path, expected_sha256)
            .map_err(|e| InferenceError::Engine(format!("Failed to load model: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
            family,
            context_window,
            default_temperature,
        })
    }
}

#[async_trait]
impl ChatInferenceEngine for LlamaChatInferenceEngine {
    /// The model is loaded in this process.
    fn runs_on_this_machine(&self) -> bool {
        true
    }

    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError> {
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
        // 2048 fallback for GGUF path when no cap is requested (tool-calling iterations
        // pass max_tokens: None to avoid truncating argument JSON mid-field). Ollama uses
        // stream:false for tool turns so its effective cap is the model's own EOS — the two
        // backends diverge here intentionally.
        //
        // 2048 is sufficient for any realistic tool-call argument blob and bounds runaway
        // generation (e.g. Gemma 4 12B hitting the old 4096 ceiling on every turn). This
        // mirrors MAX_RESPONSE_TOKENS (agent_loop.rs) so the per-turn token budget is
        // symmetric: tool iterations and final replies are both capped at 2048.
        let max_tokens = request.max_tokens.unwrap_or(2048);

        // Bridge ChatChunk → StreamingChunk
        let usage_result = self
            .engine
            .generate_streaming(
                request.messages,
                tools,
                temperature,
                max_tokens,
                move |chunk| bridge_chat_chunk(chunk, on_chunk.as_ref()),
            )
            .await
            .map_err(|e| InferenceError::Engine(e.to_string()))?;

        Ok(InferenceUsage {
            prompt_tokens: usage_result.prompt_tokens,
            completion_tokens: usage_result.completion_tokens,
        })
    }

    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        // Report the *effective* context window the engine actually allocated
        // (sized to available memory at load time), not the configured ceiling.
        // Callers that budget conversation history against this must see the
        // real window, or they pack more tokens than the model can hold. Falls
        // back to the configured value if the model is not loaded yet.
        let loaded = self.engine.model_info();
        let context_window = loaded
            .as_ref()
            .map(|info| info.context_size)
            .unwrap_or(self.context_window);
        Ok(Some(ChatModelSpec {
            model_id: loaded.map(|info| info.model_path).unwrap_or_default(),
            family: self.family,
            context_window,
            default_temperature: self.default_temperature,
            type_k: None,
            type_v: None,
        }))
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        self.engine
            .token_count(text)
            .map_err(|e| InferenceError::Engine(e.to_string()))
    }
}

/// Maps one nlp-engine `ChatChunk` to the app-crate `StreamingChunk` and
/// forwards it via `on_chunk`.
///
/// Extracted from the inline closure passed to `generate_streaming` so the
/// mapping is directly unit-testable without a loaded model -- in
/// particular, that a mid-generation `ChatChunk::Error` (e.g. the context
/// window filling up during generation) actually reaches the caller as a
/// `StreamingChunk::Error` instead of only being logged. Before this, the
/// underlying engine still finished and reported success on a mid-generation
/// overflow, so a silently truncated response was presented to the user as
/// a complete, successful answer. Closing that required two changes, not
/// one: this bridge forwarding the chunk at all (what this function fixes),
/// and the ReAct loop (`agent_loop::run_turn`) actually treating an error
/// chunk as a failed turn instead of a normal, complete response -- see
/// `run_turn`'s use of `parse_chunks`'s error field for the other half.
fn bridge_chat_chunk(chunk: ChatChunk, on_chunk: &dyn Fn(StreamingChunk)) {
    match chunk {
        ChatChunk::Token(text) => {
            on_chunk(StreamingChunk::Token { text });
        }
        ChatChunk::Reasoning(text) => {
            on_chunk(StreamingChunk::Reasoning { text });
        }
        ChatChunk::ToolCallStart { id, name } => {
            on_chunk(StreamingChunk::ToolCallStart {
                id,
                name,
                provider_extra: None,
            });
        }
        ChatChunk::ToolCallArgs { id, json } => {
            on_chunk(StreamingChunk::ToolCallArgs {
                id,
                args_json: json,
            });
        }
        ChatChunk::Done => {
            // Done is handled by the return value, not a chunk.
        }
        ChatChunk::Error(msg) => {
            tracing::error!("Inference error chunk: {}", msg);
            on_chunk(StreamingChunk::Error { message: msg });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Drives `bridge_chat_chunk` with a spy `on_chunk` and returns everything
    /// it forwarded. `RefCell`, not a channel/mutex, is enough here since the
    /// call is synchronous and single-threaded.
    fn bridge(chat_chunk: ChatChunk) -> Vec<StreamingChunk> {
        let collected: RefCell<Vec<StreamingChunk>> = RefCell::new(Vec::new());
        bridge_chat_chunk(chat_chunk, &|chunk| collected.borrow_mut().push(chunk));
        collected.into_inner()
    }

    /// Regression guard: a mid-generation `ChatChunk::Error` must reach the
    /// caller as a `StreamingChunk::Error`, not just a log line. Before this
    /// fix, the underlying engine still finished and reported success on a
    /// mid-generation context-window overflow, so a truncated response was
    /// silently presented as a complete, successful answer.
    #[test]
    fn error_chunk_reaches_on_chunk_as_streaming_error() {
        let chunks = bridge(ChatChunk::Error("Context window full".to_string()));
        assert_eq!(
            chunks.len(),
            1,
            "a ChatChunk::Error must forward exactly one StreamingChunk"
        );
        match &chunks[0] {
            StreamingChunk::Error { message } => {
                assert_eq!(message, "Context window full");
            }
            other => panic!("expected StreamingChunk::Error, got {other:?}"),
        }
    }

    /// `Done` is signaled to the caller via `generate`'s return value, not a
    /// chunk -- must stay a no-op here, or callers would see a spurious
    /// forwarded chunk with no counterpart on the `ChatChunk` side.
    #[test]
    fn done_chunk_forwards_nothing() {
        let chunks = bridge(ChatChunk::Done);
        assert!(
            chunks.is_empty(),
            "Done must not forward a chunk -- it's signaled via the return value"
        );
    }

    #[test]
    fn token_chunk_maps_straight_through() {
        let chunks = bridge(ChatChunk::Token("hello".to_string()));
        match &chunks[0] {
            StreamingChunk::Token { text } => assert_eq!(text, "hello"),
            other => panic!("expected StreamingChunk::Token, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_chunks_map_field_for_field() {
        let start = bridge(ChatChunk::ToolCallStart {
            id: "call-1".to_string(),
            name: "create_node".to_string(),
        });
        match &start[0] {
            StreamingChunk::ToolCallStart {
                id,
                name,
                provider_extra,
            } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "create_node");
                assert!(provider_extra.is_none());
            }
            other => panic!("expected StreamingChunk::ToolCallStart, got {other:?}"),
        }

        let args = bridge(ChatChunk::ToolCallArgs {
            id: "call-1".to_string(),
            json: "{\"a\":1}".to_string(),
        });
        match &args[0] {
            StreamingChunk::ToolCallArgs { id, args_json } => {
                assert_eq!(id, "call-1");
                assert_eq!(args_json, "{\"a\":1}");
            }
            other => panic!("expected StreamingChunk::ToolCallArgs, got {other:?}"),
        }
    }
}
