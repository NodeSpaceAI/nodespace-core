/**
 * Tauri System Commands — non-node Tauri IPC wrappers.
 *
 * Node-CRUD operations were removed in C1a; use backendAdapter directly.
 *
 * In browser/proxy mode, model management and agent commands are forwarded to
 * the dev-proxy's /api/agent/* routes, which translate them to gRPC calls on
 * LocalAgentService. PTY commands remain Tauri-only (native process required).
 */

import type { AcpAgentInfo, LocalAgentStatus } from '$lib/types/agent-types';
import { invoke } from '@tauri-apps/api/core';
import { handleResponse } from './http-response';

// ============================================================================
// Environment Detection
// ============================================================================

/** Check if running in a Tauri desktop environment. */
function isTauri(): boolean {
  return (
    typeof window !== 'undefined' && ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

const PROXY_BASE = 'http://localhost:3001';

async function proxyGet<T>(path: string): Promise<T> {
  const res = await fetch(`${PROXY_BASE}${path}`);
  return handleResponse<T>(res);
}

async function proxyPost<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${PROXY_BASE}${path}`, {
    method: 'POST',
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : {},
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  return handleResponse<T>(res);
}

async function proxyDelete(path: string): Promise<void> {
  const res = await fetch(`${PROXY_BASE}${path}`, { method: 'DELETE' });
  await handleResponse<void>(res);
}

// ============================================================================
// Local Agent Commands
// ============================================================================

export async function localAgentStatus(): Promise<LocalAgentStatus> {
  if (!isTauri()) return { status: 'idle' };
  return invoke<LocalAgentStatus>('local_agent_status');
}

export async function localAgentCancelTurn(nodeId: string): Promise<void> {
  if (isTauri()) return invoke<void>('local_agent_cancel_turn', { nodeId });
  return proxyPost('/api/agent/cancel-turn', { nodeId });
}

/**
 * Create the chat a play is edited through (ADR-090 §3) and return its id.
 * The daemon creates the chat, its pins to the play-authoring skill and the
 * play, and its opening message; `model` is the model the chat starts on.
 */
export async function localAgentCreatePlayEditChat(
  playId: string,
  model?: { provider: string; model: string } | null
): Promise<string> {
  const args = { playId, provider: model?.provider ?? null, model: model?.model ?? null };
  if (isTauri()) return invoke<string>('local_agent_create_play_edit_chat', args);
  const created = await proxyPost<{ chatId: string }>('/api/agent/play-edit-chat', args);
  return created.chatId;
}

// ============================================================================
// Chat Model Management Commands
// ============================================================================

/** Lifecycle status of a catalog model (tagged union from the daemon). */
export interface ChatModelStatus {
  status: 'not_downloaded' | 'downloading' | 'verifying' | 'ready' | 'loaded' | 'error';
  [key: string]: unknown;
}

/**
 * A model entry from `chat_model_list`. Built-in (GGUF) and remotely-served
 * (OpenAI-compatible) models are
 * merged into one list, distinguished by `backend`. Shape matches the daemon's
 * `chat_model_list` command output (camelCase).
 */
export interface ChatModelEntry {
  id: string;
  name: string;
  backend: 'gguf' | 'openai-compat';
  status: ChatModelStatus;
  sizeBytes: number;
  quantization: string;
  minMemoryGb: number;
}

/**
 * List catalog + discovered models.
 *
 * The daemon caches OpenAI-compat endpoint discovery for a short TTL so the
 * model selector's several mount-time callers don't each re-query every
 * endpoint (see `OPENAI_COMPAT_DISCOVERY_CACHE_TTL` in the daemon's
 * `local_agent_service.rs`). Pass `forceRefresh: true` to bypass a fresh
 * cache hit — used by the explicit "Refresh remote models" action in
 * Settings; every other call site should omit it and accept a cache hit.
 */
export async function chatModelList(forceRefresh = false): Promise<ChatModelEntry[]> {
  if (isTauri()) return invoke<ChatModelEntry[]>('chat_model_list', { forceRefresh });
  return proxyGet<ChatModelEntry[]>(`/api/agent/models?force_refresh=${forceRefresh}`);
}

export async function chatModelRecommended(): Promise<string> {
  if (isTauri()) return invoke<string>('chat_model_recommended');
  const res = await proxyGet<{ modelId: string }>('/api/agent/recommended-model');
  return res.modelId;
}

export async function chatModelDownload(modelId: string): Promise<void> {
  if (isTauri()) return invoke<void>('chat_model_download', { modelId });
  return proxyPost(`/api/agent/models/${encodeURIComponent(modelId)}/download`);
}

export async function chatModelCancelDownload(modelId: string): Promise<void> {
  if (isTauri()) return invoke<void>('chat_model_cancel_download', { modelId });
  return proxyDelete(`/api/agent/models/${encodeURIComponent(modelId)}/download`);
}

export async function chatModelDelete(modelId: string): Promise<void> {
  if (isTauri()) return invoke<void>('chat_model_delete', { modelId });
  return proxyDelete(`/api/agent/models/${encodeURIComponent(modelId)}`);
}

export async function chatModelLoad(modelId: string): Promise<void> {
  if (isTauri()) return invoke<void>('chat_model_load', { modelId });
  return proxyPost(`/api/agent/models/${encodeURIComponent(modelId)}/load`);
}

export async function chatModelUnload(): Promise<void> {
  if (isTauri()) return invoke<void>('chat_model_unload');
  return proxyPost('/api/agent/models/unload');
}

export async function getSystemRamGb(): Promise<number> {
  if (isTauri()) return invoke<number>('get_system_ram_gb');
  const res = await proxyGet<{ ramGb: number }>('/api/agent/system-ram');
  return res.ramGb;
}

export async function ensureModelReady(modelId: string): Promise<boolean> {
  if (isTauri()) return invoke<boolean>('ensure_model_ready', { modelId });
  await proxyPost('/api/agent/ensure-model-ready', { modelId });
  return false;
}

// ============================================================================
// ACP Commands — temporarily disabled
// ============================================================================
//
// The ACP transport and Tauri command bridge were removed ahead of
// the PTY-based agent rewrite (ADR-032). The wrappers below stay so callers
// in the chat store / agent store don't need to be edited mid-transition,
// but they no longer invoke any Tauri command. The follow-up PTY-UI issue
// replaces these with real PTY-spawn commands.

export async function acpListAgents(): Promise<AcpAgentInfo[]> {
  return [];
}

export async function acpStartSession(_agentId: string): Promise<string> {
  return `pty-session-pending-${Date.now()}`;
}

export async function acpSendMessage(_sessionId: string, _message: string): Promise<void> {
  return;
}

export async function acpEndSession(_sessionId: string): Promise<void> {
  return;
}

export async function acpRefreshAgents(): Promise<AcpAgentInfo[]> {
  return [];
}

// ============================================================================
// PTY Agent Session Commands
// ============================================================================

export interface PtyLaunchInput {
  agentType: string;
  prompt?: string | null;
  cols: number;
  rows: number;
  /**
   * ID of the `ai-chat-pty` node this PTY session is a view onto (ADR-088).
   * When set, the session's end is recorded on this node instead of a new one.
   * Omit to launch a session not bound to a node (nothing is then written).
   */
  nodeId?: string | null;
}

export interface PtyLaunchResult {
  sessionId: string;
  createdAt: number;
}

export interface PtySessionInfo {
  sessionId: string;
  agentType: string;
  startedAt: number;
  /** The `ai-chat-pty` node the session was launched for, if any. */
  nodeId?: string | null;
}

export interface PtyListSessionsResult {
  sessions: PtySessionInfo[];
  count: number;
}

export interface PtyTerminateResult {
  sessionId: string;
  wasRunning: boolean;
}

export async function ptyLaunchSession(input: PtyLaunchInput): Promise<PtyLaunchResult> {
  return invoke<PtyLaunchResult>('launch_session', { input });
}

export async function ptyWriteInput(sessionId: string, data: number[]): Promise<number> {
  return invoke<number>('write_input', { sessionId, data });
}

export async function ptyResizeTerminal(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  return invoke<void>('resize_terminal', { sessionId, cols, rows });
}

export async function ptyTerminateSession(sessionId: string): Promise<PtyTerminateResult> {
  return invoke<PtyTerminateResult>('terminate_session', { sessionId });
}

export async function ptyListSessions(): Promise<PtyListSessionsResult> {
  return invoke<PtyListSessionsResult>('list_sessions');
}

// ============================================================================
// Session Capture Settings Commands
// ============================================================================

export type CaptureContentLevel = 'metadata_only' | 'summary' | 'full';

export interface CaptureSettings {
  enabled: boolean;
  content: CaptureContentLevel;
}

export async function getCaptureSettings(): Promise<CaptureSettings> {
  if (!isTauri()) {
    return { enabled: false, content: 'metadata_only' };
  }
  return invoke<CaptureSettings>('get_capture_settings');
}

export async function updateCaptureSettings(
  settings: Partial<CaptureSettings>
): Promise<CaptureSettings> {
  if (!isTauri()) {
    return {
      enabled: false,
      content: 'metadata_only',
      ...settings,
    } as CaptureSettings;
  }
  return invoke<CaptureSettings>('update_capture_settings', {
    enabled: settings.enabled ?? null,
    content: settings.content ?? null,
  });
}

// ============================================================================
// OpenAI-compatible Provider Config Commands
// ============================================================================

export interface OpenAiCompatConfigDto {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  model: string;
}

/** Read all OpenAI-compatible provider configs from the daemon (source of truth). */
export async function getOpenAiCompatConfigsFromDaemon(): Promise<OpenAiCompatConfigDto[]> {
  if (!isTauri()) return [];
  return invoke<OpenAiCompatConfigDto[]>('get_openai_compat_configs');
}

/** Replace the full set of OpenAI-compatible provider configs on the daemon. */
export async function setOpenAiCompatConfigsOnDaemon(
  configs: OpenAiCompatConfigDto[]
): Promise<OpenAiCompatConfigDto[]> {
  if (!isTauri()) return configs;
  return invoke<OpenAiCompatConfigDto[]>('set_openai_compat_configs', { configs });
}

// ============================================================================
// PTY Agent Availability Commands
// ============================================================================

export interface AgentAvailabilityInfo {
  agentType: string;
  binary: string;
  binaryFound: boolean;
  authFound: boolean;
  binaryPath: string | null;
  installHint: string | null;
}

export interface CheckAvailabilityResult {
  agents: AgentAvailabilityInfo[];
}

export async function ptyCheckAgentAvailability(): Promise<CheckAvailabilityResult> {
  return invoke<CheckAvailabilityResult>('check_agent_availability');
}

