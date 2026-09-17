/**
 * Backend Adapter Pattern for Tauri IPC vs HTTP Dev Server
 *
 * This module provides a unified interface for backend communication that works
 * in both Tauri desktop mode (IPC) and browser development mode (HTTP).
 *
 * # Architecture
 *
 * - **TauriAdapter**: Uses Tauri's `invoke()` for IPC communication (desktop app)
 * - **HttpAdapter**: Uses `fetch()` to communicate with HTTP dev-proxy on port 3001 (browser dev)
 * - **MockAdapter**: Returns empty/default values for test environment
 * - **Auto-detection**: Runtime detection based on `window.__TAURI_INTERNALS__` existence
 *
 * Both adapters are thin transport shims — the request/response shaping rules
 * (which fields are optional-omit vs. tri-state-clearable, matching
 * node_service.proto) live in ./adapter-core so the two paths cannot drift.
 *
 * # Usage
 *
 * ```typescript
 * import { backendAdapter } from '$lib/services/backend-adapter';
 *
 * const nodes = await backendAdapter.getChildren('parent-id');
 * ```
 */

import type { Node, NodeReference, NodeWithChildren, TaskNode, TaskNodeUpdate } from '$lib/types';
import type { SchemaNode } from '$lib/types/schema-node';
import type { RawNodeRelationships } from './relationship-grouping';
import { createLogger } from '$lib/utils/logger';
import { withDiagnosticLogging } from './diagnostic-logger';
import { invoke } from '@tauri-apps/api/core';
import {
  buildCreateNodeFields,
  buildExecuteQueryWire,
  normalizeChildrenTree,
  HTTP_ROUTES,
  type BackendAdapter,
  type CreateNodeInput,
  type UpdateNodeInput,
  type DeleteResult,
  type NodeQuery,
  type ExecuteQueryInput,
  type CreateContainerInput,
  type InsertPosition,
} from './adapter-core';

const log = createLogger('BackendAdapter');

export type {
  BackendAdapter,
  InsertPosition,
  CreateNodeInput,
  UpdateNodeInput,
  DeleteResult,
  EdgeRecord,
  NodeQuery,
  ExecuteQueryInput,
  CreateContainerInput,
} from './adapter-core';
export { insertPosition } from './adapter-core';

// ============================================================================
// Tauri Adapter (Desktop App - IPC)
// ============================================================================

class TauriAdapter implements BackendAdapter {
  async createNode(input: CreateNodeInput | Node): Promise<string> {
    // Tauri 2.x with #[serde(rename_all = "camelCase")] expects camelCase field names
    const nodeInput = buildCreateNodeFields(input);
    return withDiagnosticLogging(
      'createNode',
      () => invoke<string>('create_node', { node: nodeInput }),
      [nodeInput]
    );
  }

  async getNode(id: string): Promise<Node | null> {
    return withDiagnosticLogging(
      'getNode',
      () => invoke<Node | null>('get_node', { id }),
      [id]
    );
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    return withDiagnosticLogging(
      'updateNode',
      () => invoke<Node>('update_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode> {
    // Forwards `update` as-is rather than calling buildTaskNodeUpdatePatch —
    // the tri-state clear/set/no-change encoding for this path is done by
    // the Rust #[tauri::command] handler, not here. That handler and
    // dev-proxy's buildTaskNodeUpdatePatch call site can't literally share
    // code across the language boundary; ADR-048 decision 2 (a Rust-side
    // integration test driving the Tauri command layer directly) is the
    // planned way to prove the two encodings still agree.
    return withDiagnosticLogging(
      'updateTaskNode',
      () => invoke<TaskNode>('update_task_node', { id, version, update }),
      [id, version, update]
    );
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    return withDiagnosticLogging(
      'deleteNode',
      () => invoke<DeleteResult>('delete_node', { id, version }),
      [id, version]
    );
  }

  async getChildren(parentId: string): Promise<Node[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildren',
      () => invoke<Node[]>('get_children', { parentId }),
      [parentId]
    );
  }

  async getDescendants(rootNodeId: string): Promise<Node[]> {
    return getDescendantsViaChildren(rootNodeId, (parentId) => this.getChildren(parentId));
  }

  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildrenTree',
      async () => {
        const result = await invoke<NodeWithChildren | Record<string, never>>('get_children_tree', { parentId });
        return normalizeChildrenTree(result);
      },
      [parentId]
    );
  }

  async moveNode(nodeId: string, version: number, newParentId: string | null, insertPosition: InsertPosition | null): Promise<Node> {
    return withDiagnosticLogging(
      'moveNode',
      () => invoke<Node>('move_node', {
        nodeId,
        version,
        newParentId,
        insertPosition
      }),
      [nodeId, version, newParentId, insertPosition]
    );
  }

  async moveChildrenToParent(newParentId: string, children: Array<{ id: string; version: number }>): Promise<Node[]> {
    return withDiagnosticLogging(
      'moveChildrenToParent',
      () => invoke<Node[]>('move_children_to_parent', {
        newParentId,
        children: children.map(c => ({ nodeId: c.id, version: c.version }))
      }),
      [newParentId, children.length]
    );
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'createMention',
      () => invoke<void>('create_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      }),
      [mentioningNodeId, mentionedNodeId]
    );
  }

  async deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'deleteMention',
      () => invoke<void>('delete_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      }),
      [mentioningNodeId, mentionedNodeId]
    );
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getOutgoingMentions',
      () => invoke<string[]>('get_outgoing_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getIncomingMentions',
      () => invoke<string[]>('get_incoming_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getMentioningContainers(nodeId: string): Promise<NodeReference[]> {
    // Tauri 2.x auto-converts snake_case Rust params to camelCase JS params
    return withDiagnosticLogging(
      'getMentioningContainers',
      () => invoke<NodeReference[]>('get_mentioning_roots', { nodeId }),
      [nodeId]
    );
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    return withDiagnosticLogging(
      'queryNodes',
      () => invoke<Node[]>('query_nodes_simple', { query }),
      [query]
    );
  }

  async executeQuery(input: ExecuteQueryInput): Promise<Node[]> {
    return withDiagnosticLogging(
      'executeQuery',
      () => invoke<Node[]>('execute_query', { request: buildExecuteQueryWire(input) }),
      [input]
    );
  }

  async countQuery(input: ExecuteQueryInput): Promise<number> {
    return withDiagnosticLogging(
      'countQuery',
      () => invoke<number>('count_query', { request: buildExecuteQueryWire(input) }),
      [input]
    );
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    return withDiagnosticLogging(
      'mentionAutocomplete',
      () => invoke<Node[]>('mention_autocomplete', { query, limit }),
      [query, limit]
    );
  }

  async findDuplicateFor(
    nodeType: string,
    field: string,
    value: string,
    excludeId?: string
  ): Promise<Node | null> {
    return withDiagnosticLogging(
      'findDuplicateFor',
      () => invoke<Node | null>('find_duplicate', { nodeType, field, value, excludeId }),
      [nodeType, field, value, excludeId]
    );
  }

  async createContainerNode(input: CreateContainerInput): Promise<string> {
    // Keep snake_case for struct fields to match Rust serde expectations
    const rustInput = {
      content: input.content,
      node_type: input.nodeType,
      properties: input.properties ?? {},
      mentioned_by: input.mentionedBy
    };
    return withDiagnosticLogging(
      'createContainerNode',
      () => invoke<string>('create_root_node', { input: rustInput }),
      [input]
    );
  }

  async getAllSchemas(): Promise<SchemaNode[]> {
    return withDiagnosticLogging(
      'getAllSchemas',
      () => invoke<SchemaNode[]>('get_all_schemas'),
      []
    );
  }

  async getSchema(schemaId: string): Promise<SchemaNode> {
    return withDiagnosticLogging(
      'getSchema',
      () => invoke<SchemaNode>('get_schema_definition', { schemaId }),
      [schemaId]
    );
  }

  async searchNodesByTitle(nodeType: string | null, titleContains: string, limit?: number): Promise<Node[]> {
    return this.queryNodes({ nodeType: nodeType ?? undefined, titleContains, limit });
  }

  async getNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
    // The get_node_relationships command already parses relationships_json into
    // a serde_json::Value and returns it; its shape matches RawNodeRelationships.
    return withDiagnosticLogging(
      'getNodeRelationships',
      () => invoke<RawNodeRelationships>('get_node_relationships', { nodeId }),
      [nodeId]
    );
  }

  async createRelationship(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    edgeData?: Record<string, unknown>
  ): Promise<void> {
    return withDiagnosticLogging(
      'createRelationship',
      () => invoke<void>('create_relationship', {
        sourceId,
        relationshipName,
        targetId,
        edgeData: edgeData ?? null
      }),
      [sourceId, relationshipName, targetId]
    );
  }

  async deleteRelationship(sourceId: string, relationshipName: string, targetId: string): Promise<void> {
    return withDiagnosticLogging(
      'deleteRelationship',
      () => invoke<void>('delete_relationship', { sourceId, relationshipName, targetId }),
      [sourceId, relationshipName, targetId]
    );
  }

  async updateRelationshipProperties(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    properties: Record<string, unknown>
  ): Promise<void> {
    return withDiagnosticLogging(
      'updateRelationshipProperties',
      () => invoke<void>('update_relationship_properties', {
        sourceId,
        relationshipName,
        targetId,
        properties
      }),
      [sourceId, relationshipName, targetId]
    );
  }

  async getDaemonVersion(): Promise<string> {
    return withDiagnosticLogging(
      'getDaemonVersion',
      () => invoke<string>('get_daemon_version'),
      []
    );
  }
}

// ============================================================================
// HTTP Adapter (Browser Dev Mode - fetch to dev-proxy)
// ============================================================================

/**
 * Shape of the JSON error body dev-proxy sends for a failed request —
 * mirrors the Tauri command layer's `CommandError` (see
 * packages/desktop-app/src-tauri/src/commands/nodes.rs) closely enough that
 * a well-formed body carries the same `code`/`conflictData` a Tauri
 * CommandError would.
 */
interface BackendErrorBody {
  message?: string;
  code?: string;
  conflictData?: unknown;
}

/**
 * Typed error thrown by `HttpAdapter.handleResponse` for a failed request.
 *
 * Unlike a bare `Error(message)`, this preserves the wire-level detail a
 * caller needs to classify the failure: the backend's structured error
 * `code` (e.g. `"SUBTREE_ACCESS_DENIED"`, `"VERSION_CONFLICT"`), the HTTP
 * `status`, and — when the body carries one — the structured `conflictData`
 * payload the daemon attaches to OCC-conflict and subtree-access refusals.
 *
 * `code`/`conflictData` are read structurally (not via `instanceof`) by
 * `isSubtreeAccessDenied` / `isVersionConflict` in `$lib/types/errors`, so a
 * `BackendError` shaped like a `SUBTREE_ACCESS_DENIED` or `VERSION_CONFLICT`
 * CommandError is classified the same way regardless of transport.
 */
export class BackendError extends Error {
  constructor(
    message: string,
    readonly code?: string,
    readonly status?: number,
    readonly conflictData?: unknown
  ) {
    super(message);
    this.name = 'BackendError';
  }
}

export class HttpAdapter implements BackendAdapter {
  private readonly baseUrl: string;

  constructor(baseUrl: string = 'http://localhost:3001') {
    this.baseUrl = baseUrl;
  }

  private getHeaders(): Record<string, string> {
    return {
      'Content-Type': 'application/json'
      // No X-Client-Id header needed
      // dev-proxy represents all browser clients as single logical client
    };
  }

  private async handleResponse<T>(response: Response): Promise<T> {
    if (!response.ok) {
      const fallbackMessage = `HTTP ${response.status}: ${response.statusText}`;

      // This try only guards the JSON parse itself — a genuinely malformed
      // (non-JSON) error body takes the SyntaxError branch below. A
      // well-formed body, whatever its content, falls through to error
      // construction OUTSIDE the try so it can't be caught by its own catch.
      let errorData: BackendErrorBody | undefined;
      try {
        errorData = await response.json();
      } catch (parseError) {
        if (parseError instanceof SyntaxError) {
          throw new BackendError(fallbackMessage, undefined, response.status);
        }
        throw parseError;
      }

      throw new BackendError(
        errorData?.message || fallbackMessage,
        errorData?.code,
        response.status,
        errorData?.conflictData
      );
    }

    if (response.status === 204 || response.headers.get('content-length') === '0') {
      return undefined as T;
    }

    return await response.json();
  }

  async createNode(input: CreateNodeInput | Node): Promise<string> {
    const now = new Date().toISOString();
    const fields = buildCreateNodeFields(input);
    const requestBody = {
      ...fields,
      createdAt: now,
      modifiedAt: now,
      version: 1
    };

    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.createNode()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(requestBody)
    });

    return await this.handleResponse<string>(response);
  }

  async getNode(id: string): Promise<Node | null> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getNode(id)}`);
    if (response.status === 404) return null;
    return await this.handleResponse<Node>(response);
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.updateNode(id)}`, {
      method: 'PATCH',
      headers: this.getHeaders(),
      body: JSON.stringify({ ...update, version })
    });
    return await this.handleResponse<Node>(response);
  }

  async updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.updateTaskNode(id)}`, {
      method: 'PATCH',
      headers: this.getHeaders(),
      body: JSON.stringify({ ...update, version })
    });
    return await this.handleResponse<TaskNode>(response);
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.deleteNode(id)}`, {
      method: 'DELETE',
      headers: this.getHeaders(),
      body: JSON.stringify({ version })
    });
    return await this.handleResponse<DeleteResult>(response);
  }

  async getChildren(parentId: string): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getChildren(parentId)}`);
    return await this.handleResponse<Node[]>(response);
  }

  async getDescendants(rootNodeId: string): Promise<Node[]> {
    return getDescendantsViaChildren(rootNodeId, (parentId) => this.getChildren(parentId));
  }

  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getChildrenTree(parentId)}`);
    const result = await this.handleResponse<NodeWithChildren | Record<string, never>>(response);
    return normalizeChildrenTree(result);
  }

  async moveNode(nodeId: string, version: number, newParentId: string | null, insertPosition: InsertPosition | null): Promise<Node> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.moveNode(nodeId)}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ version, parentId: newParentId, insertPosition })
    });
    return this.handleResponse<Node>(response);
  }

  async moveChildrenToParent(newParentId: string, children: Array<{ id: string; version: number }>): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.moveChildrenToParent(newParentId)}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ children: children.map(c => ({ nodeId: c.id, version: c.version })) })
    });
    return this.handleResponse<Node[]>(response);
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.createMention()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId: mentioningNodeId, targetId: mentionedNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.deleteMention()}`, {
      method: 'DELETE',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId: mentioningNodeId, targetId: mentionedNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getOutgoingMentions(nodeId)}`);
    return await this.handleResponse<string[]>(response);
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getIncomingMentions(nodeId)}`);
    return await this.handleResponse<string[]>(response);
  }

  async getMentioningContainers(nodeId: string): Promise<NodeReference[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getMentioningContainers(nodeId)}`);
    return await this.handleResponse<NodeReference[]>(response);
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.queryNodes()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(query)
    });
    return await this.handleResponse<Node[]>(response);
  }

  async executeQuery(input: ExecuteQueryInput): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.executeQuery()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(buildExecuteQueryWire(input))
    });
    return await this.handleResponse<Node[]>(response);
  }

  async countQuery(input: ExecuteQueryInput): Promise<number> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.countQuery()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(buildExecuteQueryWire(input))
    });
    return await this.handleResponse<number>(response);
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.mentionAutocomplete()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ query, limit })
    });
    return await this.handleResponse<Node[]>(response);
  }

  async findDuplicateFor(
    nodeType: string,
    field: string,
    value: string,
    excludeId?: string
  ): Promise<Node | null> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.findDuplicate()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ nodeType, field, value, excludeId })
    });
    return await this.handleResponse<Node | null>(response);
  }

  async createContainerNode(input: CreateContainerInput): Promise<string> {
    // Use createNode with no parent for root node creation
    return this.createNode({
      id: crypto.randomUUID(),
      nodeType: input.nodeType,
      content: input.content,
      properties: input.properties,
      mentions: [],
      parentId: null
    });
  }

  async getAllSchemas(): Promise<SchemaNode[]> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getAllSchemas()}`);
    return this.handleResponse<SchemaNode[]>(response);
  }

  async getSchema(schemaId: string): Promise<SchemaNode> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getSchema(schemaId)}`);
    return this.handleResponse<SchemaNode>(response);
  }

  async searchNodesByTitle(nodeType: string | null, titleContains: string, limit?: number): Promise<Node[]> {
    return this.queryNodes({ nodeType: nodeType ?? undefined, titleContains, limit });
  }

  async getNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getNodeRelationships(nodeId)}`);
    return this.handleResponse<RawNodeRelationships>(response);
  }

  async createRelationship(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    edgeData?: Record<string, unknown>
  ): Promise<void> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.createRelationship()}`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId, relationshipName, targetId, edgeData: edgeData ?? null })
    });
    await this.handleResponse<void>(response);
  }

  async deleteRelationship(sourceId: string, relationshipName: string, targetId: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.deleteRelationship()}`, {
      method: 'DELETE',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId, relationshipName, targetId })
    });
    await this.handleResponse<void>(response);
  }

  async updateRelationshipProperties(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    properties: Record<string, unknown>
  ): Promise<void> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.updateRelationshipProperties()}`, {
      method: 'PATCH',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId, relationshipName, targetId, properties })
    });
    await this.handleResponse<void>(response);
  }

  async getDaemonVersion(): Promise<string> {
    const response = await fetch(`${this.baseUrl}${HTTP_ROUTES.getDaemonVersion()}`);
    return this.handleResponse<string>(response);
  }
}

// ============================================================================
// Shared helper (not transport-specific — recursion, not wire shaping)
// ============================================================================

async function getDescendantsViaChildren(
  rootNodeId: string,
  getChildren: (parentId: string) => Promise<Node[]>,
): Promise<Node[]> {
  const allNodes: Node[] = [];
  const queue: string[] = [rootNodeId];

  while (queue.length > 0) {
    const parentId = queue.shift()!;
    const children = await getChildren(parentId);
    allNodes.push(...children);
    queue.push(...children.map((c) => c.id));
  }

  return allNodes;
}

// ============================================================================
// Mock Adapter (Test Environment)
// ============================================================================

class MockAdapter implements BackendAdapter {
  async createNode(_input: CreateNodeInput | Node): Promise<string> {
    return 'mock-id';
  }
  async getNode(_id: string): Promise<Node | null> {
    return null;
  }
  async updateNode(_id: string, _version: number, _update: UpdateNodeInput): Promise<Node> {
    return {} as Node;
  }
  async updateTaskNode(_id: string, _version: number, _update: TaskNodeUpdate): Promise<TaskNode> {
    return {} as TaskNode;
  }
  async deleteNode(_id: string, _version: number): Promise<DeleteResult> {
    return { existed: true, deletedCount: 0 };
  }
  async getChildren(_parentId: string): Promise<Node[]> {
    return [];
  }
  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    // Return null for non-existent parent (consistent with API contract)
    if (!parentId || parentId === 'non-existent') {
      return null;
    }
    // Return realistic mock structure with empty children
    return {
      id: parentId,
      nodeType: 'text',
      content: '',
      version: 0,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      children: []
    };
  }
  async getDescendants(_rootNodeId: string): Promise<Node[]> {
    return [];
  }
  async moveNode(_nodeId: string, _version: number, _newParentId: string | null, _insertPosition: InsertPosition | null): Promise<Node> {
    return {} as Node;
  }
  async moveChildrenToParent(_newParentId: string, children: Array<{ id: string; version: number }>): Promise<Node[]> {
    return children.map(() => ({} as Node));
  }
  async createMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async deleteMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async getOutgoingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getIncomingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getMentioningContainers(_nodeId: string): Promise<NodeReference[]> {
    return [];
  }
  async queryNodes(_query: NodeQuery): Promise<Node[]> {
    return [];
  }
  async executeQuery(_input: ExecuteQueryInput): Promise<Node[]> {
    return [];
  }
  async countQuery(_input: ExecuteQueryInput): Promise<number> {
    return 0;
  }
  async mentionAutocomplete(_query: string, _limit?: number): Promise<Node[]> {
    return [];
  }
  async findDuplicateFor(
    _nodeType: string,
    _field: string,
    _value: string,
    _excludeId?: string
  ): Promise<Node | null> {
    return null;
  }
  async createContainerNode(_input: CreateContainerInput): Promise<string> {
    return 'mock-container-id';
  }
  async getAllSchemas(): Promise<SchemaNode[]> {
    return [];
  }
  async getSchema(schemaId: string): Promise<SchemaNode> {
    // Return a mock schema node with typed top-level fields
    return {
      id: schemaId,
      nodeType: 'schema',
      content: schemaId,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      // Typed schema fields at top level (not in properties)
      isCore: false,
      schemaVersion: 1,
      description: '',
      fields: []
    };
  }
  async searchNodesByTitle(_nodeType: string | null, _titleContains: string, _limit?: number): Promise<Node[]> {
    return [];
  }
  async getNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
    return { nodeId, nodeType: '', groups: [] };
  }
  async createRelationship(
    _sourceId: string,
    _relationshipName: string,
    _targetId: string,
    _edgeData?: Record<string, unknown>
  ): Promise<void> {}
  async deleteRelationship(_sourceId: string, _relationshipName: string, _targetId: string): Promise<void> {}
  async updateRelationshipProperties(
    _sourceId: string,
    _relationshipName: string,
    _targetId: string,
    _properties: Record<string, unknown>
  ): Promise<void> {}
  async getDaemonVersion(): Promise<string> {
    return '0.0.0-mock';
  }
}

// ============================================================================
// Environment Detection & Factory
// ============================================================================

/**
 * Check if running in Tauri desktop environment
 */
function isTauriEnvironment(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

/**
 * Check if running in test environment
 */
function isTestEnvironment(): boolean {
  return typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
}

/**
 * Get the appropriate backend adapter based on runtime environment
 *
 * - Test environment: Returns MockAdapter (no-op mocks)
 * - Tauri desktop app: Returns TauriAdapter (uses IPC)
 * - Web dev mode: Returns HttpAdapter (uses HTTP to port 3001)
 */
export function getBackendAdapter(): BackendAdapter {
  if (isTestEnvironment()) {
    return new MockAdapter();
  }

  if (isTauriEnvironment()) {
    log.debug('Using Tauri IPC adapter');
    return new TauriAdapter();
  }

  log.debug('Using HTTP dev server adapter (port 3001)');
  return new HttpAdapter();
}

/**
 * Singleton instance for convenient access
 * Auto-detects environment and returns appropriate adapter
 */
export const backendAdapter = getBackendAdapter();
