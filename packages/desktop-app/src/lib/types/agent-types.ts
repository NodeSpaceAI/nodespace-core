/**
 * Shared types and interface contracts for agent subsystems.
 *
 * TypeScript equivalents of the Rust types defined in `agent_types.rs`.
 * These interfaces mirror the serde JSON output from the Tauri boundary
 * and are used for type-safe event handling and command return values.
 *
 * Issue #998: prerequisite that unblocks all parallel agent implementation streams.
 */

// ---------------------------------------------------------------------------
// Enums (as string union types matching Rust serde output)
// ---------------------------------------------------------------------------

/** Role of a participant in a chat conversation. */
export type Role = 'system' | 'user' | 'assistant' | 'tool';

/** Family of language models. */
export type ModelFamily = 'ministral';

// ---------------------------------------------------------------------------
// Tagged union enums (matching Rust #[serde(tag = "type/status/state/method")])
// ---------------------------------------------------------------------------

/** A token of generated text. */
export interface StreamingToken {
	readonly type: 'token';
	readonly text: string;
}

/** The model is starting a tool call. */
export interface StreamingToolCallStart {
	readonly type: 'tool_call_start';
	readonly id: string;
	readonly name: string;
}

/** Incremental arguments JSON for an in-progress tool call. */
export interface StreamingToolCallArgs {
	readonly type: 'tool_call_args';
	readonly id: string;
	readonly args_json: string;
}

/** Inference is complete. */
export interface StreamingDone {
	readonly type: 'done';
	readonly usage: InferenceUsage;
}

/** An error occurred during streaming. */
export interface StreamingError {
	readonly type: 'error';
	readonly message: string;
}

/** A single chunk emitted during streaming inference. */
export type StreamingChunk =
	| StreamingToken
	| StreamingToolCallStart
	| StreamingToolCallArgs
	| StreamingDone
	| StreamingError;

/** Model is known but not yet downloaded. */
export interface ModelStatusNotDownloaded {
	readonly status: 'not_downloaded';
}

/** Model is currently being downloaded. */
export interface ModelStatusDownloading {
	readonly status: 'downloading';
	readonly progress_pct: number;
	readonly bytes_downloaded: number;
	readonly bytes_total: number;
}

/** Download complete, verifying integrity. */
export interface ModelStatusVerifying {
	readonly status: 'verifying';
}

/** Model is on disk and ready to be loaded. */
export interface ModelStatusReady {
	readonly status: 'ready';
}

/** Model is loaded into memory and available for inference. */
export interface ModelStatusLoaded {
	readonly status: 'loaded';
}

/** An error occurred. */
export interface ModelStatusError {
	readonly status: 'error';
	readonly message: string;
}

/** Current status of a model in the local catalog. */
export type ModelStatus =
	| ModelStatusNotDownloaded
	| ModelStatusDownloading
	| ModelStatusVerifying
	| ModelStatusReady
	| ModelStatusLoaded
	| ModelStatusError;

/** No active ACP session. */
export interface AcpSessionIdle {
	readonly state: 'idle';
}

/** ACP session is being set up. */
export interface AcpSessionInitializing {
	readonly state: 'initializing';
}

/** ACP session is active and processing messages. */
export interface AcpSessionActive {
	readonly state: 'active';
}

/** The ACP agent is producing its final response. */
export interface AcpSessionCompleting {
	readonly state: 'completing';
}

/** ACP session ended successfully. */
export interface AcpSessionCompleted {
	readonly state: 'completed';
}

/** ACP session ended with an error. */
export interface AcpSessionFailed {
	readonly state: 'failed';
	readonly reason: string;
}

/** State of an ACP session. */
export type AcpSessionState =
	| AcpSessionIdle
	| AcpSessionInitializing
	| AcpSessionActive
	| AcpSessionCompleting
	| AcpSessionCompleted
	| AcpSessionFailed;

/** Agent is idle. */
export interface LocalAgentIdle {
	readonly status: 'idle';
}

/** Agent is processing a request. */
export interface LocalAgentThinking {
	readonly status: 'thinking';
}

/** Agent is executing a tool. */
export interface LocalAgentToolExecution {
	readonly status: 'tool_execution';
	readonly tool_name: string;
}

/** Agent is streaming a response. */
export interface LocalAgentStreaming {
	readonly status: 'streaming';
}

/** Agent encountered an error. */
export interface LocalAgentError {
	readonly status: 'error';
	readonly message: string;
}

/** Current status of the local agent. */
export type LocalAgentStatus =
	| LocalAgentIdle
	| LocalAgentThinking
	| LocalAgentToolExecution
	| LocalAgentStreaming
	| LocalAgentError;

/** Agent manages its own credentials. */
export interface AcpAuthAgentManaged {
	readonly method: 'agent_managed';
}

/** Credentials provided via environment variable. */
export interface AcpAuthEnvApiKey {
	readonly method: 'env_api_key';
	readonly var_name: string;
}

/** How an ACP agent authenticates with external services. */
export type AcpAuthMethod = AcpAuthAgentManaged | AcpAuthEnvApiKey;

// ---------------------------------------------------------------------------
// Struct interfaces
// ---------------------------------------------------------------------------

/** A single message in a chat conversation. */
export interface ChatMessage {
	readonly role: Role;
	readonly content: string;
	readonly tool_call_id?: string;
	readonly name?: string;
}

/** Parameters for an inference request. */
export interface InferenceRequest {
	readonly messages: ChatMessage[];
	readonly tools?: ToolDefinition[];
	readonly temperature?: number;
	readonly max_tokens?: number;
}

/** Token usage statistics for a completed inference turn. */
export interface InferenceUsage {
	readonly prompt_tokens: number;
	readonly completion_tokens: number;
}

/** Definition of a tool that the model can invoke. */
export interface ToolDefinition {
	readonly name: string;
	readonly description: string;
	readonly parameters_schema: unknown;
}

/** Result of a single tool invocation. */
export interface ToolResult {
	readonly tool_call_id: string;
	readonly name: string;
	readonly result: unknown;
	readonly is_error: boolean;
}

/** Complete record of a tool execution for session history. */
export interface ToolExecutionRecord {
	readonly tool_call_id: string;
	readonly name: string;
	readonly args: unknown;
	readonly result: unknown;
	readonly is_error: boolean;
	readonly duration_ms: number;
}

/** A JSON-RPC 2.0 message used by the Agent Communication Protocol. */
export interface AcpMessage {
	readonly jsonrpc: string;
	readonly method?: string;
	readonly params?: unknown;
	readonly id?: unknown;
	readonly result?: unknown;
	readonly error?: AcpError;
}

/** Error object in a JSON-RPC response. */
export interface AcpError {
	readonly code: number;
	readonly message: string;
	readonly data?: unknown;
}

/** Information about an ACP-compatible agent. */
export interface AcpAgentInfo {
	readonly id: string;
	readonly name: string;
	readonly binary: string;
	readonly args: string[];
	readonly auth_method: AcpAuthMethod;
	readonly available: boolean;
	readonly version?: string;
}

/** Metadata about a language model in the local catalog. */
export interface ModelInfo {
	readonly id: string;
	readonly family: ModelFamily;
	readonly name: string;
	readonly filename: string;
	readonly size_bytes: number;
	readonly quantization: string;
	readonly url: string;
	readonly sha256: string;
	readonly status: ModelStatus;
}

/** Specification of a chat model's capabilities. */
export interface ChatModelSpec {
	readonly model_id: string;
	readonly context_window: number;
	readonly default_temperature: number;
}

/** Event payload emitted during model download progress. */
export interface DownloadEvent {
	readonly model_id: string;
	readonly bytes_downloaded: number;
	readonly bytes_total: number;
	readonly speed_bps: number;
}

/** State of a local agent conversation session. */
export interface AgentSession {
	readonly id: string;
	readonly model_id?: string;
	readonly messages: ChatMessage[];
	readonly status: LocalAgentStatus;
	readonly created_at: string;
	readonly tool_executions: ToolExecutionRecord[];
}

/** Assembled context packet ready for injection into a system prompt. */
export interface ContextPacket {
	readonly system_prompt: string;
	readonly context_nodes: ContextNode[];
	readonly token_count: number;
}

/** A single node included in an assembled context. */
export interface ContextNode {
	readonly node_id: string;
	readonly node_type: string;
	readonly content: string;
	readonly relationships: ContextRelationship[];
}

/** A relationship from a context node to another node. */
export interface ContextRelationship {
	readonly target_id: string;
	readonly relationship_type: string;
	readonly target_label: string;
}

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

/** Check if a streaming chunk is a text token. */
export function isStreamingToken(chunk: StreamingChunk): chunk is StreamingToken {
	return chunk.type === 'token';
}

/** Check if a streaming chunk is the start of a tool call. */
export function isToolCallStart(chunk: StreamingChunk): chunk is StreamingToolCallStart {
	return chunk.type === 'tool_call_start';
}

/** Check if a streaming chunk is incremental tool call arguments. */
export function isToolCallArgs(chunk: StreamingChunk): chunk is StreamingToolCallArgs {
	return chunk.type === 'tool_call_args';
}

/** Check if a streaming chunk signals completion. */
export function isStreamingDone(chunk: StreamingChunk): chunk is StreamingDone {
	return chunk.type === 'done';
}

/** Check if a streaming chunk signals an error. */
export function isStreamingError(chunk: StreamingChunk): chunk is StreamingError {
	return chunk.type === 'error';
}

/** Check if a model status indicates download is in progress. */
export function isModelDownloading(status: ModelStatus): status is ModelStatusDownloading {
	return status.status === 'downloading';
}

/** Check if a model status indicates an error. */
export function isModelError(status: ModelStatus): status is ModelStatusError {
	return status.status === 'error';
}

/** Check if the local agent is currently executing a tool. */
export function isAgentToolExecution(
	status: LocalAgentStatus
): status is LocalAgentToolExecution {
	return status.status === 'tool_execution';
}

/** Check if an ACP session has failed. */
export function isAcpSessionFailed(state: AcpSessionState): state is AcpSessionFailed {
	return state.state === 'failed';
}

// ---------------------------------------------------------------------------
// Tauri event channel constants
// ---------------------------------------------------------------------------

/** Constants for Tauri event channel names used by the agent subsystem. */
export const AGENT_EVENTS = {
	/** Streaming inference chunk from the local agent. */
	LOCAL_AGENT_CHUNK: 'local-agent://chunk',
	/** Tool execution event from the local agent. */
	LOCAL_AGENT_TOOL: 'local-agent://tool',
	/** Local agent status change. */
	LOCAL_AGENT_STATUS: 'local-agent://status',
	/** Local agent error event. */
	LOCAL_AGENT_ERROR: 'local-agent://error',
	/** Model download progress update. */
	MODEL_DOWNLOAD_PROGRESS: 'model://download-progress',
	/** ACP session state transition. */
	ACP_SESSION_STATE: 'acp://session-state',
	/** Message received from an ACP agent. */
	ACP_AGENT_MESSAGE: 'acp://agent-message',
} as const;
