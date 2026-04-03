//! Shared types, traits, and interface contracts for agent subsystems.
//!
//! This module defines the foundational type definitions, trait interfaces,
//! message formats, and Tauri event constants that all agent-related subsystems
//! code against. It produces no runtime behavior -- only type definitions,
//! trait declarations, and module scaffolding.
//!
//! Issue #998: prerequisite that unblocks all parallel agent implementation streams.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

/// Errors returned by [`AcpTransport`] methods.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The connection to the agent process is not alive.
    #[error("transport not connected")]
    NotConnected,

    /// Sending a message failed.
    #[error("send failed: {0}")]
    SendFailed(String),

    /// Receiving a message timed out.
    #[error("receive timed out")]
    ReceiveTimeout,

    /// The agent process exited unexpectedly.
    #[error("agent process exited: {0}")]
    ProcessExited(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors returned by [`AgentRegistry`] methods.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The requested agent ID was not found.
    #[error("agent not found: {0}")]
    NotFound(String),

    /// Discovery of agents failed.
    #[error("discovery failed: {0}")]
    DiscoveryFailed(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors returned by [`ContextAssembler`] methods.
#[derive(Debug, Error)]
pub enum ContextError {
    /// The requested node could not be found.
    #[error("node not found: {0}")]
    NodeNotFound(String),

    /// Assembling the context exceeded the token budget.
    #[error("token budget exceeded: requested {requested}, budget {budget}")]
    TokenBudgetExceeded {
        /// Tokens requested.
        requested: u32,
        /// Token budget limit.
        budget: u32,
    },

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Role of a participant in a chat conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt providing instructions to the model.
    System,
    /// Message from the human user.
    User,
    /// Response from the AI assistant.
    Assistant,
    /// Output from a tool invocation.
    Tool,
}

/// A single chunk emitted during streaming inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingChunk {
    /// A token of generated text.
    Token {
        /// The text content of this token.
        text: String,
    },
    /// The model is starting a tool call.
    ToolCallStart {
        /// Unique identifier for this tool call.
        id: String,
        /// Name of the tool being invoked.
        name: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// State of an ACP (Agent Communication Protocol) session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AcpSessionState {
    /// No active session.
    Idle,
    /// Session is being set up (process spawn, handshake).
    Initializing,
    /// Session is active and processing messages.
    Active,
    /// The agent is producing its final response.
    Completing,
    /// Session ended successfully.
    Completed,
    /// Session ended with an error.
    Failed {
        /// Explanation of the failure.
        reason: String,
    },
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

/// How an ACP agent authenticates with external services.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum AcpAuthMethod {
    /// The agent manages its own credentials internally.
    AgentManaged,
    /// Credentials are provided via an environment variable.
    EnvApiKey {
        /// Name of the environment variable holding the API key.
        var_name: String,
    },
}

/// Family of language models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFamily {
    /// Ministral — Mistral AI's small model series (Ministral 3B, Ministral 8B).
    /// Named after the specific product line, not the parent company.
    Ministral,
}

// ---------------------------------------------------------------------------
// Structs -- Chat & Inference
// ---------------------------------------------------------------------------

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role of the message author.
    pub role: Role,
    /// Text content of the message.
    pub content: String,
    /// If this message is a tool result, the ID of the originating tool call.
    pub tool_call_id: Option<String>,
    /// Optional name for tool-role messages (the tool name).
    pub name: Option<String>,
}

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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

/// A raw tool call parsed from model output before execution.
///
/// Represents the model's intent to invoke a tool. The `arguments_json` field
/// contains the raw JSON string as emitted by the model (may need validation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRaw {
    /// Unique identifier for this tool call (from the model).
    pub id: String,
    /// Name of the tool the model wants to invoke.
    pub function_name: String,
    /// Raw JSON string of tool arguments as produced by the model.
    pub arguments_json: String,
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
// Structs -- ACP (Agent Communication Protocol)
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 message used by the Agent Communication Protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpMessage {
    /// JSON-RPC version string (always "2.0").
    pub jsonrpc: String,
    /// Method name for requests/notifications; absent for responses.
    pub method: Option<String>,
    /// Parameters for the method.
    pub params: Option<serde_json::Value>,
    /// Request identifier; absent for notifications.
    pub id: Option<serde_json::Value>,
    /// Result payload for successful responses.
    pub result: Option<serde_json::Value>,
    /// Error payload for failed responses.
    pub error: Option<AcpError>,
}

/// Error object in a JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    /// Numeric error code (follows JSON-RPC conventions).
    pub code: i32,
    /// Human-readable error description.
    pub message: String,
    /// Optional structured error data.
    pub data: Option<serde_json::Value>,
}

/// Information about an ACP-compatible agent discovered by the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpAgentInfo {
    /// Unique identifier for this agent.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Path to the agent binary.
    pub binary: String,
    /// Command-line arguments to pass when spawning the agent.
    pub args: Vec<String>,
    /// How the agent authenticates with external services.
    pub auth_method: AcpAuthMethod,
    /// Whether the agent is currently reachable.
    pub available: bool,
    /// Semantic version of the agent, if reported.
    pub version: Option<String>,
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
    pub filename: String,
    /// Size of the model file in bytes.
    pub size_bytes: u64,
    /// Quantization format (e.g. "Q4_K_M").
    pub quantization: String,
    /// URL to download the model weights.
    pub url: String,
    /// Expected SHA-256 hash of the model file.
    pub sha256: String,
    /// Current download / load status.
    pub status: ModelStatus,
}

/// Specification of a chat model's capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatModelSpec {
    /// Identifier of the model this spec describes.
    pub model_id: String,
    /// Maximum number of tokens the model can process.
    pub context_window: u32,
    /// Default sampling temperature.
    pub default_temperature: f32,
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
}

/// Result of a complete agent turn (one round of generation + tool execution).
///
/// Captures the final assistant response text, any tool calls that were made
/// and executed, and token usage for the turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnResult {
    /// The final text response produced by the agent (after all tool calls).
    pub response: String,
    /// Tool calls that were made and executed during this turn.
    pub tool_calls_made: Vec<ToolExecutionRecord>,
    /// Token usage statistics for this turn.
    pub usage: InferenceUsage,
}

// ---------------------------------------------------------------------------
// Structs -- Context Assembly
// ---------------------------------------------------------------------------

/// Assembled context packet ready for injection into a system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPacket {
    /// The assembled system prompt incorporating node context.
    pub system_prompt: String,
    /// Nodes included in the context window.
    pub context_nodes: Vec<ContextNode>,
    /// Estimated token count for the entire packet.
    pub token_count: u32,
}

/// A single node included in an assembled context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextNode {
    /// Node identifier.
    pub node_id: String,
    /// Type of the node (e.g. "text", "task").
    pub node_type: String,
    /// Text content of the node.
    pub content: String,
    /// Relationships from this node to other nodes.
    pub relationships: Vec<ContextRelationship>,
}

/// A relationship from a context node to another node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRelationship {
    /// Identifier of the target node.
    pub target_id: String,
    /// Type of relationship (e.g. "mentions", "child_of").
    pub relationship_type: String,
    /// Human-readable label for the target node.
    pub target_label: String,
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
    ///
    /// Chunks are delivered incrementally via `on_chunk`. The callback receives
    /// each [`StreamingChunk`] as it becomes available (tokens, tool call
    /// fragments, errors). When inference completes, the method returns
    /// aggregate [`InferenceUsage`] statistics.
    ///
    /// This callback-based design avoids buffering the entire response and
    /// lets callers forward chunks to the frontend in real time.
    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError>;

    /// Return metadata about the currently loaded model.
    ///
    /// Returns `None` if no model is loaded.
    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError>;

    /// Estimate the token count for the given text.
    ///
    /// This is used for context window management and budget tracking.
    async fn token_count(&self, text: &str) -> Result<u32, InferenceError>;
}

/// Manager for the local model catalog: download, verify, load, and unload.
#[async_trait]
pub trait ModelManager: Send + Sync {
    /// List all known models in the catalog.
    async fn list(&self) -> Result<Vec<ModelInfo>, ModelError>;

    /// Begin downloading a model by its identifier.
    ///
    /// Progress is reported via [`DownloadEvent`] emitted on the
    /// [`events::MODEL_DOWNLOAD_PROGRESS`] Tauri event channel.
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

/// Executor for agent tools (function calling).
///
/// Each tool is identified by name and accepts/returns JSON values.
#[async_trait]
pub trait AgentToolExecutor: Send + Sync {
    /// Return definitions of all currently available tools.
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError>;

    /// Execute a tool by name with the given JSON arguments.
    async fn execute(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, ToolError>;
}

/// Transport layer for communicating with ACP agent processes.
///
/// Manages the stdio/JSON-RPC connection to a spawned agent binary.
#[async_trait]
pub trait AcpTransport: Send + Sync {
    /// Send a JSON-RPC message to the agent.
    async fn send(&self, message: AcpMessage) -> Result<(), TransportError>;

    /// Receive the next JSON-RPC message from the agent.
    ///
    /// Blocks until a message is available or the transport is shut down.
    async fn receive(&self) -> Result<AcpMessage, TransportError>;

    /// Check whether the agent process is still alive.
    async fn is_alive(&self) -> bool;

    /// Gracefully shut down the transport and the underlying agent process.
    async fn shutdown(&self) -> Result<(), TransportError>;
}

/// Registry for discovering and managing ACP-compatible agents.
#[async_trait]
pub trait AgentRegistry: Send + Sync {
    /// Discover all available agents (scans configured directories).
    async fn discover_agents(&self) -> Result<Vec<AcpAgentInfo>, RegistryError>;

    /// Get information about a specific agent by its identifier.
    async fn get_agent(&self, agent_id: &str) -> Result<AcpAgentInfo, RegistryError>;

    /// Refresh the agent catalog by re-scanning discovery paths.
    async fn refresh(&self) -> Result<(), RegistryError>;
}

/// Assembler for building context packets from the knowledge graph.
///
/// Gathers relevant nodes, relationships, and system prompt fragments
/// into a single [`ContextPacket`] that fits within a token budget.
#[async_trait]
pub trait ContextAssembler: Send + Sync {
    /// Assemble a context packet for the given node IDs within the token budget.
    ///
    /// The assembler selects the most relevant content from the knowledge graph
    /// (including semantic neighbors and relationships) that fits within
    /// `token_budget` tokens.
    async fn assemble(
        &self,
        node_ids: Vec<String>,
        token_budget: u32,
    ) -> Result<ContextPacket, ContextError>;
}

// ---------------------------------------------------------------------------
// Tauri event channel constants
// ---------------------------------------------------------------------------

/// Constants for Tauri event channel names used by the agent subsystem.
///
/// Both the Rust backend and TypeScript frontend must use identical strings
/// when emitting / listening for events across the Tauri bridge.
pub mod events {
    /// Streaming inference chunk from the local agent.
    pub const LOCAL_AGENT_CHUNK: &str = "local-agent://chunk";

    /// Tool execution event from the local agent.
    pub const LOCAL_AGENT_TOOL: &str = "local-agent://tool";

    /// Local agent status change (idle, thinking, streaming, etc.).
    pub const LOCAL_AGENT_STATUS: &str = "local-agent://status";

    /// Local agent error event.
    pub const LOCAL_AGENT_ERROR: &str = "local-agent://error";

    /// Model download progress update.
    pub const MODEL_DOWNLOAD_PROGRESS: &str = "model://download-progress";

    /// ACP session state transition.
    pub const ACP_SESSION_STATE: &str = "acp://session-state";

    /// Message received from an ACP agent.
    pub const ACP_AGENT_MESSAGE: &str = "acp://agent-message";
}
