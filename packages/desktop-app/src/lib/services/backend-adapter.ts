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

/* global fetch, crypto */

import type { Node, NodeWithChildren } from '$lib/types';
import { getClientId } from './client-id';
import type {
  SchemaDefinition,
  AddFieldConfig,
  AddFieldResult,
  RemoveFieldResult,
  ExtendEnumResult,
  RemoveEnumValueResult
} from '$lib/types/schema';

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
  deleteNode(id: string, version: number): Promise<DeleteResult>;

  // Hierarchy
  getChildren(parentId: string): Promise<Node[]>;
  getDescendants(rootNodeId: string): Promise<Node[]>;
  getChildrenTree(parentId: string): Promise<NodeWithChildren | null>;
  moveNode(nodeId: string, newParentId: string | null, insertAfterNodeId: string | null): Promise<void>;
  getAllEdges(): Promise<EdgeRecord[]>;

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

  // Schema operations
  getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>>;
  getSchema(schemaId: string): Promise<SchemaDefinition>;
  addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult>;
  removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult>;
  extendSchemaEnum(schemaId: string, fieldName: string, newValues: string[]): Promise<ExtendEnumResult>;
  removeSchemaEnumValue(schemaId: string, fieldName: string, value: string): Promise<RemoveEnumValueResult>;
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
    return invoke<string>('create_node', { node: nodeInput });
  }

  async getNode(id: string): Promise<Node | null> {
    const invoke = await this.getInvoke();
    return invoke<Node | null>('get_node', { id });
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    const invoke = await this.getInvoke();
    return invoke<Node>('update_node', { id, version, update });
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    const invoke = await this.getInvoke();
    return invoke<DeleteResult>('delete_node', { id, version });
  }

  async getChildren(parentId: string): Promise<Node[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<Node[]>('get_children', { parentId });
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
    const result = await invoke<NodeWithChildren | Record<string, never>>('get_children_tree', { parentId });
    // Backend returns {} for non-existent parent, normalize to null
    if (!result || Object.keys(result).length === 0) {
      return null;
    }
    return result as NodeWithChildren;
  }

  async moveNode(nodeId: string, newParentId: string | null, insertAfterNodeId: string | null): Promise<void> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<void>('move_node', {
      nodeId,
      newParentId,
      insertAfterNodeId
    });
  }

  async getAllEdges(): Promise<EdgeRecord[]> {
    const invoke = await this.getInvoke();
    return invoke<EdgeRecord[]>('get_all_edges', {});
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<void>('create_node_mention', {
      mentioningNodeId,
      mentionedNodeId
    });
  }

  async deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<void>('delete_node_mention', {
      mentioningNodeId,
      mentionedNodeId
    });
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<string[]>('get_outgoing_mentions', { nodeId });
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case to camelCase
    return invoke<string[]>('get_incoming_mentions', { nodeId });
  }

  async getMentioningContainers(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    // Tauri 2.x auto-converts snake_case Rust params to camelCase JS params
    return invoke<string[]>('get_mentioning_roots', { nodeId });
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    const invoke = await this.getInvoke();
    return invoke<Node[]>('query_nodes_simple', { query });
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    const invoke = await this.getInvoke();
    return invoke<Node[]>('mention_autocomplete', { query, limit });
  }

  async createContainerNode(input: CreateContainerInput): Promise<string> {
    const invoke = await this.getInvoke();
    // Keep snake_case for struct fields to match Rust serde expectations
    return invoke<string>('create_root_node', {
      input: {
        content: input.content,
        node_type: input.nodeType,
        properties: input.properties ?? {},
        mentioned_by: input.mentionedBy
      }
    });
  }

  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    const invoke = await this.getInvoke();
    return invoke<Array<SchemaDefinition & { id: string }>>('get_all_schemas');
  }

  async getSchema(schemaId: string): Promise<SchemaDefinition> {
    const invoke = await this.getInvoke();
    return invoke<SchemaDefinition>('get_schema', { schemaId });
  }

  async addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult> {
    const invoke = await this.getInvoke();
    return invoke<AddFieldResult>('add_schema_field', { schemaId, config });
  }

  async removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult> {
    const invoke = await this.getInvoke();
    return invoke<RemoveFieldResult>('remove_schema_field', { schemaId, fieldName });
  }

  async extendSchemaEnum(schemaId: string, fieldName: string, newValues: string[]): Promise<ExtendEnumResult> {
    const invoke = await this.getInvoke();
    return invoke<ExtendEnumResult>('extend_schema_enum', { schemaId, fieldName, newValues });
  }

  async removeSchemaEnumValue(schemaId: string, fieldName: string, value: string): Promise<RemoveEnumValueResult> {
    const invoke = await this.getInvoke();
    return invoke<RemoveEnumValueResult>('remove_schema_enum_value', { schemaId, fieldName, value });
  }
}

// ============================================================================
// HTTP Adapter (Browser Dev Mode - fetch to dev-proxy)
// ============================================================================

class HttpAdapter implements BackendAdapter {
  private readonly baseUrl: string;
  private readonly clientId: string;

  constructor(baseUrl: string = 'http://localhost:3001') {
    this.baseUrl = baseUrl;
    // Get or create client ID for this browser session
    this.clientId = getClientId();
  }

  private getHeaders(): Record<string, string> {
    return {
      'Content-Type': 'application/json',
      'X-Client-Id': this.clientId
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

  async moveNode(nodeId: string, newParentId: string | null, insertAfterNodeId: string | null): Promise<void> {
    const response = await fetch(`${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/parent`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify({ parentId: newParentId, insertAfterNodeId })
    });
    await this.handleResponse<void>(response);
  }

  async getAllEdges(): Promise<EdgeRecord[]> {
    // Not directly exposed via dev-proxy - return empty for browser mode
    // Structure is managed via LIVE SELECT in Tauri mode
    return [];
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

  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    const response = await fetch(`${this.baseUrl}/api/schemas`);
    return await this.handleResponse<Array<SchemaDefinition & { id: string }>>(response);
  }

  async getSchema(schemaId: string): Promise<SchemaDefinition> {
    const response = await fetch(`${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}`);
    return await this.handleResponse<SchemaDefinition>(response);
  }

  async addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult> {
    const response = await fetch(`${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields`, {
      method: 'POST',
      headers: this.getHeaders(),
      body: JSON.stringify(config)
    });
    return await this.handleResponse<AddFieldResult>(response);
  }

  async removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult> {
    const response = await fetch(
      `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}`,
      { method: 'DELETE' }
    );
    return await this.handleResponse<RemoveFieldResult>(response);
  }

  async extendSchemaEnum(schemaId: string, fieldName: string, newValues: string[]): Promise<ExtendEnumResult> {
    // Add values one by one (API takes single value)
    // Dev-proxy route: /api/schemas/:id/fields/:name/enum
    let result: ExtendEnumResult = { schemaId, newVersion: 0, success: true };
    for (const value of newValues) {
      const response = await fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}/enum`,
        {
          method: 'POST',
          headers: this.getHeaders(),
          body: JSON.stringify({ value })
        }
      );
      result = await this.handleResponse<ExtendEnumResult>(response);
    }
    return result;
  }

  async removeSchemaEnumValue(schemaId: string, fieldName: string, value: string): Promise<RemoveEnumValueResult> {
    // Dev-proxy route: /api/schemas/:id/fields/:name/enum/:value
    const response = await fetch(
      `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}/enum/${encodeURIComponent(value)}`,
      { method: 'DELETE' }
    );
    return await this.handleResponse<RemoveEnumValueResult>(response);
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
  async moveNode(_nodeId: string, _newParentId: string | null, _insertAfterNodeId: string | null): Promise<void> {}
  async getAllEdges(): Promise<EdgeRecord[]> {
    return [];
  }
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
  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    return [];
  }
  async getSchema(_schemaId: string): Promise<SchemaDefinition> {
    return { version: 1, isCore: false, description: '', fields: [] };
  }
  async addSchemaField(_schemaId: string, _config: AddFieldConfig): Promise<AddFieldResult> {
    return { schemaId: _schemaId, newVersion: 2, success: true };
  }
  async removeSchemaField(_schemaId: string, _fieldName: string): Promise<RemoveFieldResult> {
    return { schemaId: _schemaId, newVersion: 2, success: true };
  }
  async extendSchemaEnum(_schemaId: string, _fieldName: string, _newValues: string[]): Promise<ExtendEnumResult> {
    return { schemaId: _schemaId, newVersion: 2, success: true };
  }
  async removeSchemaEnumValue(_schemaId: string, _fieldName: string, _value: string): Promise<RemoveEnumValueResult> {
    return { schemaId: _schemaId, newVersion: 2, success: true };
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
    console.debug('[BackendAdapter] Using Tauri IPC adapter');
    return new TauriAdapter();
  }

  console.debug('[BackendAdapter] Using HTTP dev server adapter (port 3001)');
  return new HttpAdapter();
}

/**
 * Singleton instance for convenient access
 * Auto-detects environment and returns appropriate adapter
 */
export const backendAdapter = getBackendAdapter();
