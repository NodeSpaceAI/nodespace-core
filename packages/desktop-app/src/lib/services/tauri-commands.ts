/**
 * Tauri Command Wrappers
 *
 * Provides typed TypeScript wrappers for Tauri backend commands.
 * These replace the obsolete tauri-node-service.ts with direct invoke calls.
 *
 * Architecture:
 * - All database operations go through Tauri invoke
 * - LIVE SELECT events are handled by tauri-sync-listener.ts
 * - No intermediate service layer needed
 * - In test environment, operations are no-ops (mocked)
 */

import { isTestEnvironment } from '$lib/utils/test-environment';
import type { Node } from '$lib/types';

// Lazy-loaded invoke function - only imported in non-test environment
let _invoke: typeof import('@tauri-apps/api/core').invoke | null = null;

/**
 * Get the Tauri invoke function, lazily loaded to avoid test environment errors.
 * In test environment, returns a no-op mock that silently succeeds.
 * This allows SharedNodeStore persistence logic to run without actual database calls.
 */
async function getInvoke(): Promise<typeof import('@tauri-apps/api/core').invoke> {
  if (isTestEnvironment()) {
    // In test environment, return a no-op mock
    // SharedNodeStore tests use in-memory state; actual Tauri calls are not needed
    // This prevents unhandled rejection errors during debounced persistence
    return (async <T>(): Promise<T> => {
      // Return sensible defaults for different command types
      // Most commands return void or empty results in test mode
      return undefined as T;
    }) as unknown as typeof import('@tauri-apps/api/core').invoke;
  }

  if (!_invoke) {
    const tauriCore = await import('@tauri-apps/api/core');
    _invoke = tauriCore.invoke;
  }
  return _invoke;
}

// ============================================================================
// Types for Tauri Commands
// ============================================================================

/**
 * Input for creating a node via Tauri command
 */
export interface CreateNodeInput {
  id: string;
  nodeType: string;
  content: string;
  parentId?: string | null;
  beforeSiblingId?: string | null;
  properties: Record<string, unknown>;
}

/**
 * Input for updating a node via Tauri command
 */
export interface UpdateNodeInput {
  content?: string;
  nodeType?: string;
  properties?: Record<string, unknown>;
}

/**
 * Result from delete operation
 */
export interface DeleteResult {
  deletedNodeIds: string[];
}

/**
 * Edge record from database
 */
export interface EdgeRecord {
  id: string;
  in: string;
  out: string;
  edgeType: string;
  positionId?: string;
}

// ============================================================================
// Node CRUD Commands
// ============================================================================

/**
 * Create a new node
 * Accepts either CreateNodeInput or a full Node object
 */
export async function createNode(input: CreateNodeInput | Node): Promise<string> {
  const invoke = await getInvoke();
  // Handle both CreateNodeInput and Node formats
  const node = 'nodeType' in input && !('node_type' in input)
    ? {
        id: input.id,
        node_type: input.nodeType,
        content: input.content,
        parent_id: (input as CreateNodeInput).parentId ?? null,
        before_sibling_id: (input as CreateNodeInput).beforeSiblingId ?? null,
        properties: input.properties ?? {}
      }
    : {
        id: input.id,
        node_type: input.nodeType,
        content: input.content,
        parent_id: null,
        before_sibling_id: null,
        properties: input.properties ?? {}
      };

  return invoke<string>('create_node', { node });
}

/**
 * Get a node by ID
 */
export async function getNode(id: string): Promise<Node | null> {
  const invoke = await getInvoke();
  return invoke<Node | null>('get_node', { id });
}

/**
 * Update an existing node
 */
export async function updateNode(
  id: string,
  version: number,
  update: UpdateNodeInput
): Promise<Node> {
  const invoke = await getInvoke();
  return invoke<Node>('update_node', { id, version, update });
}

/**
 * Delete a node by ID
 */
export async function deleteNode(id: string, version: number): Promise<DeleteResult> {
  const invoke = await getInvoke();
  return invoke<DeleteResult>('delete_node', { id, version });
}

// ============================================================================
// Hierarchy Commands
// ============================================================================

/**
 * Get child nodes of a parent
 */
export async function getChildren(parentId: string): Promise<Node[]> {
  const invoke = await getInvoke();
  return invoke<Node[]>('get_children', { parent_id: parentId });
}

/**
 * Get nodes by container ID (for page loading)
 */
export async function getNodesByContainerId(containerNodeId: string): Promise<Node[]> {
  const invoke = await getInvoke();
  return invoke<Node[]>('get_nodes_by_container_id', { container_node_id: containerNodeId });
}

/**
 * Move a node to a new parent with new sibling position
 */
export async function moveNode(
  nodeId: string,
  newParentId: string | null,
  newBeforeSiblingId: string | null
): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('move_node', {
    node_id: nodeId,
    new_parent_id: newParentId,
    new_before_sibling_id: newBeforeSiblingId
  });
}

/**
 * Reorder a node within its siblings
 */
export async function reorderNode(
  nodeId: string,
  version: number,
  beforeSiblingId: string | null
): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('reorder_node', {
    node_id: nodeId,
    version,
    before_sibling_id: beforeSiblingId
  });
}

/**
 * Get all edges (for bulk tree loading)
 */
export async function getAllEdges(): Promise<EdgeRecord[]> {
  const invoke = await getInvoke();
  return invoke<EdgeRecord[]>('get_all_edges', {});
}

// ============================================================================
// Mention Commands
// ============================================================================

/**
 * Create a mention relationship
 */
export async function createMention(
  mentioningNodeId: string,
  mentionedNodeId: string
): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('create_node_mention', {
    mentioning_node_id: mentioningNodeId,
    mentioned_node_id: mentionedNodeId
  });
}

/**
 * Delete a mention relationship
 */
export async function deleteMention(
  mentioningNodeId: string,
  mentionedNodeId: string
): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('delete_node_mention', {
    mentioning_node_id: mentioningNodeId,
    mentioned_node_id: mentionedNodeId
  });
}

/**
 * Get outgoing mentions from a node
 */
export async function getOutgoingMentions(nodeId: string): Promise<string[]> {
  const invoke = await getInvoke();
  return invoke<string[]>('get_outgoing_mentions', { node_id: nodeId });
}

/**
 * Get incoming mentions (backlinks) to a node
 */
export async function getIncomingMentions(nodeId: string): Promise<string[]> {
  const invoke = await getInvoke();
  return invoke<string[]>('get_incoming_mentions', { node_id: nodeId });
}

/**
 * Get containers of nodes that mention the target node
 */
export async function getMentioningContainers(nodeId: string): Promise<string[]> {
  const invoke = await getInvoke();
  return invoke<string[]>('get_mentioning_containers', { node_id: nodeId });
}

// ============================================================================
// Query Commands
// ============================================================================

/**
 * Query nodes with flexible filtering
 */
export interface NodeQuery {
  id?: string;
  mentionedBy?: string;
  contentContains?: string;
  nodeType?: string;
  limit?: number;
}

export async function queryNodes(query: NodeQuery): Promise<Node[]> {
  const invoke = await getInvoke();
  return invoke<Node[]>('query_nodes_simple', { query });
}

/**
 * Mention autocomplete query
 */
export async function mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
  const invoke = await getInvoke();
  return invoke<Node[]>('mention_autocomplete', { query, limit });
}

// ============================================================================
// Composite Commands
// ============================================================================

/**
 * Create a container node (root-level node)
 */
export interface CreateContainerInput {
  content: string;
  nodeType: string;
  properties?: Record<string, unknown>;
  mentionedBy?: string;
}

export async function createContainerNode(input: CreateContainerInput): Promise<string> {
  const invoke = await getInvoke();
  return invoke<string>('create_container_node', {
    input: {
      content: input.content,
      node_type: input.nodeType,
      properties: input.properties ?? {},
      mentioned_by: input.mentionedBy
    }
  });
}

/**
 * Save node with automatic parent creation
 */
export interface SaveNodeWithParentInput {
  nodeId: string;
  content: string;
  nodeType: string;
  parentId: string;
  containerNodeId: string;
  beforeSiblingId?: string | null;
}

export async function saveNodeWithParent(input: SaveNodeWithParentInput): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('save_node_with_parent', {
    input: {
      node_id: input.nodeId,
      content: input.content,
      node_type: input.nodeType,
      parent_id: input.parentId,
      container_node_id: input.containerNodeId,
      before_sibling_id: input.beforeSiblingId
    }
  });
}
