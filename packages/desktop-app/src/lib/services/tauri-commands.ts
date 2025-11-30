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
 */
export async function moveNode(
  nodeId: string,
  version: number,
  newParentId: string | null,
  insertAfterNodeId?: string | null
): Promise<void> {
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

