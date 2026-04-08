/**
 * Tauri Commands - Simplified API for Backend Communication
 *
 * This module provides a clean API for frontend components to communicate with the backend.
 * It uses the BackendAdapter pattern to automatically select the right transport:
 * - Tauri IPC (desktop app)
 * - HTTP fetch (browser dev mode)
 * - Mocks (test environment)
 *
 * Usage:
 * ```typescript
 * import * as tauriCommands from '$lib/services/tauri-commands';
 *
 * const nodes = await tauriCommands.getChildren('parent-id');
 * ```
 */

import {
  backendAdapter,
  type CreateNodeInput,
  type UpdateNodeInput,
  type DeleteResult,
  type EdgeRecord,
  type NodeQuery,
  type CreateContainerInput
} from './backend-adapter';
import type { Node, NodeWithChildren } from '$lib/types';
import type {
  AcpAgentInfo,
  AgentSession,
  AgentTurnResult,
  LocalAgentStatus,
  ModelInfo,
} from '$lib/types/agent-types';
import { invoke } from '@tauri-apps/api/core';

// Re-export types for convenience
export type {
  CreateNodeInput,
  UpdateNodeInput,
  DeleteResult,
  EdgeRecord,
  NodeQuery,
  CreateContainerInput
};

// ============================================================================
// Node CRUD Commands
// ============================================================================

/**
 * Create a new node
 */
export async function createNode(input: CreateNodeInput | Node): Promise<string> {
  return backendAdapter.createNode(input);
}

/**
 * Get a node by ID
 */
export async function getNode(id: string): Promise<Node | null> {
  return backendAdapter.getNode(id);
}

/**
 * Update an existing node
 */
export async function updateNode(
  id: string,
  version: number,
  update: UpdateNodeInput
): Promise<Node> {
  return backendAdapter.updateNode(id, version, update);
}

/**
 * Update a task node with type-safe property updates
 *
 * Use this for task-specific updates (status, priority, dueDate, assignee).
 * Routes through the type-specific update path that directly modifies task node properties.
 *
 * @param id - Task node ID
 * @param version - Expected version for OCC
 * @param update - TaskNodeUpdate with fields to update
 * @returns Updated TaskNode with new version
 */
export async function updateTaskNode(
  id: string,
  version: number,
  update: import('$lib/types').TaskNodeUpdate
): Promise<import('$lib/types').TaskNode> {
  return backendAdapter.updateTaskNode(id, version, update);
}

/**
 * Delete a node by ID
 */
export async function deleteNode(id: string, version: number): Promise<DeleteResult> {
  return backendAdapter.deleteNode(id, version);
}

// ============================================================================
// Hierarchy Commands
// ============================================================================

/**
 * Get child nodes of a parent
 */
export async function getChildren(parentId: string): Promise<Node[]> {
  return backendAdapter.getChildren(parentId);
}

/**
 * Get all descendants of a node (entire subtree)
 */
export async function getDescendants(rootNodeId: string): Promise<Node[]> {
  return backendAdapter.getDescendants(rootNodeId);
}

/**
 * Get children tree with nested structure (for recursive loading in browser mode)
 */
export async function getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
  return backendAdapter.getChildrenTree(parentId);
}

/**
 * Move a node to a new parent with new sibling position (with OCC)
 *
 * @param nodeId - The node to move
 * @param version - Expected version for optimistic concurrency control
 * @param newParentId - New parent ID (null = make root node)
 * @param insertAfterNodeId - Sibling to insert after (null = append at end)
 * @returns Updated node with new version (critical for frontend to sync local state)
 */
export async function moveNode(
  nodeId: string,
  version: number,
  newParentId: string | null,
  insertAfterNodeId?: string | null
): Promise<Node> {
  return backendAdapter.moveNode(nodeId, version, newParentId, insertAfterNodeId ?? null);
}

// ============================================================================
// Mention Commands
// ============================================================================

/**
 * Create a mention relationship between nodes
 */
export async function createMention(
  mentioningNodeId: string,
  mentionedNodeId: string
): Promise<void> {
  return backendAdapter.createMention(mentioningNodeId, mentionedNodeId);
}

/**
 * Delete a mention relationship
 */
export async function deleteMention(
  mentioningNodeId: string,
  mentionedNodeId: string
): Promise<void> {
  return backendAdapter.deleteMention(mentioningNodeId, mentionedNodeId);
}

/**
 * Get outgoing mentions from a node
 */
export async function getOutgoingMentions(nodeId: string): Promise<string[]> {
  return backendAdapter.getOutgoingMentions(nodeId);
}

/**
 * Get incoming mentions (backlinks) to a node
 */
export async function getIncomingMentions(nodeId: string): Promise<string[]> {
  return backendAdapter.getIncomingMentions(nodeId);
}

/**
 * Get root nodes that mention the target node (backlinks at root level)
 */
export async function getMentioningRoots(nodeId: string): Promise<string[]> {
  return backendAdapter.getMentioningContainers(nodeId);
}

// ============================================================================
// Query Commands
// ============================================================================

/**
 * Query nodes with flexible filtering
 */
export async function queryNodes(query: NodeQuery): Promise<Node[]> {
  return backendAdapter.queryNodes(query);
}

/**
 * Mention autocomplete query
 */
export async function mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
  return backendAdapter.mentionAutocomplete(query, limit);
}

// ============================================================================
// Composite Commands
// ============================================================================

/**
 * Create a container node (root-level node)
 */
export async function createContainerNode(input: CreateContainerInput): Promise<string> {
  return backendAdapter.createContainerNode(input);
}

// ============================================================================
// Environment Detection
// ============================================================================

/** Check if running in a Tauri desktop environment. */
function isTauri(): boolean {
  return typeof window !== 'undefined' && ('__TAURI__' in window || '__TAURI_INTERNALS__' in window);
}

// ============================================================================
// Local Agent Commands (Issue #1008)
// ============================================================================

/**
 * Get the current local agent status.
 */
export async function localAgentStatus(): Promise<LocalAgentStatus> {
  if (!isTauri()) return { status: 'idle' };
  return invoke<LocalAgentStatus>('local_agent_status');
}

/**
 * Create a new local agent conversation session.
 * @returns Session ID
 */
export async function localAgentNewSession(modelId: string): Promise<string> {
  if (!isTauri()) return `mock-session-${Date.now()}`;
  return invoke<string>('local_agent_new_session', { modelId });
}

/**
 * Send a user message and run one agent turn.
 * Streaming chunks are delivered via Tauri events (local-agent://chunk).
 * @returns Final turn result when generation completes.
 */
export async function localAgentSend(
  sessionId: string,
  message: string
): Promise<AgentTurnResult> {
  if (!isTauri()) {
    return {
      response: 'Mock response (Tauri not available)',
      tool_calls_made: [],
      usage: { prompt_tokens: 0, completion_tokens: 0 },
    };
  }
  return invoke<AgentTurnResult>('local_agent_send', { sessionId, message });
}

/**
 * Cancel an in-progress generation for the given session.
 */
export async function localAgentCancel(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('local_agent_cancel', { sessionId });
}

/**
 * End a local agent session, freeing all resources.
 */
export async function localAgentEndSession(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('local_agent_end_session', { sessionId });
}

/**
 * Get all active local agent sessions.
 */
export async function localAgentGetSessions(): Promise<AgentSession[]> {
  if (!isTauri()) return [];
  return invoke<AgentSession[]>('local_agent_get_sessions');
}

// ============================================================================
// Chat Model Management Commands (Issue #1008)
// ============================================================================

/**
 * List all models in the local catalog.
 */
export async function chatModelList(): Promise<ModelInfo[]> {
  if (!isTauri()) return [];
  return invoke<ModelInfo[]>('chat_model_list');
}

/**
 * Get the recommended model ID based on system RAM.
 */
export async function chatModelRecommended(): Promise<string> {
  if (!isTauri()) return 'ministral-3b-q4km';
  return invoke<string>('chat_model_recommended');
}

/**
 * Download a model. Progress events are emitted via model://download-progress.
 */
export async function chatModelDownload(modelId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('chat_model_download', { modelId });
}

/**
 * Cancel an in-progress model download.
 */
export async function chatModelCancelDownload(modelId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('chat_model_cancel_download', { modelId });
}

/**
 * Delete a downloaded model from disk.
 */
export async function chatModelDelete(modelId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('chat_model_delete', { modelId });
}

/**
 * Load a downloaded model into memory for inference.
 */
export async function chatModelLoad(modelId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('chat_model_load', { modelId });
}

/**
 * Unload the currently loaded model, freeing resources.
 */
export async function chatModelUnload(): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('chat_model_unload');
}

/**
 * Ensure a model is downloaded, loaded, and the inference engine is ready.
 * Handles full lifecycle: download → load → engine swap.
 * Emits model://status and model://download-progress events during the process.
 */
/** Returns true if the engine was (re-)installed and sessions were dropped. */
export async function ensureModelReady(modelId: string): Promise<boolean> {
  if (!isTauri()) return false;
  return invoke<boolean>('ensure_model_ready', { modelId });
}

// ============================================================================
// ACP Commands (Issue #1008)
// ============================================================================

/**
 * List all discovered ACP agents.
 */
export async function acpListAgents(): Promise<AcpAgentInfo[]> {
  if (!isTauri()) return [];
  return invoke<AcpAgentInfo[]>('acp_list_agents');
}

/**
 * Start a new ACP session with the specified agent.
 * @returns Session ID
 */
export async function acpStartSession(agentId: string): Promise<string> {
  if (!isTauri()) return `mock-acp-session-${Date.now()}`;
  return invoke<string>('acp_start_session', { agentId });
}

/**
 * Send a message to an active ACP session.
 * The agent's response is emitted via acp://agent-message event.
 */
export async function acpSendMessage(sessionId: string, message: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('acp_send_message', { sessionId, message });
}

/**
 * End an ACP session gracefully.
 */
export async function acpEndSession(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  return invoke<void>('acp_end_session', { sessionId });
}

/**
 * Refresh the agent registry and return the updated list.
 */
export async function acpRefreshAgents(): Promise<AcpAgentInfo[]> {
  if (!isTauri()) return [];
  return invoke<AcpAgentInfo[]>('acp_refresh_agents');
}

