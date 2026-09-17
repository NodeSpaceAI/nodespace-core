//! Shared types, traits, and interface contracts for agent subsystems.
//!
//! This module defines the foundational type definitions, trait interfaces,
//! and message formats that all agent-related subsystems code against. It
//! produces no runtime behavior -- only type definitions, trait declarations,
//! and module scaffolding.
//!
//! Tauri event channel constants live in the desktop-app crate (they depend
//! on Tauri, which is not a dependency of this crate).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Re-export the canonical chat message types from nlp-engine (single source of truth).
pub use nodespace_nlp_engine::{ChatMessage, Role, ToolCallRaw};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by [`ChatInferenceEngine`] methods.
#[derive(Debug, Error)]
pub enum InferenceError {
    /// No model is currently loaded.
    #[error("no model loaded")]
    NoModelLoaded,

    /// The model ran out of context window space.
    #[error("context window exceeded: {0}")]
    ContextOverflow(String),

    /// An internal engine error occurred.
    #[error("inference engine error: {0}")]
    Engine(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors returned by [`ModelManager`] methods.
#[derive(Debug, Error)]
pub enum ModelError {
    /// The requested model ID does not exist in the catalog.
    #[error("model not found: {0}")]
    NotFound(String),

    /// A download was already in progress for this model.
    #[error("download already in progress for model: {0}")]
    DownloadInProgress(String),

    /// Network or I/O failure during download.
    #[error("download failed: {0}")]
    DownloadFailed(String),

    /// Verification (SHA-256 checksum) failed after download.
    #[error("verification failed for model: {0}")]
    VerificationFailed(String),

    /// The model file could not be loaded into memory.
    #[error("failed to load model: {0}")]
    LoadFailed(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors returned by [`AgentToolExecutor`] methods.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The requested tool name is not registered.
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    /// The tool received invalid arguments.
    #[error("invalid arguments for tool {tool}: {reason}")]
    InvalidArguments {
        /// Name of the tool.
        tool: String,
        /// Explanation of what was wrong.
        reason: String,
    },

    /// The tool execution itself failed.
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors returned when writing a context file for a PTY agent session (ADR-032).
#[derive(Debug, Error)]
pub enum ContextError {
    /// The requested node could not be found.
    #[error("node not found: {0}")]
    NodeNotFound(String),

    /// Writing the context file to disk failed.
    #[error("context file write failed: {0}")]
    WriteFailed(#[from] std::io::Error),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// A single chunk emitted during streaming inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingChunk {
    /// A token of generated text (answer content shown to the user).
    Token {
        /// The text content of this token.
        text: String,
    },
    /// A span of the model's internal reasoning (chain-of-thought).
    ///
    /// Captured separately from the answer so it can be surfaced in a dedicated
    /// collapsible UI section rather than inline. Not forwarded to the live UI
    /// overlay; it is accumulated into the final assistant message.
    Reasoning {
        /// The reasoning text content of this span.
        text: String,
    },
    /// The model is starting a tool call.
    ToolCallStart {
        /// Unique identifier for this tool call.
        id: String,
        /// Name of the tool being invoked.
        name: String,
        /// Provider-opaque data (e.g. Gemini 3's `thought_signature`) that
        /// must be echoed back verbatim if this call is replayed as history.
        /// `None` for providers/models that attach nothing extra.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_extra: Option<serde_json::Value>,
    },
    /// Incremental arguments JSON for an in-progress tool call.
    ToolCallArgs {
        /// Identifier matching the corresponding `ToolCallStart`.
        id: String,
        /// Partial JSON string of tool arguments.
        args_json: String,
    },
    /// Inference is complete.
    Done {
        /// Token usage statistics for the completed turn.
        usage: InferenceUsage,
    },
    /// An error occurred during streaming.
    Error {
        /// Human-readable error description.
        message: String,
    },
}

/// Current status of a model in the local catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Model is known but not yet downloaded.
    NotDownloaded,
    /// Model is currently being downloaded.
    Downloading {
        /// Download progress as a percentage (0.0 -- 100.0).
        progress_pct: f32,
        /// Bytes downloaded so far.
        bytes_downloaded: u64,
        /// Total bytes to download.
        bytes_total: u64,
    },
    /// Download complete, verifying integrity (SHA-256).
    Verifying,
    /// Model is on disk and ready to be loaded.
    Ready,
    /// Model is loaded into memory and available for inference.
    Loaded,
    /// An error occurred (download, verification, or loading).
    Error {
        /// Human-readable error description.
        message: String,
    },
}

/// External agent CLI catalogued for PTY spawning (ADR-032).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentType {
    ClaudeCode,
    Codex,
    AntigravityCli,
    Pi,
    OpenCode,
}

/// Context file convention an external agent expects on launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextFile {
    /// Claude Code reads `CLAUDE.md` from its working directory.
    ClaudeMd,
    /// All other supported agents read `AGENTS.md`.
    AgentsMd,
}

impl ContextFile {
    /// Filename written to the session directory.
    pub fn filename(self) -> &'static str {
        match self {
            ContextFile::ClaudeMd => "CLAUDE.md",
            ContextFile::AgentsMd => "AGENTS.md",
        }
    }
}

/// Current status of the local agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LocalAgentStatus {
    /// Agent is idle, waiting for user input.
    Idle,
    /// Agent is processing a request (pre-generation).
    Thinking,
    /// Agent is executing a tool.
    ToolExecution {
        /// Name of the tool currently being executed.
        tool_name: String,
    },
    /// Agent is streaming a response to the user.
    Streaming,
    /// Agent encountered an error.
    Error {
        /// Human-readable error description.
        message: String,
    },
}

/// Family of language models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFamily {
    /// Ministral -- Mistral AI's small model series (Ministral 3B, Ministral 8B).
    Ministral,
    /// Gemma 4 -- Google's multimodal model series (E4B, 31B).
    Gemma4,
    /// MistralSmall -- Mistral AI's Small series (24B dense, strong reasoning).
    MistralSmall,
    /// Model served via a user-configured OpenAI-compatible endpoint (family
    /// determined by the remote server).
    OpenAiCompat,
}

/// Backend used to serve a language model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ModelBackend {
    /// Local GGUF model loaded via llama.cpp.
    #[default]
    Gguf,
    /// Model served by a user-configured OpenAI-compatible endpoint. Covers
    /// every remotely-served model, Ollama's `/v1` API included.
    OpenAiCompat,
}

impl ModelBackend {
    /// Wire identifier sent to the frontend in the model catalog.
    ///
    /// Spelled out rather than derived from `Debug`, which would render the
    /// variant as "openaicompat" and silently disagree with the hyphenated
    /// value the frontend matches on.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ModelBackend::Gguf => "gguf",
            ModelBackend::OpenAiCompat => "openai-compat",
        }
    }
}

// ---------------------------------------------------------------------------
// Structs -- Chat & Inference
// ---------------------------------------------------------------------------

/// Parameters for an inference request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    /// Ordered list of chat messages forming the conversation.
    pub messages: Vec<ChatMessage>,
    /// Tool definitions available for the model to invoke.
    pub tools: Option<Vec<ToolDefinition>>,
    /// Sampling temperature (0.0 = deterministic, higher = more creative).
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate.
    pub max_tokens: Option<u32>,
}

/// Token usage statistics for a completed inference turn.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct InferenceUsage {
    /// Number of tokens in the input prompt.
    pub prompt_tokens: u32,
    /// Number of tokens generated by the model.
    pub completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// Structs -- Tools
// ---------------------------------------------------------------------------

/// Definition of a tool that the model can invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique name of the tool (e.g. "search_nodes").
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters_schema: serde_json::Value,
}

/// Result of a single tool invocation, returned to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the tool call this result corresponds to.
    pub tool_call_id: String,
    /// Name of the tool that was executed.
    pub name: String,
    /// The output produced by the tool.
    pub result: serde_json::Value,
    /// Whether the tool execution itself failed.
    pub is_error: bool,
}

/// Complete record of a tool execution for session history / debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    /// ID of the tool call.
    pub tool_call_id: String,
    /// Name of the tool.
    pub name: String,
    /// Arguments passed to the tool.
    pub args: serde_json::Value,
    /// Output produced by the tool.
    pub result: serde_json::Value,
    /// Whether the tool execution failed.
    pub is_error: bool,
    /// Wall-clock duration of execution in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Structs -- Model Management
// ---------------------------------------------------------------------------

/// Metadata about a language model in the local catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Unique identifier for this model.
    pub id: String,
    /// Model family (e.g. Ministral).
    pub family: ModelFamily,
    /// Human-readable model name.
    pub name: String,
    /// Filename of the model weights on disk.
    pub filename: Option<String>,
    /// Size of the model file in bytes.
    pub size_bytes: u64,
    /// Quantization format (e.g. "Q4_K_M").
    pub quantization: String,
    /// URL to download the model weights.
    pub url: Option<String>,
    /// Expected SHA-256 hash of the model file.
    pub sha256: Option<String>,
    /// Backend used to serve this model.
    #[serde(default)]
    pub backend: ModelBackend,
    /// Current download / load status.
    pub status: ModelStatus,
    /// Minimum system RAM (in GiB) required to run this model comfortably.
    /// Zero means unknown (e.g. for remotely-served models).
    pub min_memory_gb: u8,
}

/// Specification of a chat model's capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatModelSpec {
    /// Identifier of the model this spec describes.
    pub model_id: String,
    /// Family the model belongs to (drives any per-family behavior, e.g. UI
    /// hardware-requirement labels).
    pub family: ModelFamily,
    /// Maximum number of tokens the model can process.
    pub context_window: u32,
    /// Default sampling temperature.
    pub default_temperature: f32,
    /// KV cache quantization for key tensors. `None` leaves keys at F16.
    /// Set to `Q8_0` for ~2× memory savings (recommended for 12B+ models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_k: Option<nodespace_nlp_engine::KvCacheQuantType>,
    /// KV cache quantization for value tensors. Same semantics as `type_k`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_v: Option<nodespace_nlp_engine::KvCacheQuantType>,
}

/// Event payload emitted during model download progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadEvent {
    /// Identifier of the model being downloaded.
    pub model_id: String,
    /// Bytes downloaded so far.
    pub bytes_downloaded: u64,
    /// Total bytes to download.
    pub bytes_total: u64,
    /// Current download speed in bytes per second.
    pub speed_bps: u64,
}

// ---------------------------------------------------------------------------
// Structs -- Agent Session
// ---------------------------------------------------------------------------

/// State of a local agent conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    /// Unique session identifier.
    pub id: String,
    /// Identifier of the model used for this session, if any.
    pub model_id: Option<String>,
    /// Ordered list of messages in this session.
    pub messages: Vec<ChatMessage>,
    /// Current status of the local agent.
    pub status: LocalAgentStatus,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Record of tool executions during this session.
    pub tool_executions: Vec<ToolExecutionRecord>,
    /// Cached dynamic context string (workspace schemas, collections, playbooks).
    /// Built once per session on first turn, then reused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_context: Option<String>,
    /// Full system prompt override (bypasses PromptAssembler / fallback).
    /// Integration tests inject a pre-built prompt without a live database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Writes completed in *earlier* turns of this conversation.
    ///
    /// The session is rebuilt from persisted messages on every turn, so this is
    /// the only channel by which a turn can know what previous turns already
    /// wrote. The tool-execution path consults it to refuse a repeat
    /// deterministically, rather than relying on the model to honour a prompt
    /// note saying the work is done.
    ///
    /// Empty for sessions with no prior writes, and for callers that do not
    /// persist history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_writes: Vec<PriorWrite>,

    /// Whether Stage-2 candidate-block injection is disabled for this
    /// session's model.
    ///
    /// Set by the caller (`LocalAgentService`) from a cached routing-probe
    /// verdict (see `local_agent::routing_probe`) when the session's model is
    /// loaded — the loop itself never probes. The routing-reliability matrix
    /// (`tests/live_openai_compat_routing.rs`) found this a per-model
    /// property: injecting the candidate block suppresses tool-calling
    /// outright on some served models, independent of the block's content.
    /// `false` for every session whose model was never probed (the native
    /// path, and any served model before its first load completes), which
    /// preserves today's behavior for everything this was not measured on.
    #[serde(default)]
    pub routing_disabled: bool,
}

/// A write completed in an earlier turn, as the duplicate guard sees it.
///
/// `tool` plus `canonical_args` is the write's identity; `node_id` and `summary`
/// exist so a refusal can name what already exists rather than reporting a bare
/// "duplicate", which the model could not relay usefully to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorWrite {
    /// Tool that performed the write.
    pub tool: String,
    /// The call's arguments, canonicalised — see `canonical_args`.
    pub canonical_args: String,
    /// Node the write produced, when the tool reported one.
    pub node_id: Option<String>,
    /// Short label for the written node, when available.
    pub summary: Option<String>,
}

/// Structured question/options from `route_clarify` (ADR-038), preserved
/// alongside the flattened `response` text so the frontend can render
/// clickable options instead of parsing markdown bullets back out of prose.
///
/// `response` on [`AgentTurnResult`] still carries `format_clarification`'s
/// flattened text for the internal LLM-facing history — a bare string is what
/// `session_already_clarified` scans for and what any plain-text reader of
/// `response` still gets. This struct is the additional structured channel
/// the UI needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarifyPrompt {
    /// The specific question put to the user.
    pub question: String,
    /// Concrete options to offer, when the model supplied any.
    pub options: Vec<String>,
}

/// Result of a complete agent turn (one round of generation + tool execution).
///
/// Captures the final assistant response text, any tool calls that were made
/// and executed, and token usage for the turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnResult {
    /// The final text response produced by the agent (after all tool calls).
    pub response: String,
    /// The model's internal reasoning (chain-of-thought) accumulated across all
    /// ReAct iterations of this turn. `None` when the model produced none.
    pub reasoning: Option<String>,
    /// Tool calls that were made and executed during this turn.
    pub tool_calls_made: Vec<ToolExecutionRecord>,
    /// Token usage statistics for this turn.
    pub usage: InferenceUsage,
    /// `Some` when this turn's response is a `route_clarify` question rather
    /// than an ordinary reply — carries the structured question/options
    /// `response` flattens into text, so the frontend can render them
    /// distinctly. `None` for every other turn, including ordinary
    /// zero-tool-call replies that merely phrase themselves as a question
    /// (see #1930's scope note on `agent_loop::run_turn`).
    #[serde(default)]
    pub clarify: Option<ClarifyPrompt>,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Engine for running chat inference against a loaded language model.
///
/// Implementors manage model state and produce streaming or complete responses.
#[async_trait]
pub trait ChatInferenceEngine: Send + Sync {
    /// Run streaming inference on the given request.
    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError>;

    /// Return metadata about the currently loaded model.
    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError>;

    /// Estimate the token count for the given text.
    async fn token_count(&self, text: &str) -> Result<u32, InferenceError>;
}

/// Manager for the local model catalog: download, verify, load, and unload.
#[async_trait]
pub trait ModelManager: Send + Sync {
    /// List all known models in the catalog.
    async fn list(&self) -> Result<Vec<ModelInfo>, ModelError>;

    /// Begin downloading a model by its identifier.
    async fn download(&self, model_id: &str) -> Result<(), ModelError>;

    /// Cancel an in-progress download.
    async fn cancel_download(&self, model_id: &str) -> Result<(), ModelError>;

    /// Delete a downloaded model from disk.
    async fn delete(&self, model_id: &str) -> Result<(), ModelError>;

    /// Load a downloaded model into memory for inference.
    async fn load(&self, model_id: &str) -> Result<(), ModelError>;

    /// Unload the currently loaded model, freeing resources.
    async fn unload(&self) -> Result<(), ModelError>;

    /// Return the identifier of the currently loaded model, if any.
    async fn loaded_model(&self) -> Result<Option<String>, ModelError>;

    /// Return the identifier of the recommended default model.
    async fn recommended_model(&self) -> Result<String, ModelError>;
}

/// A skill candidate returned by the deterministic retrieval step.
///
/// Produced by [`AgentToolExecutor::retrieve_skills`], not by a model tool
/// call. ADR-038 keeps retrieval out of the model's hands so the system can
/// bound the candidate count and apply the trust filter; this struct is what
/// crosses that boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCandidate {
    /// Node id of the matched skill.
    pub id: String,
    /// Skill name (the node's content).
    pub name: String,
    /// The skill's `description` property — the index key retrieval matched on.
    pub description: String,
    /// Raw retrieval score. The mechanical half of the Stage-2 gate; the
    /// model's judgment is the other, independently-sourced half.
    pub score: f32,
    /// Tools this skill is permitted to fire. Also the source of the skill's
    /// blast radius — see [`SkillCandidate::is_mutating`].
    pub tools: Vec<String>,
    /// The skill's full instruction subtree, rendered to markdown.
    pub instructions: String,
    /// Schema metadata scoped to this skill, as returned by retrieval.
    pub schema_metadata: serde_json::Value,
}

/// Outcome of the deterministic retrieval step.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRetrieval {
    /// Candidates, highest score first, already truncated to the bounded K.
    pub candidates: Vec<SkillCandidate>,
}

/// Executor for agent tools (function calling).
///
/// Each tool is identified by name and accepts/returns JSON values.
#[async_trait]
pub trait AgentToolExecutor: Send + Sync {
    /// Return definitions of all currently available tools.
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError>;

    /// Execute a tool by name with the given JSON arguments.
    async fn execute(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, ToolError>;

    /// Whether skill retrieval is wired up and worth spending a routing turn on.
    ///
    /// Stage 1 costs a full model generation, so the loop asks first: with no
    /// retrieval behind it there are no candidates to judge and the query it
    /// produces goes nowhere. Executors that do not implement
    /// [`AgentToolExecutor::retrieve_skills`] inherit `false` here and keep the
    /// single-turn behaviour, which is also what keeps routing out of the way
    /// of test doubles that never opted into it.
    async fn routing_available(&self) -> bool {
        false
    }

    /// Run semantic retrieval over the skill registry as a **deterministic
    /// system step**, returning at most `limit` candidates.
    ///
    /// This is deliberately not a model-facing tool. ADR-038 rejects the
    /// single-turn pull (the model calling retrieval itself) because it
    /// "removes the system's ability to bound K and enforce the trust
    /// boundary" — both of which happen here instead.
    ///
    /// The default returns no candidates, which the agent loop treats as the
    /// documented degraded path: with nothing retrieved there is nothing for
    /// Stage-2 to judge, so the turn proceeds on the general tool surface
    /// rather than failing. Test doubles that do not exercise routing inherit
    /// this and need no implementation.
    async fn retrieve_skills(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<SkillRetrieval, ToolError> {
        Ok(SkillRetrieval::default())
    }

    /// `task.status`'s currently-declared values (`core_values` +
    /// `user_values`), used to keep `update_task_status`'s parameter `enum`
    /// in step with a vocabulary ADR-076 lets a methodology bundle extend at
    /// install time.
    ///
    /// The default returns `None`, meaning "no live vocabulary available" —
    /// the tool definition then keeps the static `enum` from
    /// `def_update_task_status`. That is the correct degraded behaviour
    /// rather than an empty list, which as a JSON Schema `enum` would admit
    /// no value at all and make the tool uncallable. Test doubles inherit it
    /// and need no implementation.
    async fn task_status_values(&self) -> Option<Vec<String>> {
        None
    }
}
