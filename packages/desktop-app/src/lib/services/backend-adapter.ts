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
 * - **Auto-detection**: Runtime detection based on `window.__TAURI__` existence
 *
 * # Usage
 *
 * ```typescript
 * import { backendAdapter } from '$lib/services/backend-adapter';
 *
 * const nodes = await backendAdapter.getChildren('parent-id');
 * ```
 */

import type { Node, NodeWithChildren, TaskNode, TaskNodeUpdate } from '$lib/types';
import type { SchemaNode } from '$lib/types/schema-node';
import { createLogger } from '$lib/utils/logger';
import { withDiagnosticLogging } from './diagnostic-logger';

const log = createLogger('BackendAdapter');

// ============================================================================
// Types
// ============================================================================

export interface CreateNodeInput {
  id: string;
  nodeType: string;
  content: string;
  properties?: Record<string, unknown>;
  mentions?: string[];
  parentId?: string | null;
  /** Sibling node ID to insert after (null = insert at beginning of siblings) */
  insertAfterNodeId?: string | null;
}

export interface UpdateNodeInput {
  content?: string;
  nodeType?: string;
  properties?: Record<string, unknown>;
  mentions?: string[];
}

export interface DeleteResult {
  deletedId: string;
  deletedChildCount: number;
}

export interface EdgeRecord {
  id: string;
  in: string;
  out: string;
  order: number;
}

export interface NodeQuery {
  id?: string;
  mentionedBy?: string;
  contentContains?: string;
  nodeType?: string;
  limit?: number;
}

export interface CreateContainerInput {
  content: string;
  nodeType: string;
  properties?: Record<string, unknown>;
  mentionedBy?: string;
}


// ============================================================================
// Backend Adapter Interface
// ============================================================================

export interface BackendAdapter {
  // Node CRUD
  createNode(input: CreateNodeInput | Node): Promise<string>;
  getNode(id: string): Promise<Node | null>;
  updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node>;
  updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode>;
  deleteNode(id: string, version: number): Promise<DeleteResult>;

  // Hierarchy
  getChildren(parentId: string): Promise<Node[]>;
  getDescendants(rootNodeId: string): Promise<Node[]>;
  getChildrenTree(parentId: string): Promise<NodeWithChildren | null>;
  moveNode(nodeId: string, version: number, newParentId: string | null, insertAfterNodeId: string | null): Promise<void>;

  // Mentions
  createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;
  deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;
  getOutgoingMentions(nodeId: string): Promise<string[]>;
  getIncomingMentions(nodeId: string): Promise<string[]>;
  getMentioningContainers(nodeId: string): Promise<string[]>;

  // Queries
  queryNodes(query: NodeQuery): Promise<Node[]>;
  mentionAutocomplete(query: string, limit?: number): Promise<Node[]>;

  // Composite operations
  createContainerNode(input: CreateContainerInput): Promise<string>;

  // Schema operations (read-only - mutation commands removed in Issue #690)
  // Returns SchemaNode with typed top-level fields (isCore, schemaVersion, description, fields)
  getAllSchemas(): Promise<SchemaNode[]>;
  getSchema(schemaId: string): Promise<SchemaNode>;
}

// ============================================================================
// Tauri Adapter (Desktop App - IPC)
// ============================================================================

class TauriAdapter implements BackendAdapter {
  private _invoke: typeof import('@tauri-apps/api/core').invoke | null = null;

  private async getInvoke(): Promise<typeof import('@tauri-apps/api/core').invoke> {
    if (!this._invoke) {
      const tauriCore = await import('@tauri-apps/api/core');
      this._invoke = tauriCore.invoke;
    }
    return this._invoke;
  }

  async createNode(input: CreateNodeInput | Node): Promise<string> {
    const invoke = await this.getInvoke();
    // Tauri 2.x with #[serde(rename_all = "camelCase")] expects camelCase field names
    const nodeInput = {
      id: input.id,
      nodeType: input.nodeType,
      content: input.content,
      properties: input.properties ?? {},
      mentions: input.mentions ?? [],
      parentId: (input as CreateNodeInput).parentId ?? null,
      insertAfterNodeId: (input as CreateNodeInput).insertAfterNodeId ?? null
    };
    return withDiagnosticLogging(
      'createNode',
      () => invoke<string>('create_node', { node: nodeInput }),
      [nodeInput]
    );
  }

  async getNode(id: string): Promise<Node | null> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'getNode',
      () => invoke<Node | null>('get_node', { id }),
      [id]
    );
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'updateNode',
      () => invoke<Node>('update_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'updateTaskNode',
      () => invoke<TaskNode>('update_task_node', { id, version, update }),
      [id, version, update]
    );
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'deleteNode',
      () => invoke<DeleteResult>('delete_node', { id, version }),
      [id, version]
    );
  }

  async getChildren(parentId: string): Promise<Node[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildren',
      () => invoke<Node[]>('get_children', { parentId }),
      [parentId]
    );
  }

  async getDescendants(rootNodeId: string): Promise<Node[]> {
    // Recursively fetch all descendants using getChildren
    const allNodes: Node[] = [];
    const queue: string[] = [rootNodeId];

    while (queue.length > 0) {
      const parentId = queue.shift()!;
      const children = await this.getChildren(parentId);
      allNodes.push(...children);
      // Add children to queue for recursive traversal
      queue.push(...children.map((c) => c.id));
    }

    return allNodes;
  }

  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildrenTree',
      async () => {
        const result = await invoke<NodeWithChildren | Record<string, never>>('get_children_tree', { parentId });
        // Backend returns {} for non-existent parent, normalize to null
        if (!result || Object.keys(result).length === 0) {
          return null;
        }
        return result as NodeWithChildren;
      },
      [parentId]
    );
  }

  async moveNode(nodeId: string, version: number, newParentId: string | null, insertAfterNodeId: string | null): Promise<void> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'moveNode',
      () => invoke<void>('move_node', {
        nodeId,
        version,
        newParentId,
        insertAfterNodeId
      }),
      [nodeId, version, newParentId, insertAfterNodeId]
    );
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const invoke = await this.getInvoke();
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
    const invoke = await this.getInvoke();
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
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getOutgoingMentions',
      () => invoke<string[]>('get_outgoing_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getIncomingMentions',
      () => invoke<string[]>('get_incoming_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getMentioningContainers(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case Rust params to camelCase JS params
    return withDiagnosticLogging(
      'getMentioningContainers',
      () => invoke<string[]>('get_mentioning_roots', { nodeId }),
      [nodeId]
    );
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'queryNodes',
      () => invoke<Node[]>('query_nodes_simple', { query }),
      [query]
    );
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'mentionAutocomplete',
      () => invoke<Node[]>('mention_autocomplete', { query, limit }),
      [query, limit]
    );
  }

  async createContainerNode(input: CreateContainerInput): Promise<string> {
    const invoke = await this.getInvoke();
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
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'getAllSchemas',
      () => invoke<SchemaNode[]>('get_all_schemas'),
      []
    );
  }

  async getSchema(schemaId: string): Promise<SchemaNode> {
    const invoke = await this.getInvoke();
    return withDiagnosticLogging(
      'getSchema',
      () => invoke<SchemaNode>('get_schema_definition', { schemaId }),
      [schemaId]
    );
  }
}

// ============================================================================
// HTTP Adapter (Browser Dev Mode - fetch to dev-proxy)
// ============================================================================

class HttpAdapter implements BackendAdapter {
  private readonly baseUrl: string;

  constructor(baseUrl: string = 'http://localhost:3001') {
    this.baseUrl = baseUrl;
  }

  private getHeaders(): Record<string, string> {
    return {
      'Content-Type': 'application/json'
      // No X-Client-Id header needed (Issue #715)
      // dev-proxy represents all browser clients as single logical client
    };
  }

  private async handleResponse<T>(response: Response): Promise<T> {
    if (!response.ok) {
      try {
        const errorData = await response.json();
        throw new Error(errorData.message || `HTTP ${response.status}: ${response.statusText}`);
      } catch (parseError) {
        if (parseError instanceof SyntaxError) {
          throw new Error(`HTTP ${response.status}: ${response.statusText}`);
        }
        throw parseError;
      }
    }

    if (response.status === 204 || response.headers.get('content-length') === '0') {
      return undefined as T;
    }

    return await response.json();
  }

  async createNode(input: CreateNodeInput | Node): Promise<string> {
    const now = new Date().toISOString();
    const requestBody = {
      id: input.id,
      nodeType: input.nodeType,
      content: input.content,
      properties: input.properties ?? {},
      mentions: input.mentions ?? [],
      parentId: (input as CreateNodeInput).parentId ?? null,
      insertAfterNodeId: (input as CreateNodeInput).insertAfterNodeId ?? null,
      createdAt: now,
      modifiedAt: now,
      version: 1
    };

    const response = await fetch(`${this.baseUrl}/api/nodes`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(requestBody)
    });

    return await this.handleResponse<string>(response);
  }

  async getNode(id: string): Promise<Node | null> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`);
    if (response.status === 404) return null;
    return await this.handleResponse<Node>(response);
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      headers: this.getHeaders(),
      body: JSON.stringify({ ...update, version })
    });
    return await this.handleResponse<Node>(response);
  }

  async updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode> {
    const response = await fetch(`${this.baseUrl}/api/tasks/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      headers: this.getHeaders(),
      body: JSON.stringify({ ...update, version })
    });
    return await this.handleResponse<TaskNode>(response);
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      headers: this.getHeaders(),
      body: JSON.stringify({ version })
    });
    return await this.handleResponse<DeleteResult>(response);
  }

  async getChildren(parentId: string): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(parentId)}/children`);
    return await this.handleResponse<Node[]>(response);
  }

  async getDescendants(rootNodeId: string): Promise<Node[]> {
    // Recursively fetch all descendants using getChildren
    const allNodes: Node[] = [];
    const queue: string[] = [rootNodeId];

    while (queue.length > 0) {
      const parentId = queue.shift()!;
      const children = await this.getChildren(parentId);
      allNodes.push(...children);
      // Add children to queue for recursive traversal
      queue.push(...children.map((c) => c.id));
    }

    return allNodes;
  }

  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(parentId)}/children-tree`);
    const result = await this.handleResponse<NodeWithChildren | Record<string, never>>(response);
    // Backend returns {} for non-existent parent, normalize to null
    if (!result || Object.keys(result).length === 0) {
      return null;
    }
    return result as NodeWithChildren;
  }

  async moveNode(nodeId: string, version: number, newParentId: string | null, insertAfterNodeId: string | null): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/parent`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ version, parentId: newParentId, insertAfterNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/mentions`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId: mentioningNodeId, targetId: mentionedNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/mentions`, {
      method: 'DELETE',
      headers: this.getHeaders(),
      body: JSON.stringify({ sourceId: mentioningNodeId, targetId: mentionedNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/mentions/outgoing`);
    return await this.handleResponse<string[]>(response);
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/mentions/incoming`);
    return await this.handleResponse<string[]>(response);
  }

  async getMentioningContainers(nodeId: string): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/mentions/roots`);
    return await this.handleResponse<string[]>(response);
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}/api/query`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(query)
    });
    return await this.handleResponse<Node[]>(response);
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    const response = await fetch(`${this.baseUrl}/api/mentions/autocomplete`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ query, limit })
    });
    return await this.handleResponse<Node[]>(response);
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
    const response = await fetch(`${this.baseUrl}/api/schemas`);
    return this.handleResponse<SchemaNode[]>(response);
  }

  async getSchema(schemaId: string): Promise<SchemaNode> {
    const response = await fetch(`${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}`);
    return this.handleResponse<SchemaNode>(response);
  }
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
  async deleteNode(id: string, _version: number): Promise<DeleteResult> {
    return { deletedId: id, deletedChildCount: 0 };
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
  async moveNode(_nodeId: string, _version: number, _newParentId: string | null, _insertAfterNodeId: string | null): Promise<void> {}
  async createMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async deleteMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async getOutgoingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getIncomingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getMentioningContainers(_nodeId: string): Promise<string[]> {
    return [];
  }
  async queryNodes(_query: NodeQuery): Promise<Node[]> {
    return [];
  }
  async mentionAutocomplete(_query: string, _limit?: number): Promise<Node[]> {
    return [];
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
