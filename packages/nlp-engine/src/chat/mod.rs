/// Chat inference engine using llama.cpp.
///
/// Provides streaming text generation from GGUF chat models with tool-call
/// parsing via llama.cpp's native `ChatParseStateOaicompat` streaming parser.
/// This handles all model families (Mistral, Gemma 4, etc.) natively at the
/// C++ level without custom sentinel detection. Designed to coexist with the
/// embedding service on the same GPU (validated in PoC with shared Metal backend).
///
/// # Architecture
///
/// The `ChatEngine` lives in the nlp-engine crate and exposes a
/// crate-local API. The app crate wraps it to implement the
/// `ChatInferenceEngine` trait — the same pattern used for embeddings.
///
/// # GPU Scheduling
///
/// A `tokio::sync::Mutex` serializes all inference requests so that only
/// one generation runs at a time. This prevents Metal command-buffer
/// collisions between concurrent requests.
pub mod error;
pub mod parser;
pub mod prompt_dump;
pub mod types;

pub use error::{ChatError, Result};
pub use parser::{parse_tool_calls, ParseResult, StreamingToolCallParser};
pub use types::{
    ChatChunk, ChatConfig, ChatMessage, ChatUsage, LoadedModelInfo, Role, ToolCallRaw, ToolSpec,
};

#[cfg(feature = "chat-service")]
use crate::embedding::{get_or_init_backend, register_atexit_handler, register_state_for_cleanup};

#[cfg(feature = "chat-service")]
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
#[cfg(feature = "chat-service")]
use llama_cpp_2::context::LlamaContext;
#[cfg(feature = "chat-service")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "chat-service")]
use llama_cpp_2::model::ChatTemplateResult;
#[cfg(feature = "chat-service")]
use llama_cpp_2::model::{AddBos, LlamaModel};
#[cfg(any(feature = "chat-service", test))]
use llama_cpp_2::model::{GrammarTrigger, GrammarTriggerType};
#[cfg(feature = "chat-service")]
use llama_cpp_2::openai::OpenAIChatTemplateParams;
#[cfg(feature = "chat-service")]
use llama_cpp_2::sampling::LlamaSampler;

#[cfg(feature = "chat-service")]
use std::sync::{Arc, Mutex};

/// Helper to convert backend init errors into ChatError.
#[cfg(feature = "chat-service")]
fn backend() -> Result<crate::embedding::BackendGuard> {
    get_or_init_backend().map_err(ChatError::ModelLoadError)
}

/// Chat inference engine backed by llama.cpp.
///
/// Loads a GGUF chat model and provides streaming text generation.
/// Thread-safe: a `tokio::sync::Mutex` serializes inference requests.
pub struct ChatEngine {
    config: ChatConfig,
    #[cfg(feature = "chat-service")]
    state: Arc<Mutex<Option<ChatLlamaState>>>,
    #[cfg(feature = "chat-service")]
    inference_lock: tokio::sync::Mutex<()>,
}

/// Internal state holding the loaded model and its context.
///
/// # Safety
///
/// Uses the same lifetime-extension pattern as `embedding.rs::LlamaState`.
/// The context is created with a transmuted `'static` lifetime because:
/// 1. The context is stored alongside the model that owns it.
/// 2. Drop order is guaranteed: context drops before model.
/// 3. Access is serialized through the outer Mutex.
#[cfg(feature = "chat-service")]
struct ChatLlamaState {
    model: LlamaModel,
    context: Option<LlamaContext<'static>>,
    model_path: String,
    context_size: u32,
    n_threads: i32,
    type_k: Option<crate::chat::types::KvCacheQuantType>,
    type_v: Option<crate::chat::types::KvCacheQuantType>,
    /// Tokens from the last decoded prompt, used to find the reusable prefix.
    cached_prompt: Vec<llama_cpp_2::token::LlamaToken>,
}

#[cfg(feature = "chat-service")]
impl ChatLlamaState {
    fn new(
        model: LlamaModel,
        model_path: String,
        context_size: u32,
        n_threads: i32,
        type_k: Option<crate::chat::types::KvCacheQuantType>,
        type_v: Option<crate::chat::types::KvCacheQuantType>,
    ) -> Self {
        Self {
            model,
            context: None,
            model_path,
            context_size,
            n_threads,
            type_k,
            type_v,
            cached_prompt: Vec::new(),
        }
    }

    /// Get or create the generation context.
    ///
    /// Unlike the embedding context, the chat context does NOT use embeddings
    /// mode, and its batch size is a fixed constant independent of the context
    /// window (see [`PREFILL_BATCH_TOKENS`]) so the compute buffer does not
    /// scale with `n_ctx`.
    fn get_or_create_context(&mut self) -> Result<&mut LlamaContext<'static>> {
        if self.context.is_none() {
            // `context_size` was already fitted to this machine's memory at load
            // time (see `load_model`), so the KV cache below allocates exactly
            // the window `model_info` reports and the overflow/stop guards
            // measure.
            tracing::info!(
                "Creating chat LlamaContext (n_ctx={}, n_threads={}, type_k={:?}, type_v={:?})",
                self.context_size,
                self.n_threads,
                self.type_k,
                self.type_v,
            );

            let mut ctx_params = LlamaContextParams::default()
                .with_n_ctx(std::num::NonZeroU32::new(self.context_size))
                .with_n_batch(PREFILL_BATCH_TOKENS)
                .with_n_ubatch(PREFILL_UBATCH_TOKENS)
                .with_n_threads(self.n_threads)
                .with_n_threads_batch(self.n_threads);

            if let Some(k) = self.type_k {
                ctx_params = ctx_params.with_type_k(kv_quant_to_llama(k));
            }
            if let Some(v) = self.type_v {
                ctx_params = ctx_params.with_type_v(kv_quant_to_llama(v));
            }

            let backend = backend()?;
            let ctx = self.model.new_context(&backend, ctx_params).map_err(|e| {
                ChatError::InferenceError(format!("Context creation failed: {}", e))
            })?;

            // SAFETY: Same pattern as embedding.rs. The context is stored in this
            // struct alongside model. Drop order is guaranteed (context before model).
            let ctx: LlamaContext<'static> = unsafe { std::mem::transmute(ctx) };
            self.context = Some(ctx);

            tracing::info!("Chat context created — Metal kernels compiled");
        }

        Ok(self.context.as_mut().expect("context just created"))
    }

    /// Tear down a context that just failed a `llama_decode` call.
    ///
    /// llama.cpp does not recover a backend on its own after a compute
    /// failure (its own log says "recreate the backend to recover" and
    /// nothing does) — the Metal `sched`/command queue stays in the failed
    /// state and every future decode on the same context fails identically.
    /// Dropping the context here, plus the prefix cache it invalidates,
    /// makes the *next* `get_or_create_context()` call build a fresh context
    /// (and thus a fresh Metal backend/command queue) instead of reusing the
    /// poisoned one — turning a permanent outage into one failed turn.
    fn poison_context(&mut self) {
        self.context = None;
        self.cached_prompt.clear();
    }
}

#[cfg(feature = "chat-service")]
unsafe impl Send for ChatLlamaState {}
#[cfg(feature = "chat-service")]
unsafe impl Sync for ChatLlamaState {}

impl ChatEngine {
    /// Create a new chat engine with the given configuration.
    pub fn new(config: ChatConfig) -> Result<Self> {
        config.validate().map_err(ChatError::ConfigError)?;

        Ok(Self {
            config,
            #[cfg(feature = "chat-service")]
            state: Arc::new(Mutex::new(None)),
            #[cfg(feature = "chat-service")]
            inference_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Load a GGUF chat model from the given path.
    ///
    /// The model file must exist and be a valid GGUF file with an embedded
    /// chat template. GPU layers are offloaded according to `ChatConfig`.
    ///
    /// When `expected_sha256` is `Some`, the on-disk file is verified against
    /// that digest before it is handed to native llama.cpp — closing the
    /// post-install tamper window (a model correctly downloaded but later
    /// swapped on disk is refused). `None` is the escape hatch for a
    /// user-supplied / non-catalog model with no pinned digest: it loads with a
    /// warning rather than failing, since there is nothing to verify against.
    pub fn load_model(&self, model_path: &str, expected_sha256: Option<&str>) -> Result<()> {
        #[cfg(feature = "chat-service")]
        {
            tracing::info!("Loading chat model: {}", model_path);

            let path = std::path::Path::new(model_path);
            if !path.exists() {
                return Err(ChatError::ModelLoadError(format!(
                    "Model file not found: {}",
                    model_path
                )));
            }

            // Integrity gate: reject a tampered/substituted artifact before native
            // llama.cpp parses it with full authority. Mirrors the embedding-model
            // load gate; the digest is the selected catalog entry's pinned SHA-256.
            //
            // Uses the verified-state cache: an unchanged file that was already
            // verified (at download or a prior load) is not re-hashed. Any change
            // to the file's identity (size/mtime/inode) still forces a full
            // re-hash, so a post-install tamper is still refused.
            match expected_sha256 {
                Some(expected) => {
                    crate::config::verify_file_sha256_cached(path, expected)
                        .map_err(ChatError::IntegrityError)?;
                    tracing::info!("Chat model integrity verified: {}", model_path);
                }
                None => {
                    tracing::warn!(
                        "Loading chat model WITHOUT integrity verification (no pinned \
                         digest for a user-supplied / non-catalog model): {}",
                        model_path
                    );
                }
            }

            // Get global backend (shares with embedding service)
            let backend = backend()?;

            let model_params =
                LlamaModelParams::default().with_n_gpu_layers(self.config.n_gpu_layers);

            let model = LlamaModel::load_from_file(&backend, path, &model_params)
                .map_err(|e| ChatError::ModelLoadError(format!("Failed to load model: {}", e)))?;

            tracing::info!(
                "Chat model loaded: vocab_size={}, n_ctx_train={}",
                model.n_vocab(),
                model.n_ctx_train(),
            );

            // Decide the effective context window now, while the model geometry
            // and free memory are both known, so it is reported by `model_info`
            // immediately after load — the agent's history-budgeting layer reads
            // it to size summarization. The context itself is still created
            // lazily on first generation, but from this same fixed value.
            let effective_n_ctx = compute_effective_n_ctx(
                &model,
                model_path,
                self.config.n_ctx,
                self.config.type_k,
                self.config.type_v,
            )?;

            let state = ChatLlamaState::new(
                model,
                model_path.to_string(),
                effective_n_ctx,
                self.config.n_threads,
                self.config.type_k,
                self.config.type_v,
            );

            {
                let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
                *guard = Some(state);
            }

            register_atexit_handler();
            register_state_for_cleanup(&self.state);

            tracing::info!("Chat model ready for inference");
        }

        #[cfg(not(feature = "chat-service"))]
        {
            let _ = model_path;
            let _ = expected_sha256;
            tracing::info!("STUB: Chat model load (feature disabled)");
        }

        Ok(())
    }

    /// Run streaming inference on a conversation.
    ///
    /// Applies the model's built-in chat template, generates tokens one by one,
    /// and invokes `on_chunk` for each token. Tool calls are detected by the
    /// streaming parser and emitted as `ChatChunk::ToolCallStart` /
    /// `ChatChunk::ToolCallArgs` events.
    ///
    /// Returns usage statistics when generation completes.
    ///
    /// # GPU Scheduling
    ///
    /// This method acquires a tokio Mutex to ensure only one generation runs
    /// at a time. Concurrent callers will wait.
    pub async fn generate_streaming(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        temperature: f32,
        max_tokens: u32,
        on_chunk: impl Fn(ChatChunk) + Send + 'static,
    ) -> Result<ChatUsage> {
        #[cfg(feature = "chat-service")]
        {
            // Serialize inference requests
            let _lock = self.inference_lock.lock().await;

            // Run the blocking llama.cpp inference on a blocking thread.
            // The effective context window is decided inside `generate_blocking`
            // when the context is created (sized to available memory), not from
            // the configured ceiling here.
            let state = Arc::clone(&self.state);

            tokio::task::spawn_blocking(move || {
                Self::generate_blocking(&state, messages, tools, temperature, max_tokens, &on_chunk)
            })
            .await
            .map_err(|e| ChatError::InferenceError(format!("Task join error: {}", e)))?
        }

        #[cfg(not(feature = "chat-service"))]
        {
            let _ = (messages, tools, temperature, max_tokens);
            on_chunk(ChatChunk::Token("STUB: chat disabled".to_string()));
            on_chunk(ChatChunk::Done);
            Ok(ChatUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
            })
        }
    }

    /// Blocking inference implementation (runs on a blocking thread).
    #[cfg(feature = "chat-service")]
    fn generate_blocking(
        state: &Arc<Mutex<Option<ChatLlamaState>>>,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        temperature: f32,
        max_tokens: u32,
        on_chunk: &(impl Fn(ChatChunk) + Send),
    ) -> Result<ChatUsage> {
        let mut state_guard = state.lock().unwrap_or_else(|p| p.into_inner());
        let llama = state_guard.as_mut().ok_or(ChatError::ModelNotLoaded)?;

        // --- Apply chat template ---
        let tmpl_result = Self::apply_chat_template(&llama.model, &messages, &tools)?;
        tracing::debug!(
            "Chat template applied: chat_format={} parse_tool_calls={} additional_stops={:?}",
            tmpl_result.chat_format,
            tmpl_result.parse_tool_calls,
            tmpl_result.additional_stops,
        );
        // chat_format=0 (CONTENT_ONLY) means llama.cpp failed to detect the model's
        // specialized format (e.g. COMMON_CHAT_FORMAT_PEG_GEMMA4=3). When tools are
        // provided this is a routing failure — tool calls will be emitted as plain text
        // and never parsed. Warn so the failure is visible without a debug build.
        if let Some(active_tools) = tools.as_ref().filter(|t| !t.is_empty()) {
            if tmpl_result.chat_format == 0 {
                tracing::warn!(
                    "chat_format=0 (CONTENT_ONLY) with {} tools — specialized template \
                     detection may have failed; tool calls will not be parsed",
                    active_tools.len(),
                );
            }
        }
        let prompt = &tmpl_result.prompt;
        // `.chars().take(N)` (not a `[..N]` byte slice) -- a byte slice can
        // panic when byte 200 falls inside a multi-byte UTF-8 character,
        // reachable with any non-ASCII prompt content. Says explicitly when
        // truncation happened and points at the full-fidelity dump so this
        // preview never reads as if it were the whole prompt (see
        // prompt_dump's module doc).
        let mut prompt_chars = prompt.chars();
        let preview: String = prompt_chars.by_ref().take(200).collect();
        if prompt_chars.next().is_some() {
            tracing::debug!(
                "Chat prompt ({} chars, showing first 200; set NODESPACE_PROMPT_DUMP for the full text): {:?}",
                prompt.len(),
                preview
            );
        } else {
            tracing::debug!("Chat prompt ({} chars): {:?}", prompt.len(), preview);
        }
        // Dev-only full-text dump (NODESPACE_PROMPT_DUMP) — see prompt_dump's
        // module doc. This is the single chokepoint every native-path caller
        // (Stage 1 routing, Stage 2 ReAct turns, resolve_query, the routing
        // probe) passes through, so capturing here covers all of them without
        // per-caller wiring. `dump_seq` correlates this prompt with its
        // response below.
        let dump_seq = prompt_dump::dump_prompt(prompt);

        // --- Tokenize ---
        // AddBos::Never -- the OAI-compat Jinja template above already injects
        // BOS where appropriate, and adding it again here would double-BOS.
        let tokens = llama
            .model
            .str_to_token(prompt, AddBos::Never)
            .map_err(|e| ChatError::TokenizationError(e.to_string()))?;

        let prompt_tokens = tokens.len() as u32;

        tracing::debug!("Prompt tokenized: {} tokens", prompt_tokens);

        // --- Extract model info before taking mutable borrow for context ---
        // Additional stop sequences from the chat template (e.g. Gemma 4's "<end_of_turn>").
        // For Gemma 4 (chat_format=3), the ggml-org GGUF does not mark <turn|> as EOG in
        // its vocabulary, so llama.cpp's template engine may not include it in additional_stops.
        // We inject the full set of Gemma 4 turn-end tokens unconditionally when the format
        // is detected as PEG_GEMMA4, guarding against incomplete vocabulary metadata.
        let mut additional_stops = tmpl_result.additional_stops.clone();
        augment_gemma4_stops(tmpl_result.chat_format, &mut additional_stops);

        // --- Prepare context and batch ---
        // Find the longest token prefix shared with the last decoded prompt.
        // Any matching prefix is already in the KV cache — only the delta needs
        // decoding, which eliminates redundant attention computation for the
        // stable system prompt on every ReAct iteration.
        //
        // Compute prefix metrics before the context borrow so the borrow
        // checker sees no overlap between `llama.cached_prompt` reads and the
        // `&mut llama` taken by `get_or_create_context`.
        let prefix_len = tokens
            .iter()
            .zip(llama.cached_prompt.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let has_reusable_prefix = prefix_len > 0 && prefix_len < tokens.len();

        // Ensure the context exists (this sizes the KV cache to available memory
        // and stores the effective window back into `llama.context_size`), then
        // read the effective window for the guards below. Done as its own
        // statement so the `&mut` borrow ends before we read `context_size`.
        llama.get_or_create_context()?;
        let effective_n_ctx = llama.context_size;

        // Reject a prompt that cannot fit in the (possibly reduced) window.
        if prompt_tokens >= effective_n_ctx {
            return Err(ChatError::ContextOverflow(format!(
                "Prompt uses {} tokens but context window is {}",
                prompt_tokens, effective_n_ctx
            )));
        }

        let ctx = llama
            .context
            .as_mut()
            .expect("context was just created by get_or_create_context");

        // Determine how many tokens to skip in the batch.  If the KV trim
        // succeeds we decode only the delta; if it fails we fall back to a
        // full decode so the batch and KV cache are never out of sync.
        let decode_from = if has_reusable_prefix {
            tracing::debug!(
                "KV cache reuse: {} prefix tokens cached, decoding {} delta tokens",
                prefix_len,
                tokens.len() - prefix_len
            );
            match ctx.clear_kv_cache_seq(Some(0), Some(prefix_len as u32), None) {
                Ok(true) => prefix_len,
                // `false` means the backend (e.g. recurrent models) does not
                // support partial removal — fall back to full decode so the
                // KV cache and batch are never out of sync.
                Ok(false) => {
                    tracing::warn!(
                        "KV cache partial trim returned false, falling back to full decode"
                    );
                    ctx.clear_kv_cache();
                    0
                }
                Err(e) => {
                    tracing::warn!("KV cache trim failed ({}), falling back to full decode", e);
                    ctx.clear_kv_cache();
                    0
                }
            }
        } else {
            // No reusable prefix (first call, or prompt changed entirely): full decode.
            ctx.clear_kv_cache();
            0
        };

        // Decode the prompt (or just the delta on subsequent calls) in chunks of
        // at most `PREFILL_BATCH_TOKENS`. `llama_decode` rejects a batch larger
        // than the context's `n_batch`, which is now a fixed constant rather
        // than the full window, so a long prompt must be split across several
        // decode calls. `pos` stays the absolute position in `tokens` so the
        // KV cache and RoPE see an unbroken sequence across chunk boundaries.
        let pending = &tokens[decode_from..];
        debug_assert!(
            !pending.is_empty(),
            "decode_from must stay below tokens.len() so the final token produces logits"
        );
        let mut batch = llama_cpp_2::llama_batch::LlamaBatch::new(PREFILL_BATCH_TOKENS as usize, 1);
        let last_idx = tokens.len() - 1;

        for (chunk_idx, chunk) in pending.chunks(PREFILL_BATCH_TOKENS as usize).enumerate() {
            batch.clear();
            let chunk_start = prefill_chunk_start(decode_from, chunk_idx);
            for (i, &token) in chunk.iter().enumerate() {
                let pos = chunk_start + i;
                // Only the final token of the whole prompt needs logits — that
                // is the position sampling reads from.
                if let Err(e) = batch.add(token, pos as i32, &[0], pos == last_idx) {
                    // The KV trim above may already have mutated the cache for
                    // this prompt while `cached_prompt` still describes the
                    // previous one — poison so the next call can't compute a
                    // prefix match against a cache that no longer agrees with it.
                    llama.poison_context();
                    return Err(ChatError::InferenceError(format!(
                        "Batch add failed: {}",
                        e
                    )));
                }
            }
            if let Err(e) = ctx.decode(&mut batch) {
                llama.poison_context();
                return Err(ChatError::BackendDecodeFailed(format!(
                    "Prompt decode failed: {}",
                    e
                )));
            }
        }

        // The KV cache now exactly matches `tokens` (prefill above completed
        // successfully). Update `cached_prompt` here rather than only on
        // successful loop exit — the grammar sampler and streaming-parser
        // init below can each fail and return via `?` before generation
        // starts, and leaving `cached_prompt` stale in that window lets a
        // later call trust a KV region that no longer agrees with it. On
        // context overflow (checked below) this gets cleared, so a
        // subsequent overflow-driven clear always wins over this write.
        llama.cached_prompt = tokens.clone();

        // --- Sampling setup ---
        // A repetition penalty runs first so it reshapes the logits before the
        // temperature softens them. Without it, small models at near-greedy
        // temperature (0.1) fall into degenerate loops — repeating a closing
        // phrase like "I'm ready to help you. What would you like to do next?"
        // until they exhaust max_tokens (a ~100s hang that never emits the
        // <end_of_turn> stop token). penalty_last_n=256 covers the recent
        // window; repeat=1.3 is firm enough to escape a loop without distorting
        // normal prose. Frequency/presence penalties stay disabled (0.0).
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(256, 1.3, 0.0, 0.0),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(0), // seed=0 for deterministic given temperature
        ]);

        // When tools are offered, `apply_chat_template` (via llama.cpp's
        // `common_chat_templates_apply`) already computed a tool-call grammar
        // scoped to exactly the active tool set for this turn. A turn with no
        // tools produces `grammar: None`, so plain chat is unaffected. Applied
        // below via `sample_with_grammar_rejection` — see its doc comment for
        // why this is rejection sampling rather than grammar-first.
        let mut grammar_sampler = build_grammar_sampler(&llama.model, &tmpl_result)?;

        // --- Initialize llama.cpp's native streaming tool-call parser ---
        // ChatParseStateOaicompat handles all model families (Mistral, Gemma 4,
        // etc.) natively at the C++ level — no custom sentinel detection needed.
        let mut oai_parser = tmpl_result.streaming_state_oaicompat().map_err(|e| {
            ChatError::InferenceError(format!("Failed to init streaming parser: {}", e))
        })?;

        // Track tool call ids by index so ToolCallStart/Args pair correctly.
        let mut tool_call_ids: std::collections::HashMap<u32, String> =
            std::collections::HashMap::new();

        // Route the model's `<|channel> … <channel|>` (Gemma 4) reasoning out
        // of the answer stream. Held across the whole loop so a span (or
        // marker) that straddles a delta boundary is resolved correctly.
        let mut channel_splitter = ChannelSplitter::default();

        // --- Token generation loop ---
        // Reborrow model and context separately to satisfy the borrow checker.
        // get_or_create_context() ensured the context exists, so we can safely
        // split the struct fields.
        let model_ref = &llama.model;
        let ctx = llama.context.as_mut().expect("context was just created");

        let mut piece_decoder = encoding_rs::UTF_8.new_decoder();
        let mut completion_tokens: u32 = 0;
        let mut n_cur = tokens.len();
        let mut context_overflowed = false;
        let mut oai_parser_errors: u32 = 0;
        // Accumulates the raw, pre-normalization token stream for
        // NODESPACE_PROMPT_DUMP — see prompt_dump's module doc. `push_str`
        // is a no-op cost when dumping is disabled (dump_prompt already
        // returned a seq either way; this just also captures the response).
        let mut raw_response_accum = String::new();
        // Hoisted out of the per-token loop below: `enabled()` reads
        // NODESPACE_PROMPT_DUMP via std::env::var(), so checking it once per
        // generated token (rather than once per generation) pays an env
        // lookup on every token even when dumping is disabled, the common
        // case.
        let dump_enabled = prompt_dump::enabled();

        loop {
            if completion_tokens >= max_tokens {
                tracing::debug!("Max tokens reached ({})", max_tokens);
                break;
            }

            if n_cur as u32 >= effective_n_ctx {
                on_chunk(ChatChunk::Error("Context window full".to_string()));
                context_overflowed = true;
                break;
            }

            // Sample next token. This indexes the *last* batch decoded, which on
            // the first pass is the final prefill chunk — correct because the
            // prompt's last token always lands in that chunk's final slot (see
            // `prefill_chunk_positions`), so the logits it carries are the ones
            // to sample from. A change to the chunking that broke that invariant
            // would silently sample the wrong position.
            let new_token = sample_with_grammar_rejection(
                ctx,
                batch.n_tokens() - 1,
                &mut sampler,
                grammar_sampler.as_mut(),
            );

            // Use is_eog_token() rather than a bare EOS comparison: models like Gemma 4
            // have multiple EOG tokens (EOS, <end_of_turn>, <turn|>) and is_eog_token
            // covers all of them via llama.cpp's vocabulary flags.
            if model_ref.is_eog_token(new_token) {
                tracing::debug!(
                    "EOG token ({}) after {} completion tokens",
                    new_token.0,
                    completion_tokens
                );
                break;
            }

            completion_tokens += 1;

            // Convert token to text (special=true so all tokens decode cleanly)
            let piece = match model_ref.token_to_piece(new_token, &mut piece_decoder, true, None) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to decode token {}: {}", new_token.0, e);
                    batch.clear();
                    if let Err(e) = batch.add(new_token, n_cur as i32, &[0], true) {
                        // The KV cache now extends past `tokens` with this turn's
                        // generated tokens, so the `cached_prompt` set after
                        // prefill understates it — poison rather than leave a
                        // prefix match that over-trusts the cache.
                        llama.poison_context();
                        return Err(ChatError::InferenceError(format!(
                            "Batch add failed: {}",
                            e
                        )));
                    }
                    if let Err(e) = ctx.decode(&mut batch) {
                        llama.poison_context();
                        return Err(ChatError::BackendDecodeFailed(format!(
                            "Decode failed: {}",
                            e
                        )));
                    }
                    n_cur += 1;
                    continue;
                }
            };

            // Stop on template-defined stop sequences (e.g. Gemma 4's "<end_of_turn>").
            if additional_stops.iter().any(|s| piece.contains(s.as_str())) {
                tracing::debug!("Stop sequence detected in piece: {:?}", piece);
                break;
            }

            if dump_enabled {
                raw_response_accum.push_str(&piece);
            }

            // Feed each token piece to the OAI-compat parser incrementally.
            // update() takes only the new text added since the previous call.
            match oai_parser.update(&piece, true) {
                Ok(deltas) => {
                    oai_parser_errors = 0;
                    for delta_json in deltas {
                        emit_oai_delta(
                            &delta_json,
                            &mut tool_call_ids,
                            &mut channel_splitter,
                            on_chunk,
                        );
                    }
                }
                Err(e) => {
                    oai_parser_errors += 1;
                    tracing::warn!("OAI parser update error ({}/5): {}", oai_parser_errors, e);
                    // After 5 consecutive parse errors the parser is in an unrecoverable
                    // state (e.g. ffi error -3 from a malformed Metal decode mid-stream).
                    // Stop generation cleanly rather than continuing to produce garbage or
                    // letting the model run to the token cap.
                    if oai_parser_errors >= 5 {
                        tracing::error!(
                            "OAI parser unrecoverable after {} consecutive errors — stopping generation",
                            oai_parser_errors
                        );
                        break;
                    }
                }
            }

            // Prepare batch for next token
            batch.clear();
            if let Err(e) = batch.add(new_token, n_cur as i32, &[0], true) {
                // Same rationale as the batch-add failure above: the KV cache
                // extends past `tokens` with this turn's generated tokens, so
                // the `cached_prompt` set after prefill would understate it.
                llama.poison_context();
                return Err(ChatError::InferenceError(format!(
                    "Batch add failed: {}",
                    e
                )));
            }

            if let Err(e) = ctx.decode(&mut batch) {
                llama.poison_context();
                return Err(ChatError::BackendDecodeFailed(format!(
                    "Decode failed: {}",
                    e
                )));
            }

            n_cur += 1;
        }

        // `cached_prompt` was already set to `tokens` right after prefill
        // completed. On context overflow the KV state is indeterminate, so we
        // clear it here to force a full decode on the next call.
        if context_overflowed {
            llama.cached_prompt.clear();
        }

        // Finalize: signal end-of-stream with empty string and is_partial=false.
        // Best-effort — if generation exited via the consecutive-error circuit breaker
        // the parser may be in an unrecoverable state; the warn-and-continue below
        // is intentional (we still want to drain the channel splitter and emit Done).
        match oai_parser.update("", false) {
            Ok(deltas) => {
                for delta_json in deltas {
                    emit_oai_delta(
                        &delta_json,
                        &mut tool_call_ids,
                        &mut channel_splitter,
                        on_chunk,
                    );
                }
            }
            Err(e) => tracing::warn!("OAI parser finalize error: {}", e),
        }

        // Drain any text the splitter held back (a trailing partial marker that
        // never completed, or an unclosed channel block truncated by the token cap).
        let (final_answer, final_reasoning) = channel_splitter.finalize();
        if !final_reasoning.is_empty() {
            on_chunk(ChatChunk::Reasoning(final_reasoning));
        }
        let final_cleaned = strip_gemma_special_tokens(&final_answer);
        if !final_cleaned.is_empty() {
            on_chunk(ChatChunk::Token(final_cleaned));
        }

        on_chunk(ChatChunk::Done);

        prompt_dump::dump_response(dump_seq, &raw_response_accum);

        let usage = ChatUsage {
            prompt_tokens,
            completion_tokens,
        };

        tracing::info!(
            "Generation complete: {} prompt + {} completion tokens",
            prompt_tokens,
            completion_tokens
        );

        Ok(usage)
    }
}

/// Channel markers Gemma 4 uses to wrap its internal chain-of-thought.
const CHANNEL_OPEN: &str = "<|channel>";
const CHANNEL_CLOSE: &str = "<channel|>";

/// Splits a streamed content sequence into answer text and reasoning text.
///
/// Gemma 4 wraps its internal chain-of-thought in `<|channel> … <channel|>`
/// markers. llama.cpp's OAI-compat parser does not separate these — it passes
/// the whole span through as plain content — so we route it ourselves: text
/// between the markers is *reasoning* (surfaced in a dedicated collapsible UI
/// section), text outside is the *answer* (the clean message bubble).
///
/// The splitter is stateful because a single logical span — and even a single
/// marker — may be split across multiple streaming deltas. It tracks whether
/// it is currently inside a channel and carries a tail that could be the
/// beginning of a marker straddling a delta boundary.
#[derive(Debug, Default)]
struct ChannelSplitter {
    /// True while between an opener and its (not-yet-seen) closer.
    in_channel: bool,
    /// Tail of the last delta that may be the prefix of a marker spanning the
    /// boundary into the next delta. Always marker-free text otherwise.
    carry: String,
}

impl ChannelSplitter {
    /// Feed a content delta, returning `(answer, reasoning)` text resolved so far.
    ///
    /// Any trailing bytes that could still grow into a marker are held back in
    /// `carry` and resolved on a later `push` or on `finalize`.
    fn push(&mut self, content: &str) -> (String, String) {
        let mut answer = String::new();
        let mut reasoning = String::new();

        let mut buf = std::mem::take(&mut self.carry);
        buf.push_str(content);

        let mut rest = buf.as_str();
        loop {
            let marker = if self.in_channel {
                CHANNEL_CLOSE
            } else {
                CHANNEL_OPEN
            };
            match rest.find(marker) {
                Some(idx) => {
                    // Emit the text before the marker to the active sink, then
                    // toggle state and continue scanning past the marker.
                    let before = &rest[..idx];
                    if self.in_channel {
                        reasoning.push_str(before);
                    } else {
                        answer.push_str(before);
                    }
                    self.in_channel = !self.in_channel;
                    rest = &rest[idx + marker.len()..];
                }
                None => {
                    // No complete marker remains. Hold back any suffix that
                    // could be the prefix of the marker we're looking for; emit
                    // the rest to the active sink.
                    let keep = partial_marker_suffix_len(rest, marker);
                    let split = rest.len() - keep;
                    let emit = &rest[..split];
                    if self.in_channel {
                        reasoning.push_str(emit);
                    } else {
                        answer.push_str(emit);
                    }
                    self.carry = rest[split..].to_string();
                    break;
                }
            }
        }

        (answer, reasoning)
    }

    /// Flush any carried text once generation is complete.
    ///
    /// No further deltas can complete a marker, so the carry is plain text and
    /// is emitted to whichever sink is currently active.
    fn finalize(&mut self) -> (String, String) {
        let carried = std::mem::take(&mut self.carry);
        if self.in_channel {
            (String::new(), carried)
        } else {
            (carried, String::new())
        }
    }
}

/// Length of the longest suffix of `text` that is a (strict) prefix of `marker`.
///
/// Used to decide how much trailing text to hold back across a delta boundary:
/// e.g. if `text` ends in `"<chan"` and `marker` is `"<channel|>"`, the last 5
/// bytes must be carried in case the rest of the marker arrives next. Marker
/// bytes are ASCII, so a match position is always a UTF-8 char boundary.
fn partial_marker_suffix_len(text: &str, marker: &str) -> usize {
    let max = marker.len().min(text.len());
    // Longest first so we hold back the most that could still match.
    (1..=max)
        .rev()
        .find(|&k| {
            marker
                .as_bytes()
                .starts_with(&text.as_bytes()[text.len() - k..])
        })
        .unwrap_or(0)
}

/// llama.cpp `common_chat_format` value for Gemma 4's PEG-based chat template.
/// Used to detect when Gemma 4-specific stop tokens must be injected.
#[cfg(any(feature = "chat-service", test))]
const CHAT_FORMAT_PEG_GEMMA4: i32 = 3;

#[cfg(any(feature = "chat-service", test))]
/// Augment `additional_stops` with the full set of Gemma 4 turn-end tokens when
/// `chat_format` indicates PEG_GEMMA4.
///
/// The ggml-org Gemma 4 12B GGUF does not mark `<turn|>` as an EOG token in its
/// vocabulary, so llama.cpp's template engine may omit it from `additional_stops`.
/// Injecting the full token set here ensures we stop on both `<end_of_turn>` and
/// `<turn|>` regardless of vocabulary metadata quality.
fn augment_gemma4_stops(chat_format: i32, stops: &mut Vec<String>) {
    if chat_format == CHAT_FORMAT_PEG_GEMMA4 {
        for stop in &["<end_of_turn>", "<turn|>"] {
            if !stops.iter().any(|s| s == stop) {
                stops.push(stop.to_string());
            }
        }
        tracing::debug!(
            "Gemma 4 (PEG_GEMMA4): augmented additional_stops = {:?}",
            stops
        );
    }
}

/// Remove Gemma 4 special-token strings from a content string.
///
/// The PEG parser (`COMMON_CHAT_FORMAT_PEG_GEMMA4`) may emit residual special-token
/// text as content before it identifies a complete tool-call boundary. Gemma 4 uses
/// two special-token patterns:
/// - `<|...|>` — symmetric delimiters e.g. `<|"|>` (quote token)
/// - `<|...>` — asymmetric e.g. `<|tool_call>` (call start marker)
/// - `<...|>` — asymmetric e.g. `<tool_call|>` (call end marker)
///
/// We strip all three by removing any substring that starts with `<|` or ends with `|>`.
fn strip_gemma_special_tokens(s: &str) -> String {
    // Known Gemma 4 special token strings to strip.
    const SPECIAL: &[&str] = &[
        "<|tool_call>",
        "<tool_call|>",
        "<|tool_response>",
        "<|/tool_response>",
        "<|\"|>",
        "<turn|>",
        "<|turn>",
        "<end_of_turn>",
        "<start_of_turn>",
        "<|channel>",
        "<channel|>",
    ];
    let mut out = s.to_string();
    for token in SPECIAL {
        out = out.replace(token, "");
    }
    out
}

/// Merge a tool call's `provider_extra` object into the outgoing tool-call
/// JSON as sibling fields, alongside `id`/`type`/`function`.
///
/// Some providers (e.g. Gemini 3 via its OpenAI-compat endpoint) attach
/// opaque state to a tool call — `thought_signature` — that isn't part of the
/// OpenAI schema but must be echoed back unchanged on replay or the provider
/// rejects the turn. `extra` is `None` for the common case (local models,
/// providers with no such requirement), in which case this is a no-op.
fn merge_provider_extra(tool_call: &mut serde_json::Value, extra: Option<&serde_json::Value>) {
    let Some(serde_json::Value::Object(fields)) = extra else {
        return;
    };
    let Some(obj) = tool_call.as_object_mut() else {
        return;
    };
    for (k, v) in fields {
        obj.insert(k.clone(), v.clone());
    }
}

/// Convert one `ChatMessage` into an OpenAI-format message value for the
/// Jinja chat template.
///
/// Pure and model-independent so it can be unit-tested without a loaded model.
/// Two shapings here are load-bearing for the Gemma 4 template — emitting the
/// wrong shape makes `apply_chat_template` fail with llama.cpp `ffi error -3`,
/// aborting the whole turn:
/// - A tool-call assistant turn carries `content: null` (OpenAI convention)
///   when it has no text, never `""`.
/// - Each tool call's `arguments` must be a valid JSON *object* string; any
///   non-object (empty, malformed) is normalized to `"{}"`.
fn chat_message_to_oai_value(msg: &ChatMessage) -> serde_json::Value {
    if msg.role == Role::Tool {
        // Ministral's Jinja template requires "name" (the function name) on tool-result
        // messages — absent → C++ exception → ffi error -3.
        let mut v = serde_json::json!({
            "role": "tool",
            "tool_call_id": msg.tool_call_id.as_deref().unwrap_or("unknown"),
            "content": msg.content,
        });
        if let Some(name) = msg.name.as_deref().filter(|n| !n.is_empty()) {
            v["name"] = serde_json::Value::String(name.to_string());
        }
        v
    } else if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
        let tool_calls: Vec<serde_json::Value> = msg
            .tool_calls
            .iter()
            .map(|tc| {
                let arguments = match serde_json::from_str::<serde_json::Value>(&tc.arguments_json)
                {
                    Ok(v) if v.is_object() => tc.arguments_json.clone(),
                    _ => "{}".to_string(),
                };
                let mut v = serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.function_name, "arguments": arguments }
                });
                merge_provider_extra(&mut v, tc.provider_extra.as_ref());
                v
            })
            .collect();
        let content: serde_json::Value = if msg.content.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(msg.content.clone())
        };
        serde_json::json!({
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls,
        })
    } else {
        serde_json::json!({
            "role": msg.role.as_str(),
            "content": msg.content,
        })
    }
}

/// Parse an OpenAI-compat streaming delta JSON string and emit the appropriate
/// `ChatChunk` events. Called once per delta returned by `ChatParseStateOaicompat::update`.
///
/// Delta shape (subset we care about):
/// - plain text: `{"role":"assistant","content":"hello"}`
/// - tool call:  `{"role":"assistant","tool_calls":[{"index":0,"id":"...","type":"function",
///                "function":{"name":"search_nodes","arguments":"{\"q\":1}"}}]}`
#[cfg(feature = "chat-service")]
fn emit_oai_delta(
    delta_json: &str,
    tool_call_ids: &mut std::collections::HashMap<u32, String>,
    splitter: &mut ChannelSplitter,
    on_chunk: &(impl Fn(ChatChunk) + Send),
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(delta_json) else {
        tracing::warn!("OAI delta is not valid JSON: {:?}", delta_json);
        return;
    };

    // Plain text content. Route `<|channel> … <channel|>` (Gemma 4) reasoning
    // spans to a separate reasoning stream (the splitter is stateful across
    // deltas; a span or even a single marker may straddle a delta boundary).
    // The answer part still passes through `strip_gemma_special_tokens` so
    // other residual special tokens (e.g. "<|tool_call>", "<|"|>") the PEG
    // parser leaks before recognising a tool-call boundary are scrubbed.
    if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
        let (answer, reasoning) = splitter.push(content);
        if !reasoning.is_empty() {
            on_chunk(ChatChunk::Reasoning(reasoning));
        }
        let cleaned = strip_gemma_special_tokens(&answer);
        if !cleaned.is_empty() {
            on_chunk(ChatChunk::Token(cleaned));
        }
    }

    // Tool calls array
    if let Some(tool_calls) = v.get("tool_calls").and_then(|tc| tc.as_array()) {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            let function = tc.get("function");

            let name = function
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            // Only present when the delta actually carries arguments — absent on
            // name-only deltas (common in Mistral streaming where args arrive later).
            let args: Option<&str> = function
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str());

            // First delta for this index includes the name → emit ToolCallStart
            if !name.is_empty() && !tool_call_ids.contains_key(&index) {
                let id = tc
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("tc_{}", uuid_v4_simple()));
                tool_call_ids.insert(index, id.clone());
                on_chunk(ChatChunk::ToolCallStart {
                    id: id.clone(),
                    name: name.to_string(),
                });
                if let Some(a) = args.filter(|a| !a.is_empty()) {
                    on_chunk(ChatChunk::ToolCallArgs {
                        id,
                        json: a.to_string(),
                    });
                }
            } else if let Some(id) = tool_call_ids.get(&index) {
                // Subsequent arg deltas for the same call
                if let Some(a) = args.filter(|a| !a.is_empty()) {
                    on_chunk(ChatChunk::ToolCallArgs {
                        id: id.clone(),
                        json: a.to_string(),
                    });
                }
            }
        }
    }
}

/// Sample the next token using rejection sampling against an optional grammar.
///
/// Mirrors `common_sampler_sample` (`common/sampling.cpp`) with
/// `grammar_first=false`, upstream's own default: sample once from `chain`
/// with no grammar involved; if a grammar is present and that pick is
/// grammar-invalid, discard it and resample with the grammar applied to the
/// logits ahead of `chain`. When no grammar applies (plain chat, or a tool
/// turn before its lazy trigger fires) this reduces to a single
/// `chain.sample()` call — a turn with `grammar_sampler: None` is completely
/// unaffected.
///
/// Deliberately not "grammar-first" (grammar applied to every token
/// unconditionally): that mode drives llama.cpp's lazy-grammar trigger-replay
/// path (`llama_grammar_accept_impl`) far harder and was observed to hit its
/// `GGML_ASSERT(!stacks.empty())` — a hard, uncatchable process abort — on a
/// live model. Rejection sampling only invokes the grammar when the
/// unconstrained pick already violated it, exactly matching the shape
/// upstream's own CLI/server ships as the default.
#[cfg(feature = "chat-service")]
fn sample_with_grammar_rejection(
    ctx: &LlamaContext,
    idx: i32,
    chain: &mut LlamaSampler,
    grammar_sampler: Option<&mut LlamaSampler>,
) -> llama_cpp_2::token::LlamaToken {
    let Some(grammar_sampler) = grammar_sampler else {
        // `chain.sample()` wraps `llama_sampler_sample`, which already calls
        // `llama_sampler_accept` internally (see `llama-sampler.cpp`) before
        // returning — an explicit `chain.accept()` here would double-apply
        // penalties' ring buffer and frequency count for this token.
        return chain.sample(ctx, idx);
    };

    // First pass: sample without the grammar. `chain`'s own accept already
    // happened inside `sample()` (see above) — only the grammar (which did
    // not see this token yet) needs an explicit accept, and only once the
    // token is confirmed final below.
    let token = chain.sample(ctx, idx);

    // Check validity by applying the grammar to a single-candidate array —
    // mirrors upstream's own check (`common_sampler_sample`): a token the
    // grammar rejects comes back with its logit set to -infinity.
    let mut single = llama_cpp_2::token::data_array::LlamaTokenDataArray::new(
        vec![llama_cpp_2::token::data::LlamaTokenData::new(
            token, 1.0, 0.0,
        )],
        false,
    );
    grammar_sampler.apply(&mut single);
    let is_valid = single.data[0].logit() != f32::NEG_INFINITY;

    if is_valid {
        grammar_sampler.accept(token);
        return token;
    }

    // Resampling: apply the grammar to the full distribution first, then run
    // the base chain on what remains.
    let mut data_array = ctx.token_data_array_ith(idx);
    grammar_sampler.apply(&mut data_array);
    chain.apply(&mut data_array);
    let resampled = data_array
        .selected_token()
        .expect("chain ends in dist/greedy, which always selects a token");

    grammar_sampler.accept(resampled);
    chain.accept(resampled);
    resampled
}

/// Build the grammar-constrained sampler for this turn from the chat template
/// result, if one applies.
///
/// `tmpl_result.grammar` is `None` whenever no tools were offered (llama.cpp's
/// `common_chat.cpp` only populates it inside each format's `has_tools`
/// branch), so a plain-chat turn returns `Ok(None)` and the sampler chain is
/// unchanged.
///
/// When a grammar is present, mirrors llama.cpp's own `common_sampler_init`
/// (`common/sampling.cpp`): `grammar_lazy` selects between an always-on
/// grammar (`LlamaSampler::grammar`) and a lazily-triggered one
/// (`LlamaSampler::grammar_lazy_patterns`) that only engages once one of the
/// template's trigger words/patterns/tokens is seen in the stream — e.g.
/// Mistral only wraps tool calls in `[TOOL_CALLS]`, so unconstrained prose
/// must remain possible until that marker appears.
#[cfg(feature = "chat-service")]
fn build_grammar_sampler(
    model: &LlamaModel,
    tmpl_result: &ChatTemplateResult,
) -> Result<Option<LlamaSampler>> {
    let Some(grammar_str) = tmpl_result.grammar.as_deref().filter(|g| !g.is_empty()) else {
        return Ok(None);
    };

    let sampler = if tmpl_result.grammar_lazy {
        let (trigger_patterns, trigger_tokens) =
            convert_grammar_triggers(&tmpl_result.grammar_triggers);
        LlamaSampler::grammar_lazy_patterns(
            model,
            grammar_str,
            "root",
            &trigger_patterns,
            &trigger_tokens,
        )
    } else {
        LlamaSampler::grammar(model, grammar_str, "root")
    }
    .map_err(|e| ChatError::InferenceError(format!("Grammar sampler init failed: {}", e)))?;

    Ok(Some(sampler))
}

/// Convert llama.cpp's per-template `GrammarTrigger`s into the
/// `(trigger_patterns, trigger_tokens)` shape `LlamaSampler::grammar_lazy_patterns`
/// expects.
///
/// Mirrors `common_sampler_init`'s trigger conversion exactly (see
/// `common/sampling.cpp`): a `Word` trigger is regex-escaped into a plain
/// substring-match pattern (matching llama.cpp's own `regex_escape`), a
/// `Pattern` passes through unanchored, and a `PatternFull` is anchored with
/// `^`/`$` so it must match the whole generated span rather than a substring.
/// `Token` triggers are collected separately — the underlying sampler matches
/// those against sampled token ids rather than the text stream.
#[cfg(any(feature = "chat-service", test))]
fn convert_grammar_triggers(
    triggers: &[GrammarTrigger],
) -> (Vec<String>, Vec<llama_cpp_2::token::LlamaToken>) {
    let mut patterns = Vec::new();
    let mut tokens = Vec::new();

    for trigger in triggers {
        match trigger.trigger_type {
            GrammarTriggerType::Word => patterns.push(regex_escape(&trigger.value)),
            GrammarTriggerType::Pattern => patterns.push(trigger.value.clone()),
            GrammarTriggerType::PatternFull => {
                let pattern = &trigger.value;
                let mut anchored = String::new();
                if !pattern.starts_with('^') {
                    anchored.push('^');
                }
                anchored.push_str(pattern);
                if !pattern.ends_with('$') {
                    anchored.push('$');
                }
                patterns.push(anchored);
            }
            GrammarTriggerType::Token => {
                if let Some(token) = trigger.token {
                    tokens.push(token);
                }
            }
        }
    }

    (patterns, tokens)
}

/// Escape regex metacharacters in `s`, matching llama.cpp's `regex_escape`
/// (`common/common.cpp`): every character in `.^$|()*+?[]{}\` is prefixed with
/// a backslash so a `Word` trigger matches only as a literal substring.
#[cfg(any(feature = "chat-service", test))]
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '^' | '$' | '|' | '(' | ')' | '*' | '+' | '?' | '[' | ']' | '{' | '}' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl ChatEngine {
    /// Apply the model's built-in chat template to the messages.
    ///
    /// Routes through llama.cpp's OAI-compat Jinja machinery (`common_chat_*`),
    /// which handles family-specific prompt and tool formatting natively for
    /// Mistral, Gemma 4, and any other model with an embedded Jinja template.
    /// The simple `apply_chat_template` C API does not work for Gemma 4 — its
    /// chat template requires the full Jinja engine plus llama.cpp's chat
    /// specialization layer.
    ///
    /// Returns the full `ChatTemplateResult` so the caller can initialize
    /// `ChatParseStateOaicompat` for streaming tool-call parsing.
    #[cfg(feature = "chat-service")]
    fn apply_chat_template(
        model: &LlamaModel,
        messages: &[ChatMessage],
        tools: &Option<Vec<ToolSpec>>,
    ) -> Result<ChatTemplateResult> {
        // Build OpenAI-format messages JSON. Tool-result messages carry
        // `tool_call_id`; the Jinja template handles family-specific wrapping
        // (Mistral [TOOL_RESULTS], Gemma 4 turn format, etc.).
        let messages_value: Vec<serde_json::Value> =
            messages.iter().map(chat_message_to_oai_value).collect();

        let messages_json = serde_json::to_string(&messages_value)
            .map_err(|e| ChatError::TemplateError(format!("Message JSON error: {}", e)))?;

        // Build OpenAI tool-spec JSON if tools are provided. The Jinja template
        // formats these per-family (Ministral [AVAILABLE_TOOLS], Gemma 4 <tools>).
        let tools_json_string = if let Some(tool_specs) = tools.as_ref() {
            if tool_specs.is_empty() {
                None
            } else {
                let tools_value: Vec<serde_json::Value> = tool_specs
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters_schema,
                            }
                        })
                    })
                    .collect();
                Some(
                    serde_json::to_string(&tools_value)
                        .map_err(|e| ChatError::TemplateError(format!("Tool JSON error: {}", e)))?,
                )
            }
        } else {
            None
        };

        // Retrieve the model's embedded chat template
        let tmpl = model
            .chat_template(None)
            .map_err(|e| ChatError::TemplateError(format!("No chat template in model: {}", e)))?;

        let params = OpenAIChatTemplateParams {
            messages_json: &messages_json,
            tools_json: tools_json_string.as_deref(),
            tool_choice: None,
            json_schema: None,
            grammar: None,
            reasoning_format: None,
            chat_template_kwargs: None,
            add_generation_prompt: true,
            use_jinja: true,
            parallel_tool_calls: false,
            enable_thinking: false,
            // The Jinja template injects BOS where appropriate; AddBos::Never
            // at tokenization time avoids double-BOS.
            add_bos: false,
            add_eos: false,
            // Enable llama.cpp's native tool-call parsing so ChatTemplateResult
            // carries the chat_format + parser needed for ChatParseStateOaicompat.
            parse_tool_calls: true,
        };

        model
            .apply_chat_template_oaicompat(&tmpl, &params)
            .map_err(|e| ChatError::TemplateError(format!("Failed to apply chat template: {}", e)))
    }

    /// Count the number of tokens in the given text.
    pub fn token_count(&self, text: &str) -> Result<u32> {
        #[cfg(feature = "chat-service")]
        {
            let state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let llama = state_guard.as_ref().ok_or(ChatError::ModelNotLoaded)?;

            let tokens = llama
                .model
                .str_to_token(text, AddBos::Never)
                .map_err(|e| ChatError::TokenizationError(e.to_string()))?;

            Ok(tokens.len() as u32)
        }

        #[cfg(not(feature = "chat-service"))]
        {
            // Rough estimate: ~4 chars per token (common for English)
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    /// Return information about the currently loaded model.
    ///
    /// Returns `None` if no model is loaded.
    pub fn model_info(&self) -> Option<LoadedModelInfo> {
        #[cfg(feature = "chat-service")]
        {
            let state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state_guard.as_ref().map(|s| LoadedModelInfo {
                model_path: s.model_path.clone(),
                context_size: s.context_size,
            })
        }

        #[cfg(not(feature = "chat-service"))]
        {
            None
        }
    }

    /// Release GPU resources held by the chat model.
    ///
    /// After calling this, `generate_streaming` will return `ModelNotLoaded`.
    /// The model can be reloaded with `load_model`.
    pub fn unload_model(&self) {
        #[cfg(feature = "chat-service")]
        {
            let mut state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state_guard.take().is_some() {
                tracing::info!("Chat model unloaded, GPU resources released");
            }
        }
    }

    /// Check if a model is currently loaded.
    pub fn is_loaded(&self) -> bool {
        #[cfg(feature = "chat-service")]
        {
            let state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state_guard.is_some()
        }

        #[cfg(not(feature = "chat-service"))]
        {
            false
        }
    }
}

/// Convert our `KvCacheQuantType` to the llama-cpp-2 `KvCacheType`.
#[cfg(feature = "chat-service")]
fn kv_quant_to_llama(q: crate::chat::types::KvCacheQuantType) -> KvCacheType {
    match q {
        crate::chat::types::KvCacheQuantType::Q8_0 => KvCacheType::Q8_0,
        crate::chat::types::KvCacheQuantType::Q4_0 => KvCacheType::Q4_0,
    }
}

/// Bytes per KV-cache element for a given quantization.
///
/// `None` = F16 (llama.cpp default) at 2 bytes. `Q4_0` is ~0.5 bytes of raw
/// data per element, but its block layout carries scale/overhead; we round up
/// to 1 byte so the memory budget stays a conservative *upper* bound (never
/// under-reserves and OOMs).
#[cfg(any(feature = "chat-service", test))]
fn kv_bytes_per_element(q: Option<crate::chat::types::KvCacheQuantType>) -> u64 {
    match q {
        None => 2,                                             // F16
        Some(crate::chat::types::KvCacheQuantType::Q8_0) => 1, // 8-bit
        Some(crate::chat::types::KvCacheQuantType::Q4_0) => 1, // 4-bit, rounded up
    }
}

/// Compute a memory-fitted `n_ctx` from raw model geometry and a memory budget.
///
/// The KV cache reserves, for the full context window, one key + one value
/// tensor per layer sized by the GQA-reduced embedding width:
///
/// ```text
/// bytes_per_token = 2 (K+V) × n_layer × n_embd_kv × kv_bytes_per_element
/// n_embd_kv       = n_embd × n_head_kv / n_head        (GQA-aware)
/// ```
///
/// The budget is physical RAM minus what is already spoken for: the model
/// weights, [`OS_RESERVE_BYTES`] for the system, and [`COMPUTE_RESERVE_BYTES`]
/// for activation and command buffers. What remains, divided by the per-token
/// cost, gives the token count — capped at `configured` and rounded down to a
/// 256-token multiple for clean batch/KV alignment.
///
/// Budgeting against *total* RAM rather than momentarily-free memory is
/// deliberate. On macOS `available_memory()` subtracts the compressor pool and
/// excludes wired pages, so with a model resident it reports near zero on a
/// machine that comfortably hosts the allocation — measured at 0.1 GB on a
/// 16 GB M2 Pro running a model whose full KV cache is only 3.3 GiB. Total RAM
/// is a hardware constant and does not move with page-cache state, so the same
/// machine yields the same window on every load.
///
/// There is no lower clamp: a result below [`N_CTX_MINIMUM`] means the machine
/// cannot host a usable session, which the caller reports as an error rather
/// than silently allocating a window the agent cannot use.
///
/// Pure arithmetic (no model or memory probe) so it is unit-testable in
/// isolation. Falls back to `configured` on degenerate geometry (`n_head == 0`).
#[cfg(any(feature = "chat-service", test))]
fn fit_n_ctx_to_budget(
    geometry: KvGeometry,
    total_ram_bytes: u64,
    weights_bytes: u64,
    configured: u32,
) -> u32 {
    let bytes_per_token = geometry.bytes_per_token();
    if bytes_per_token == 0 {
        return configured;
    }

    let budget = total_ram_bytes
        .saturating_sub(weights_bytes)
        .saturating_sub(os_reserve_for(total_ram_bytes))
        .saturating_sub(COMPUTE_RESERVE_BYTES);

    // Round down to a 256-token multiple, then cap at the configured ceiling.
    let fit_aligned = ((budget / bytes_per_token) / 256) * 256;
    (fit_aligned.min(configured as u64)) as u32
}

/// The model shape that determines KV-cache cost, plus the per-element width
/// implied by any KV quantization.
#[cfg(any(feature = "chat-service", test))]
#[derive(Debug, Clone, Copy)]
struct KvGeometry {
    n_layer: u32,
    n_embd: u32,
    n_head: u32,
    n_head_kv: u32,
    bytes_per_elem: u64,
}

#[cfg(any(feature = "chat-service", test))]
impl KvGeometry {
    /// KV-cache bytes consumed per token of context, GQA-aware.
    ///
    /// Reserves one key + one value tensor per layer, sized by the GQA-reduced
    /// embedding width. Returns 0 on degenerate geometry, which callers treat as
    /// "cannot size" and fall back to the configured window.
    fn bytes_per_token(self) -> u64 {
        if self.n_head == 0 || self.n_layer == 0 || self.n_embd == 0 {
            return 0;
        }
        let n_embd_kv = (self.n_embd as u64) * (self.n_head_kv as u64) / (self.n_head as u64);
        2 * (self.n_layer as u64) * n_embd_kv * self.bytes_per_elem
    }
}

/// Largest token run submitted to a single `llama_decode`, and the context's
/// `n_batch`. Deliberately decoupled from `n_ctx`: sizing the batch to the full
/// window made llama.cpp allocate a compute buffer proportional to the context,
/// so a 32K window reserved a 32K-token buffer. That allocation — not the KV
/// cache — was the memory sink that forced the KV budget toward zero and floored
/// the context to an unusable size. 2048 is llama.cpp's own default.
#[cfg(any(feature = "chat-service", test))]
const PREFILL_BATCH_TOKENS: u32 = 2048;

/// Physical micro-batch handed to the backend kernels. The compute-buffer size
/// scales with this rather than with `n_batch` or the context window.
/// llama.cpp's default.
#[cfg(any(feature = "chat-service", test))]
const PREFILL_UBATCH_TOKENS: u32 = 512;

/// Absolute position of the first token of prefill chunk `chunk_idx`.
///
/// Chunking walks `tokens[decode_from..]`, but positions handed to the batch
/// must be absolute in `tokens` — the KV cache already holds `0..decode_from`
/// from the reused prefix, and RoPE encodes absolute position. Getting this
/// wrong degrades output silently rather than failing, so it is a named
/// function with its own tests (see `prefill_chunk_positions`).
#[cfg(any(feature = "chat-service", test))]
fn prefill_chunk_start(decode_from: usize, chunk_idx: usize) -> usize {
    decode_from + chunk_idx * PREFILL_BATCH_TOKENS as usize
}

/// The `(position, wants_logits)` pairs prefill emits for a prompt of
/// `token_count` tokens with `decode_from` already cached.
///
/// Mirrors the emission loop exactly so the invariants that loop depends on —
/// contiguous absolute positions, and logits requested on exactly the final
/// token — are checkable without a live model.
#[cfg(test)]
fn prefill_chunk_positions(token_count: usize, decode_from: usize) -> Vec<(usize, bool)> {
    let last_idx = token_count - 1;
    let pending = token_count - decode_from;
    let stride = PREFILL_BATCH_TOKENS as usize;
    let mut out = Vec::with_capacity(pending);

    for chunk_idx in 0..pending.div_ceil(stride) {
        let chunk_start = prefill_chunk_start(decode_from, chunk_idx);
        let chunk_len = stride.min(token_count - chunk_start);
        for i in 0..chunk_len {
            let pos = chunk_start + i;
            out.push((pos, pos == last_idx));
        }
    }
    out
}

/// Physical RAM left for the OS and other applications, on a machine large
/// enough to afford it. macOS pages other processes out under pressure, so this
/// need not cover everything running — it covers the kernel's own footprint
/// plus enough margin to keep the machine responsive.
///
/// Calibrated on Apple Silicon, where unified memory makes total RAM a good
/// proxy for what a GPU allocation can obtain. On platforms with a discrete GPU
/// (where VRAM, not system RAM, is the real ceiling) or a memory manager whose
/// "available" figure is trustworthy, a different reserve shape may fit better;
/// this one is unvalidated there.
#[cfg(any(feature = "chat-service", test))]
const OS_RESERVE_BYTES: u64 = 3 * 1024 * 1024 * 1024; // 3 GiB

/// Ceiling on the OS reserve as a fraction of total RAM, expressed as a
/// divisor. A flat reserve is regressive on small machines — 3 GiB is a modest
/// slice of 32 GB but nearly half of 8 GB, enough to refuse a model the catalog
/// advertises as fitting. Capping the reserve at `total / OS_RESERVE_DIVISOR`
/// leaves behaviour unchanged at 16 GB and above (where the flat value is
/// already the smaller of the two) while degrading proportionally below it.
#[cfg(any(feature = "chat-service", test))]
const OS_RESERVE_DIVISOR: u64 = 4;

/// The OS reserve for a machine of this size: the flat [`OS_RESERVE_BYTES`],
/// capped at a fraction of total RAM so small machines are not starved.
#[cfg(any(feature = "chat-service", test))]
fn os_reserve_for(total_ram_bytes: u64) -> u64 {
    OS_RESERVE_BYTES.min(total_ram_bytes / OS_RESERVE_DIVISOR)
}

/// Reserve for compute/activation buffers and backend command buffers. With
/// `n_batch`/`n_ubatch` capped these no longer scale with the context window,
/// so a fixed constant suffices.
#[cfg(any(feature = "chat-service", test))]
const COMPUTE_RESERVE_BYTES: u64 = 1536 * 1024 * 1024; // 1.5 GiB

/// Smallest context window in which the agent can actually operate. Its
/// tool-registered system prompt alone is ~6,600 tokens; a usable turn also
/// needs the user message, at least one tool result, and room to reply. Below
/// this every tool-using turn fails on context overflow, so the model is
/// refused at load time rather than serving a session that cannot work.
#[cfg(any(feature = "chat-service", test))]
const N_CTX_MINIMUM: u32 = 16_384;

/// Total physical RAM.
///
/// Isolated from the budget arithmetic so the untestable part of the sizing
/// path is a single function and [`fit_n_ctx_to_budget`] stays pure.
#[cfg(feature = "chat-service")]
fn probe_total_memory() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// Compute the effective `n_ctx` for a loaded model, using the configured
/// context window as a ceiling.
///
/// Reads GQA geometry from the loaded model, measures the weights from the GGUF
/// on disk, and delegates the arithmetic to [`fit_n_ctx_to_budget`]. Warns when
/// the result is below the configured ceiling so the reduction is visible in
/// release logs.
///
/// Returns [`ChatError::InsufficientMemory`] when the window that fits cannot
/// hold the agent's system prompt ([`N_CTX_MINIMUM`]). Failing here surfaces one
/// actionable error at load time instead of an opaque context overflow on every
/// subsequent turn.
#[cfg(feature = "chat-service")]
fn compute_effective_n_ctx(
    model: &LlamaModel,
    model_path: &str,
    configured: u32,
    type_k: Option<crate::chat::types::KvCacheQuantType>,
    type_v: Option<crate::chat::types::KvCacheQuantType>,
) -> Result<u32> {
    // K and V may in principle differ; budget on the larger per-element cost.
    let kv_bytes = kv_bytes_per_element(type_k).max(kv_bytes_per_element(type_v));

    let total_ram = probe_total_memory();

    // The GGUF on disk is the weights' resident size. Reading it here rather
    // than taking a catalog figure keeps the sizing correct for user-supplied
    // models that are not in the catalog at all. A stat failure would otherwise
    // budget as though the weights were free, so treat it as unknown and fall
    // back to the configured window (llama.cpp still enforces real limits).
    let weights_bytes = match std::fs::metadata(model_path) {
        Ok(meta) => meta.len(),
        Err(e) => {
            tracing::warn!(
                "Could not measure model weights at {} ({}); using the configured \
                 context window without memory-based sizing",
                model_path,
                e,
            );
            return Ok(configured);
        }
    };

    let geometry = KvGeometry {
        n_layer: model.n_layer(),
        n_embd: model.n_embd() as u32,
        n_head: model.n_head(),
        n_head_kv: model.n_head_kv(),
        bytes_per_elem: kv_bytes,
    };

    let effective = fit_n_ctx_to_budget(geometry, total_ram, weights_bytes, configured);

    if effective < N_CTX_MINIMUM {
        let bytes_per_token = geometry.bytes_per_token();
        let reserved = os_reserve_for(total_ram) + COMPUTE_RESERVE_BYTES;
        let budget = total_ram
            .saturating_sub(weights_bytes)
            .saturating_sub(reserved);
        return Err(ChatError::InsufficientMemory(format!(
            "This model needs more memory than this machine has. Total RAM {:.1} GB, \
             model weights {:.1} GB, leaving {:.1} GB for the context after the {:.1} GB \
             reserved for the operating system and compute buffers. At {} bytes per token \
             of KV cache that fits a {}-token window, below the {} tokens the agent \
             requires for its system prompt and tool results. Choose a smaller model.",
            total_ram as f64 / 1e9,
            weights_bytes as f64 / 1e9,
            budget as f64 / 1e9,
            reserved as f64 / 1e9,
            bytes_per_token,
            effective,
            N_CTX_MINIMUM,
        )));
    }

    if effective < configured {
        tracing::warn!(
            "Reducing chat context to fit memory: configured={} effective={} \
             total_ram={:.1}GB weights={:.1}GB (KV {} B/elem)",
            configured,
            effective,
            total_ram as f64 / 1e9,
            weights_bytes as f64 / 1e9,
            kv_bytes,
        );
    } else {
        tracing::info!(
            "Chat context fits configured window: n_ctx={} total_ram={:.1}GB weights={:.1}GB",
            effective,
            total_ram as f64 / 1e9,
            weights_bytes as f64 / 1e9,
        );
    }

    Ok(effective)
}

/// Generate a simple UUID-like string for tool call IDs.
/// Not cryptographically random — just unique enough for local use.
#[cfg(feature = "chat-service")]
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ChannelSplitter: route <|channel> … <channel|> reasoning out of answer ---

    /// Drive a splitter over a sequence of deltas, returning the fully
    /// accumulated `(answer, reasoning)` including the finalize flush.
    fn split_all(deltas: &[&str]) -> (String, String) {
        let mut s = ChannelSplitter::default();
        let mut answer = String::new();
        let mut reasoning = String::new();
        for d in deltas {
            let (a, r) = s.push(d);
            answer.push_str(&a);
            reasoning.push_str(&r);
        }
        let (a, r) = s.finalize();
        answer.push_str(&a);
        reasoning.push_str(&r);
        (answer, reasoning)
    }

    #[test]
    fn channel_splitter_marker_within_one_delta() {
        let (answer, reasoning) =
            split_all(&["Done.<|channel>I should add the node.<channel|>Added it!"]);
        assert_eq!(answer, "Done.Added it!");
        assert_eq!(reasoning, "I should add the node.");
    }

    // --- Load-time integrity gate ---

    #[cfg(feature = "chat-service")]
    #[test]
    fn load_model_refuses_a_mismatched_digest_before_native_load() {
        use std::io::Write;
        let engine = ChatEngine::new(ChatConfig::default()).expect("engine constructs");
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"tampered / substituted model bytes").unwrap();
        // A wrong expected digest is refused with IntegrityError — this returns
        // before the file is handed to native llama.cpp (no backend/model needed).
        let err = engine
            .load_model(f.path().to_str().unwrap(), Some(&"0".repeat(64)))
            .expect_err("mismatched digest must be refused");
        assert!(
            matches!(err, ChatError::IntegrityError(_)),
            "expected IntegrityError, got {err:?}"
        );
    }

    #[test]
    fn channel_splitter_marker_split_across_deltas() {
        // Both the opener and closer are fragmented across delta boundaries.
        let (answer, reasoning) =
            split_all(&["Hi<|cha", "nnel>think", "ing more<chan", "nel|>the answer"]);
        assert_eq!(answer, "Hithe answer");
        assert_eq!(reasoning, "thinking more");
    }

    #[test]
    fn channel_splitter_opener_without_closer_is_all_reasoning() {
        // Truncated by the token cap mid-thought: everything after the opener
        // is reasoning, and no answer text leaks.
        let (answer, reasoning) = split_all(&["Here.<|channel>reasoning that never closes"]);
        assert_eq!(answer, "Here.");
        assert_eq!(reasoning, "reasoning that never closes");
    }

    #[test]
    fn channel_splitter_clean_content_has_no_reasoning() {
        let (answer, reasoning) = split_all(&["Just a normal ", "answer."]);
        assert_eq!(answer, "Just a normal answer.");
        assert_eq!(reasoning, "");
    }

    #[test]
    fn channel_splitter_trailing_partial_marker_flushed_as_answer() {
        // A suffix that looks like the start of a marker but never completes
        // must be flushed to the answer on finalize, not swallowed.
        let (answer, reasoning) = split_all(&["answer<|chan"]);
        assert_eq!(answer, "answer<|chan");
        assert_eq!(reasoning, "");
    }

    #[test]
    fn channel_splitter_multiple_channels_concatenate() {
        let (answer, reasoning) = split_all(&["a<|channel>r1<channel|>b<|channel>r2<channel|>c"]);
        assert_eq!(answer, "abc");
        assert_eq!(reasoning, "r1r2");
    }

    #[test]
    fn channel_splitter_handles_multibyte_answer_text() {
        // Non-ASCII answer text must not panic the byte-based carry logic.
        let (answer, reasoning) = split_all(&["café ☕<|channel>think<channel|> déjà vu"]);
        assert_eq!(answer, "café ☕ déjà vu");
        assert_eq!(reasoning, "think");
    }

    #[cfg(feature = "chat-service")]
    #[test]
    fn emit_oai_delta_finalize_returns_channel_splitter_carry() {
        // Reproduces the production finalize path: a delta leaves ChannelSplitter
        // holding a carried "<|chan" fragment (a partial channel marker) when
        // generation ends mid-token. finalize() must return it as answer text
        // (no closer ever arrived, so it was never classified as reasoning).
        let delta = serde_json::json!({
            "role": "assistant",
            "content": "answer text<|chan"
        })
        .to_string();

        let chunks = std::sync::Mutex::new(Vec::<ChatChunk>::new());
        let mut ids = std::collections::HashMap::new();
        let mut splitter = ChannelSplitter::default();
        let push_chunk = |chunk: ChatChunk| chunks.lock().unwrap().push(chunk);

        emit_oai_delta(&delta, &mut ids, &mut splitter, &push_chunk);

        // ChannelSplitter holds "<|chan" as an unresolved partial-marker carry;
        // nothing should have been emitted as answer/reasoning for it yet.
        let (final_answer, final_reasoning) = splitter.finalize();

        assert_eq!(
            final_answer, "<|chan",
            "channel_splitter's trailing partial marker must be returned by finalize()"
        );
        assert_eq!(final_reasoning, "");

        let chunks = chunks.into_inner().unwrap();
        let answer_so_far: String = chunks
            .iter()
            .filter_map(|c| match c {
                ChatChunk::Token(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(answer_so_far, "answer text");
    }

    #[test]
    fn test_chat_engine_creation() {
        let config = ChatConfig::default();
        let engine = ChatEngine::new(config);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_chat_engine_not_loaded() {
        let engine = ChatEngine::new(ChatConfig::default()).unwrap();
        assert!(!engine.is_loaded());
        assert!(engine.model_info().is_none());
    }

    #[test]
    fn test_chat_engine_token_count_stub() {
        let engine = ChatEngine::new(ChatConfig::default()).unwrap();
        // Without the chat-service feature, this uses the rough estimator
        #[cfg(not(feature = "chat-service"))]
        {
            let count = engine.token_count("Hello world").unwrap();
            assert!(count > 0);
        }
        // With the feature, it should fail because no model is loaded
        #[cfg(feature = "chat-service")]
        {
            let result = engine.token_count("Hello world");
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_chat_config_validation_error() {
        let config = ChatConfig {
            n_ctx: 0,
            ..Default::default()
        };
        let result = ChatEngine::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_gemma_special_tokens() {
        assert_eq!(strip_gemma_special_tokens("hello"), "hello");
        assert_eq!(strip_gemma_special_tokens("<|tool_call>"), "");
        assert_eq!(strip_gemma_special_tokens("<|\"|>"), "");
        assert_eq!(strip_gemma_special_tokens("<tool_call|>"), "");
        assert_eq!(strip_gemma_special_tokens("<|tool_response>"), "");
        assert_eq!(strip_gemma_special_tokens("<|/tool_response>"), "");
        assert_eq!(
            strip_gemma_special_tokens("hello<|tool_call>world"),
            "helloworld"
        );
        assert_eq!(
            strip_gemma_special_tokens("<|tool_call>call:search<tool_call|>"),
            "call:search"
        );
        // Unknown <|...|>-style tokens are left as-is (only known tokens stripped)
        assert_eq!(
            strip_gemma_special_tokens("text<|unknown|>end"),
            "text<|unknown|>end"
        );
    }

    fn msg(role: Role, content: &str) -> ChatMessage {
        ChatMessage::text(role, content)
    }

    #[test]
    fn oai_value_plain_message_passes_through() {
        let v = chat_message_to_oai_value(&msg(Role::User, "hi"));
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "hi");
        assert!(v.get("tool_calls").is_none());
    }

    #[test]
    fn oai_value_tool_call_turn_empty_content_is_null() {
        // The load-bearing shape: empty content on a tool-call turn must emit
        // `null`, not `""` — else Gemma's template fails (ffi error -3).
        let m = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRaw {
                id: "tc_1".into(),
                function_name: "search_nodes".into(),
                arguments_json: r#"{"query":"x"}"#.into(),
                provider_extra: None,
            }],
        );
        let v = chat_message_to_oai_value(&m);
        assert!(
            v["content"].is_null(),
            "empty content must serialize to null"
        );
        assert_eq!(v["tool_calls"][0]["function"]["name"], "search_nodes");
        assert_eq!(
            v["tool_calls"][0]["function"]["arguments"],
            r#"{"query":"x"}"#
        );
    }

    #[test]
    fn oai_value_tool_call_turn_keeps_real_content() {
        let m = ChatMessage::assistant_with_tool_calls(
            "Let me check.",
            vec![ToolCallRaw {
                id: "tc_1".into(),
                function_name: "get_node".into(),
                arguments_json: r#"{"id":"abc"}"#.into(),
                provider_extra: None,
            }],
        );
        let v = chat_message_to_oai_value(&m);
        assert_eq!(v["content"], "Let me check.");
    }

    #[test]
    fn oai_value_tool_call_turn_merges_provider_extra_fields() {
        // Local llama.cpp models never populate provider_extra today, but
        // this path exists for consistency with the OpenAI-compat replay
        // path -- exercise the actual merge so a colliding key is proven to
        // overwrite (via serde_json::Map::insert) rather than duplicate,
        // unlike the flatten-based approach in openai_compat_inference.rs.
        let m = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRaw {
                id: "tc_1".into(),
                function_name: "search_nodes".into(),
                arguments_json: r#"{"query":"x"}"#.into(),
                provider_extra: Some(serde_json::json!({
                    "extra_content": {"google": {"thought_signature": "tok"}}
                })),
            }],
        );
        let v = chat_message_to_oai_value(&m);
        assert_eq!(
            v["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
            "tok"
        );
        assert_eq!(v["tool_calls"][0]["id"], "tc_1");
    }

    #[test]
    fn oai_value_normalizes_non_object_arguments_to_empty_object() {
        // Empty / malformed / non-object arguments are normalized to "{}" so the
        // template never receives invalid JSON.
        for bad in ["", "not json", "[1,2]", "\"str\"", "42"] {
            let m = ChatMessage::assistant_with_tool_calls(
                "",
                vec![ToolCallRaw {
                    id: "tc_1".into(),
                    function_name: "create_schema".into(),
                    arguments_json: bad.into(),
                    provider_extra: None,
                }],
            );
            let v = chat_message_to_oai_value(&m);
            assert_eq!(
                v["tool_calls"][0]["function"]["arguments"], "{}",
                "non-object arguments {bad:?} should normalize to {{}}"
            );
        }
    }

    #[test]
    fn oai_value_tool_result_carries_call_id() {
        let m = ChatMessage::tool_result("result text", "tc_1", "my_tool");
        let v = chat_message_to_oai_value(&m);
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "tc_1");
        assert_eq!(v["content"], "result text");
        assert_eq!(v["name"], "my_tool");
    }

    #[test]
    fn oai_value_tool_result_includes_name_for_ministral() {
        // Ministral's Jinja template requires "name" on tool-result messages.
        // Missing "name" → Jinja exception → ffi error -3.
        let m = ChatMessage::tool_result("[]", "tc_1", "search_nodes");
        let v = chat_message_to_oai_value(&m);
        assert_eq!(
            v["name"], "search_nodes",
            "tool-result must carry \"name\" for Ministral Jinja template"
        );
        assert_eq!(v["tool_call_id"], "tc_1");
        assert_eq!(v["content"], "[]");
    }

    #[test]
    fn oai_value_multi_turn_tool_calls_have_name_on_every_result() {
        // Regression: ffi error -3 on Ministral 8B after 6+ tool-call round-trips.
        // Each tool-result message must carry "name" regardless of conversation depth.
        let results = [
            ("tc_1", "search_nodes"),
            ("tc_2", "search_nodes"),
            ("tc_3", "search_nodes"),
            ("tc_4", "search_nodes"),
            ("tc_5", "search_nodes"),
            ("tc_6", "get_node"),
            ("tc_7", "search_nodes"),
        ];
        for (i, (call_id, tool_name)) in results.iter().enumerate() {
            let msg = ChatMessage::tool_result(format!("r{}", i + 1), *call_id, *tool_name);
            let v = chat_message_to_oai_value(&msg);
            assert_eq!(
                v["name"],
                *tool_name,
                "tool-result #{} missing \"name\" field",
                i + 1
            );
        }
    }

    // -----------------------------------------------------------------------
    // Regression fixture — Gemma 4 12B tool-call delta
    //
    // With the old vendored engine (pre-b8660) chat_format=0 caused tool calls
    // to be emitted as plain content, never reaching emit_oai_delta as
    // tool_calls arrays. With the upgraded engine (>=v0.1.146) chat_format=3
    // (PEG_GEMMA4) routes through ChatParseStateOaicompat and produces the
    // delta shape below. This test calls emit_oai_delta directly with that
    // shape and verifies ToolCallStart + ToolCallArgs events are produced.
    // -----------------------------------------------------------------------

    #[cfg(feature = "chat-service")]
    #[test]
    fn regression_gemma4_12b_oai_delta_emits_tool_call_chunks() {
        // OAI-compat delta the upgraded engine produces for a Gemma 4 create_schema call.
        // With chat_format=0 this `tool_calls` key would never appear in the stream.
        let delta = r#"{"role":"assistant","tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"create_schema","arguments":"{\"name\":\"Invoice\",\"fields\":[]}"}}]}"#;

        // emit_oai_delta takes `impl Fn(ChatChunk) + Send` so we collect via Mutex.
        let chunks = std::sync::Mutex::new(Vec::<ChatChunk>::new());
        let mut ids = std::collections::HashMap::new();
        let mut splitter = ChannelSplitter::default();
        emit_oai_delta(delta, &mut ids, &mut splitter, &|chunk| {
            chunks.lock().unwrap().push(chunk);
        });
        let chunks = chunks.into_inner().unwrap();

        let start = chunks.iter().find(
            |c| matches!(c, ChatChunk::ToolCallStart { name, .. } if name == "create_schema"),
        );
        assert!(
            start.is_some(),
            "emit_oai_delta must emit ToolCallStart for create_schema; got: {:?}",
            chunks
        );

        let args = chunks.iter().find(
            |c| matches!(c, ChatChunk::ToolCallArgs { json, .. } if json.contains("Invoice")),
        );
        assert!(
            args.is_some(),
            "emit_oai_delta must emit ToolCallArgs containing 'Invoice'; got: {:?}",
            chunks
        );
    }

    // -----------------------------------------------------------------------
    // augment_gemma4_stops — Gemma 4 runaway fix
    //
    // The ggml-org GGUF does not mark <turn|> as EOG, so llama.cpp's template
    // engine may omit it from additional_stops. augment_gemma4_stops injects
    // the full token set when chat_format=3 (PEG_GEMMA4).
    // -----------------------------------------------------------------------

    #[test]
    fn augment_gemma4_injects_missing_turn_tokens() {
        // Neither token present — both must be added.
        let mut stops: Vec<String> = vec![];
        augment_gemma4_stops(CHAT_FORMAT_PEG_GEMMA4, &mut stops);
        assert!(
            stops.iter().any(|s| s == "<end_of_turn>"),
            "must inject <end_of_turn>"
        );
        assert!(stops.iter().any(|s| s == "<turn|>"), "must inject <turn|>");
    }

    #[test]
    fn augment_gemma4_does_not_duplicate_existing_stops() {
        // Template already included <end_of_turn> at index 0 — must not be duplicated
        // and its position must be preserved.
        let mut stops = vec!["<end_of_turn>".to_string()];
        augment_gemma4_stops(CHAT_FORMAT_PEG_GEMMA4, &mut stops);
        let eot_count = stops
            .iter()
            .filter(|s| s.as_str() == "<end_of_turn>")
            .count();
        assert_eq!(eot_count, 1, "<end_of_turn> must not be duplicated");
        assert_eq!(
            stops[0], "<end_of_turn>",
            "pre-existing entry must stay at its original index"
        );
        assert!(
            stops.iter().any(|s| s == "<turn|>"),
            "<turn|> must still be added"
        );
    }

    #[test]
    fn augment_gemma4_does_not_modify_non_gemma4_format() {
        // chat_format=0 (CONTENT_ONLY) or 1 (Mistral) — must leave stops unchanged.
        let mut stops: Vec<String> = vec![];
        augment_gemma4_stops(0, &mut stops);
        assert!(stops.is_empty(), "must not add stops for non-Gemma4 format");
        augment_gemma4_stops(1, &mut stops);
        assert!(stops.is_empty(), "must not add stops for non-Gemma4 format");
    }

    // --- Memory-aware n_ctx sizing (fit_n_ctx_to_budget) ---

    use crate::chat::types::KvCacheQuantType;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn kv_bytes_per_element_maps_quant_to_upper_bound() {
        assert_eq!(kv_bytes_per_element(None), 2); // F16
        assert_eq!(kv_bytes_per_element(Some(KvCacheQuantType::Q8_0)), 1);
        // Q4_0 rounds up to 1 so the budget stays a conservative upper bound.
        assert_eq!(kv_bytes_per_element(Some(KvCacheQuantType::Q4_0)), 1);
    }

    #[test]
    fn kv_bytes_per_token_is_gqa_aware() {
        // Gemma 4 12B (48 layers, n_embd 3840, 16 heads, 8 KV heads, Q8_0):
        // n_embd_kv = 3840 * 8 / 16 = 1920; 2 * 48 * 1920 * 1 = 184_320.
        assert_eq!(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 16,
                n_head_kv: 8,
                bytes_per_elem: 1
            }
            .bytes_per_token(),
            184_320
        );
        // With no GQA reduction (n_head == n_head_kv) the full width is used.
        assert_eq!(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 16,
                n_head_kv: 16,
                bytes_per_elem: 1
            }
            .bytes_per_token(),
            2 * 48 * 3840
        );
        // Degenerate geometry cannot divide; reports zero cost.
        assert_eq!(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 0,
                n_head_kv: 8,
                bytes_per_elem: 1
            }
            .bytes_per_token(),
            0
        );
    }

    // Measured constants for the scenarios below. `TOTAL_16GB` is what sysinfo
    // reports on a 16GB M2 Pro (17.18 GB); the weight sizes are the catalog
    // `size_bytes` for the corresponding GGUF.
    const TOTAL_16GB: u64 = 17_179_869_184;
    const E4B_WEIGHTS: u64 = 5_335_289_824;
    const G12B_WEIGHTS: u64 = 7_400_000_000;

    #[test]
    fn fit_gives_e4b_full_window_on_16gb() {
        // Regression test for the reported bug, using the real geometry read
        // from the gemma-4-E4B-it-Q4_K_M GGUF: 42 layers, n_embd 2560, GQA 8/2,
        // F16 KV — exactly 105 KiB per token. The previous budget, keyed off
        // momentarily-available memory, drove this to a 4096-token floor that
        // sat below the agent's own system prompt and failed every tool turn.
        let kv_bytes = kv_bytes_per_element(None); // F16
        assert_eq!(
            KvGeometry {
                n_layer: 42,
                n_embd: 2560,
                n_head: 8,
                n_head_kv: 2,
                bytes_per_elem: kv_bytes
            }
            .bytes_per_token(),
            105 * 1024,
            "E4B per-token KV cost"
        );

        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 42,
                n_embd: 2560,
                n_head: 8,
                n_head_kv: 2,
                bytes_per_elem: kv_bytes,
            },
            TOTAL_16GB,
            E4B_WEIGHTS,
            32_768,
        );
        assert_eq!(n, 32_768, "E4B must reach its full window on 16GB, got {n}");
        assert!(n > N_CTX_MINIMUM);
    }

    #[test]
    fn fit_throttles_12b_below_known_oom_window() {
        // Gemma 4 12B (48 layers, n_embd 3840, GQA 16/8, Q8_0) on the same 16GB
        // machine. A 27,136-token window is the one empirical OOM datum we have,
        // so the computed size must stay below it, and the KV cache plus the
        // fixed reserves must fit within total RAM alongside the weights.
        let kv_bytes = kv_bytes_per_element(Some(KvCacheQuantType::Q8_0));
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 16,
                n_head_kv: 8,
                bytes_per_elem: kv_bytes,
            },
            TOTAL_16GB,
            G12B_WEIGHTS,
            32_768,
        );

        assert!(
            n < 27_136,
            "must be more conservative than the OOMing 27K window, got {n}"
        );
        assert_eq!(n % 256, 0, "must align to 256 tokens, got {n}");

        let kv_cache_bytes = KvGeometry {
            n_layer: 48,
            n_embd: 3840,
            n_head: 16,
            n_head_kv: 8,
            bytes_per_elem: kv_bytes,
        }
        .bytes_per_token()
            * n as u64;
        assert!(
            G12B_WEIGHTS + kv_cache_bytes + OS_RESERVE_BYTES + COMPUTE_RESERVE_BYTES <= TOTAL_16GB,
            "weights + KV ({kv_cache_bytes}) + reserves must fit in total RAM; n={n}"
        );
    }

    #[test]
    fn fit_returns_below_minimum_when_weights_exceed_ram() {
        // Gemma 4 31B (~18.7GB of weights) selected on a 16GB machine: the
        // weights alone exceed total RAM, so the budget saturates to zero. The
        // caller turns this into a load-time error rather than allocating a
        // context the agent cannot use — and it must not panic on the way.
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 48,
                n_embd: 5376,
                n_head: 32,
                n_head_kv: 8,
                bytes_per_elem: kv_bytes_per_element(Some(KvCacheQuantType::Q8_0)),
            },
            TOTAL_16GB,
            18_687_061_792,
            32_768,
        );
        assert!(
            n < N_CTX_MINIMUM,
            "a model larger than RAM must not yield a usable window, got {n}"
        );
    }

    #[test]
    fn fit_keeps_full_window_for_small_model_with_headroom() {
        // Small model on a large machine: the full configured 32K comfortably
        // fits, so no reduction (regression guard).
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 30,
                n_embd: 2048,
                n_head: 16,
                n_head_kv: 8,
                bytes_per_elem: kv_bytes_per_element(None),
            },
            64 * GIB,
            4 * GIB,
            32_768,
        );
        assert_eq!(n, 32_768, "small model with headroom keeps full window");
    }

    #[test]
    fn fit_falls_back_to_configured_on_degenerate_geometry() {
        // n_head == 0 would divide by zero; must fall back to the configured value.
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 0,
                n_head_kv: 8,
                bytes_per_elem: 1,
            },
            TOTAL_16GB,
            E4B_WEIGHTS,
            32_768,
        );
        assert_eq!(n, 32_768, "n_head==0 must fall back to configured");
        // n_layer == 0 likewise.
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 0,
                n_embd: 3840,
                n_head: 24,
                n_head_kv: 8,
                bytes_per_elem: 1,
            },
            TOTAL_16GB,
            E4B_WEIGHTS,
            32_768,
        );
        assert_eq!(n, 32_768, "n_layer==0 must fall back to configured");
    }

    #[test]
    fn fit_never_exceeds_configured_and_stays_aligned() {
        // Across the whole plausible RAM range the result must respect the
        // configured ceiling and the 256-token alignment.
        let kv_bytes = kv_bytes_per_element(None);
        for gib in 1..=64u64 {
            let n = fit_n_ctx_to_budget(
                KvGeometry {
                    n_layer: 42,
                    n_embd: 2560,
                    n_head: 8,
                    n_head_kv: 2,
                    bytes_per_elem: kv_bytes,
                },
                gib * GIB,
                E4B_WEIGHTS,
                32_768,
            );
            assert!(n <= 32_768, "exceeded configured at {gib}GiB: {n}");
            assert_eq!(n % 256, 0, "misaligned at {gib}GiB: {n}");
        }
    }

    #[test]
    fn fit_is_monotonic_in_total_memory() {
        // More RAM must never produce a smaller window — catches sign and
        // saturation errors in the reserve arithmetic.
        let kv_bytes = kv_bytes_per_element(Some(KvCacheQuantType::Q8_0));
        let mut prev = 0u32;
        for gib in 1..=64u64 {
            let n = fit_n_ctx_to_budget(
                KvGeometry {
                    n_layer: 48,
                    n_embd: 3840,
                    n_head: 16,
                    n_head_kv: 8,
                    bytes_per_elem: kv_bytes,
                },
                gib * GIB,
                G12B_WEIGHTS,
                32_768,
            );
            assert!(n >= prev, "window shrank from {prev} to {n} at {gib}GiB");
            prev = n;
        }
    }

    #[test]
    fn os_reserve_is_capped_on_small_machines_and_flat_above() {
        // At 16GB and up the flat reserve is already the smaller value, so the
        // cap changes nothing — the measured behaviour there must not move.
        assert_eq!(os_reserve_for(16 * GIB), OS_RESERVE_BYTES);
        assert_eq!(os_reserve_for(32 * GIB), OS_RESERVE_BYTES);
        // Below that the flat 3GiB would take a punitive share, so it scales.
        assert_eq!(os_reserve_for(8 * GIB), 2 * GIB);
        assert_eq!(os_reserve_for(4 * GIB), GIB);
    }

    #[test]
    fn fit_loads_the_8gb_tier_model_on_an_8gb_machine() {
        // Ministral 3B is catalogued at min_memory_gb: 8 (32 layers, n_embd
        // 4096, GQA 32/8, F16 → 128 KiB/token, ~2.1GB weights). A flat 3GiB OS
        // reserve consumed enough of an 8GB machine to push it under
        // N_CTX_MINIMUM, making a model the catalog advertises as fitting
        // refuse to load. With the reserve capped proportionally it gets a
        // usable window.
        let n = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 32,
                n_embd: 4096,
                n_head: 32,
                n_head_kv: 8,
                bytes_per_elem: kv_bytes_per_element(None),
            },
            8 * GIB,
            2_147_023_008,
            32_768,
        );
        assert!(
            n >= N_CTX_MINIMUM,
            "the 8GB-tier model must load on an 8GB machine, got {n}"
        );
    }

    #[test]
    fn fit_is_unchanged_at_16gb_by_the_proportional_reserve() {
        // Guard that capping the reserve did not perturb the machine class the
        // constants were actually measured on: E4B keeps its full window and
        // 12B stays below the 27,136 that OOM'd.
        let e4b = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 42,
                n_embd: 2560,
                n_head: 8,
                n_head_kv: 2,
                bytes_per_elem: kv_bytes_per_element(None),
            },
            TOTAL_16GB,
            E4B_WEIGHTS,
            32_768,
        );
        assert_eq!(e4b, 32_768);

        let g12 = fit_n_ctx_to_budget(
            KvGeometry {
                n_layer: 48,
                n_embd: 3840,
                n_head: 16,
                n_head_kv: 8,
                bytes_per_elem: kv_bytes_per_element(Some(KvCacheQuantType::Q8_0)),
            },
            TOTAL_16GB,
            G12B_WEIGHTS,
            32_768,
        );
        assert!(
            g12 < 27_136,
            "12B must stay under the OOM window, got {g12}"
        );
    }

    #[test]
    fn prefill_chunking_emits_contiguous_absolute_positions() {
        // The chunk walk must cover exactly `decode_from..token_count`, in
        // order, with no gap or repeat at a chunk boundary. A position error
        // here corrupts RoPE and degrades output silently rather than failing,
        // so the boundary cases are enumerated explicitly: exact multiples of
        // the stride, one either side of it, and a cached prefix that is itself
        // a multiple.
        let stride = PREFILL_BATCH_TOKENS as usize;
        let cases = [
            (10_787, 0),     // the measured multi-chunk prompt
            (10_787, 4_096), // same, with a reused prefix
            (stride, 0),     // exactly one full chunk
            (stride + 1, 0), // one token into a second chunk
            (stride - 1, 0), // just under a single chunk
            (2 * stride, 0), // exact multiple, no remainder
            (4_097, stride), // prefix reuse landing on a stride boundary
            (32_768, 0),     // the full configured window
            (6_000, 5_999),  // single trailing token
        ];

        for (token_count, decode_from) in cases {
            let emitted = prefill_chunk_positions(token_count, decode_from);
            let positions: Vec<usize> = emitted.iter().map(|(p, _)| *p).collect();
            let expected: Vec<usize> = (decode_from..token_count).collect();
            assert_eq!(
                positions, expected,
                "positions must be contiguous and absolute for \
                 token_count={token_count} decode_from={decode_from}"
            );

            let with_logits: Vec<usize> = emitted
                .iter()
                .filter(|(_, l)| *l)
                .map(|(p, _)| *p)
                .collect();
            assert_eq!(
                with_logits,
                vec![token_count - 1],
                "logits must be requested on exactly the final token for \
                 token_count={token_count} decode_from={decode_from}"
            );
        }
    }

    #[test]
    fn prefill_final_token_lands_in_last_chunk() {
        // Sampling reads `batch.n_tokens() - 1` of the LAST decoded batch, so
        // the prompt's final token must be the final entry of the final chunk.
        // This is what makes that index the right one to sample from.
        let stride = PREFILL_BATCH_TOKENS as usize;
        for (token_count, decode_from) in
            [(10_787, 0), (stride, 0), (2 * stride, 0), (4_097, stride)]
        {
            let emitted = prefill_chunk_positions(token_count, decode_from);
            let (last_pos, last_wants_logits) = *emitted.last().expect("chunks are non-empty");
            assert_eq!(last_pos, token_count - 1, "final emitted token is the last");
            assert!(last_wants_logits, "final emitted token carries the logits");
        }
    }

    #[test]
    fn n_ctx_minimum_exceeds_agent_system_prompt() {
        // The agent's system prompt measures ~6,600 tokens with its full tool
        // set registered. A window below that fails every tool-using turn, which
        // is exactly the regression this constant guards against — the previous
        // 4096 floor sat under it.
        const {
            assert!(
                N_CTX_MINIMUM > 6_600,
                "minimum window must hold the agent system prompt"
            );
            assert!(
                N_CTX_MINIMUM.is_multiple_of(256),
                "minimum must be 256-aligned"
            );
        }
    }

    // --- Grammar trigger conversion: mirrors llama.cpp's common_sampler_init ---

    #[test]
    fn regex_escape_escapes_all_metacharacters() {
        // Every character regex_escape (common/common.cpp) treats as special
        // must come back backslash-prefixed so a Word trigger only ever
        // matches its literal text.
        assert_eq!(regex_escape("[TOOL_CALLS]"), r"\[TOOL_CALLS\]");
        // `<` and `>` are not in llama.cpp's special-char set — only `|` is escaped.
        assert_eq!(regex_escape("<|tool_call>"), r"<\|tool_call>");
        assert_eq!(regex_escape("plain_word"), "plain_word");
        assert_eq!(regex_escape("a.b*c?"), r"a\.b\*c\?");
    }

    fn trigger(trigger_type: GrammarTriggerType, value: &str) -> GrammarTrigger {
        GrammarTrigger {
            trigger_type,
            value: value.to_string(),
            token: None,
        }
    }

    #[test]
    fn word_trigger_becomes_escaped_pattern() {
        // Mistral's tool-call marker is a Word trigger; it must survive as an
        // escaped literal-match pattern, not a raw (and here, invalid-looking)
        // regex fragment.
        let (patterns, tokens) =
            convert_grammar_triggers(&[trigger(GrammarTriggerType::Word, "[TOOL_CALLS]")]);
        assert_eq!(patterns, vec![r"\[TOOL_CALLS\]".to_string()]);
        assert!(tokens.is_empty());
    }

    #[test]
    fn pattern_trigger_passes_through_unanchored() {
        let (patterns, _tokens) =
            convert_grammar_triggers(&[trigger(GrammarTriggerType::Pattern, "^\\s+to$")]);
        assert_eq!(patterns, vec!["^\\s+to$".to_string()]);
    }

    #[test]
    fn pattern_full_trigger_is_anchored_both_ends() {
        let (patterns, _tokens) =
            convert_grammar_triggers(&[trigger(GrammarTriggerType::PatternFull, "abc")]);
        assert_eq!(patterns, vec!["^abc$".to_string()]);
    }

    #[test]
    fn pattern_full_trigger_does_not_double_anchor() {
        // A PatternFull value that already carries an anchor must not gain a
        // second one (matching common_sampler_init's own guard).
        let (patterns, _tokens) =
            convert_grammar_triggers(&[trigger(GrammarTriggerType::PatternFull, "^already$")]);
        assert_eq!(patterns, vec!["^already$".to_string()]);
    }

    #[test]
    fn token_trigger_is_collected_separately_from_patterns() {
        let mut t = trigger(GrammarTriggerType::Token, "");
        t.token = Some(llama_cpp_2::token::LlamaToken(42));
        let (patterns, tokens) = convert_grammar_triggers(&[t]);
        assert!(patterns.is_empty());
        assert_eq!(tokens, vec![llama_cpp_2::token::LlamaToken(42)]);
    }

    #[test]
    fn mixed_triggers_split_correctly() {
        let mut token_trigger = trigger(GrammarTriggerType::Token, "");
        token_trigger.token = Some(llama_cpp_2::token::LlamaToken(7));

        let (patterns, tokens) = convert_grammar_triggers(&[
            trigger(GrammarTriggerType::Word, "<|tool_call>"),
            trigger(GrammarTriggerType::Pattern, ">>>(?!all)"),
            token_trigger,
        ]);
        assert_eq!(
            patterns,
            vec![r"<\|tool_call>".to_string(), ">>>(?!all)".to_string()]
        );
        assert_eq!(tokens, vec![llama_cpp_2::token::LlamaToken(7)]);
    }
}
