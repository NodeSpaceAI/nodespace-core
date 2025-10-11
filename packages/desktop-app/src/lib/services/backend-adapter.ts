/**
 * Backend Adapter Pattern for Tauri IPC vs HTTP Dev Server
 *
 * This module provides a unified interface for backend communication that works
 * in both Tauri desktop mode (IPC) and web development mode (HTTP).
 *
 * # Architecture
 *
 * - **TauriAdapter**: Uses Tauri's `invoke()` for IPC communication
 * - **HttpAdapter**: Uses `fetch()` to communicate with HTTP dev server on port 3001
 * - **Auto-detection**: Runtime detection based on `window.__TAURI__` existence
 *
 * # Usage
 *
 * ```typescript
 * import { getBackendAdapter } from '$lib/services/backend-adapter';
 *
 * const adapter = getBackendAdapter();
 * const node = await adapter.getNode('node-123');
 * ```
 *
 * # Extension for Future Phases
 *
 * To add new operations in Phase 2/3:
 * 1. Add method to `BackendAdapter` interface
 * 2. Implement in both `TauriAdapter` and `HttpAdapter`
 * 3. No changes needed to detection or factory function
 */

import { invoke } from '@tauri-apps/api/core';
import type { Node, NodeUpdate } from '$lib/types';
import { toError, DatabaseInitializationError, NodeOperationError } from '$lib/types/errors';

/**
 * Query parameters for node queries (Phase 2)
 */
export interface QueryNodesParams {
  /**
   * Filter by parent ID
   * - Use null to query root nodes (nodes with parentId = null)
   * - Use specific ID to query children of that node
   */
  parentId?: string | null;

  /**
   * Filter by container ID (optional)
   */
  containerId?: string;
}

/**
 * Backend adapter interface - Phase 1 & Phase 2 operations
 *
 * Future phases should extend this interface by adding new methods.
 */
export interface BackendAdapter {
  /**
   * Initialize database with optional custom path
   * @param dbPath - Optional custom database path (for testing)
   * @returns Path to the initialized database
   */
  initializeDatabase(dbPath?: string): Promise<string>;

  /**
   * Create a new node
   * @param node - Node data without timestamps
   * @returns ID of the created node
   */
  createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string>;

  /**
   * Get a node by ID
   * @param id - Node ID
   * @returns Node data if found, null otherwise
   */
  getNode(id: string): Promise<Node | null>;

  /**
   * Update an existing node
   * @param id - Node ID
   * @param update - Fields to update (partial)
   */
  updateNode(id: string, update: NodeUpdate): Promise<void>;

  /**
   * Delete a node by ID
   * @param id - Node ID
   */
  deleteNode(id: string): Promise<void>;

  /**
   * Get child nodes of a parent node
   * @param parentId - Parent node ID
   * @returns Array of child nodes
   */
  getChildren(parentId: string): Promise<Node[]>;

  /**
   * Query nodes by parent and/or container (Phase 2)
   * @since Phase 2
   * @param params - Query parameters
   * @returns Array of matching nodes
   */
  queryNodes(params: QueryNodesParams): Promise<Node[]>;
}

/**
 * Tauri IPC Adapter - Uses invoke() for desktop app communication
 */
export class TauriAdapter implements BackendAdapter {
  async initializeDatabase(dbPath?: string): Promise<string> {
    try {
      if (dbPath) {
        // Note: Current Tauri command doesn't support custom path parameter
        // This would need a backend update to support custom paths
        console.warn('[TauriAdapter] Custom database path not supported via IPC yet');
      }
      return await invoke<string>('initialize_database');
    } catch (error) {
      const err = toError(error);
      throw new DatabaseInitializationError(err.message, err.stack);
    }
  }

  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string> {
    try {
      return await invoke<string>('create_node', { node });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, node.id, 'create');
    }
  }

  async getNode(id: string): Promise<Node | null> {
    try {
      return await invoke<Node | null>('get_node', { id });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'get');
    }
  }

  async updateNode(id: string, update: NodeUpdate): Promise<void> {
    try {
      await invoke<void>('update_node', { id, update });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'update');
    }
  }

  async deleteNode(id: string): Promise<void> {
    try {
      await invoke<void>('delete_node', { id });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'delete');
    }
  }

  async getChildren(parentId: string): Promise<Node[]> {
    try {
      return await invoke<Node[]>('get_children', { parentId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, parentId, 'getChildren');
    }
  }

  async queryNodes(params: QueryNodesParams): Promise<Node[]> {
    try {
      // Note: Tauri command for query_nodes needs to be implemented in the backend
      // For now, this is a placeholder that will need backend support
      return await invoke<Node[]>('query_nodes', { params });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, params.parentId ?? 'query', 'queryNodes');
    }
  }
}

/**
 * HTTP Dev Server Adapter - Uses fetch() for web mode communication
 */
export class HttpAdapter implements BackendAdapter {
  private readonly baseUrl: string;

  constructor(baseUrl: string = 'http://localhost:3001') {
    this.baseUrl = baseUrl;
  }

  private async handleResponse<T>(response: globalThis.Response): Promise<T> {
    if (!response.ok) {
      // Try to parse error as HttpError format
      try {
        const errorData = await response.json();
        throw new Error(errorData.message || `HTTP ${response.status}: ${response.statusText}`);
      } catch {
        throw new Error(`HTTP ${response.status}: ${response.statusText}`);
      }
    }

    // Handle 204 No Content responses
    if (response.status === 204 || response.headers.get('content-length') === '0') {
      return undefined as T;
    }

    return await response.json();
  }

  async initializeDatabase(dbPath?: string): Promise<string> {
    try {
      const url = new URL(`${this.baseUrl}/api/database/init`);
      if (dbPath) {
        url.searchParams.set('db_path', dbPath);
      }

      const response = await globalThis.fetch(url.toString(), {
        method: 'POST'
      });

      const data = await this.handleResponse<{ dbPath: string }>(response);
      return data.dbPath;
    } catch (error) {
      const err = toError(error);
      throw new DatabaseInitializationError(err.message, err.stack);
    }
  }

  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/nodes`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify(node)
      });

      return await this.handleResponse<string>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, node.id, 'create');
    }
  }

  async getNode(id: string): Promise<Node | null> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`
      );
      return await this.handleResponse<Node | null>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'get');
    }
  }

  async updateNode(id: string, update: NodeUpdate): Promise<void> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`,
        {
          method: 'PATCH',
          headers: {
            'Content-Type': 'application/json'
          },
          body: JSON.stringify(update)
        }
      );

      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'update');
    }
  }

  async deleteNode(id: string): Promise<void> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`,
        {
          method: 'DELETE'
        }
      );

      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'delete');
    }
  }

  async getChildren(parentId: string): Promise<Node[]> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(parentId)}/children`
      );
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, parentId, 'getChildren');
    }
  }

  /**
   * Build query URL with proper parameter encoding
   * @param params - Query parameters
   * @returns Formatted URL string
   * @private
   */
  private buildQueryUrl(params: QueryNodesParams): string {
    const url = new URL(`${this.baseUrl}/api/nodes/query`);

    // Handle parentId parameter
    if (params.parentId !== undefined) {
      // Backend expects "null" string for SQL NULL queries (root nodes)
      // See query_endpoints.rs:127-137 for backend parsing logic
      url.searchParams.set('parent_id', params.parentId === null ? 'null' : params.parentId);
    }

    // Handle containerId parameter
    if (params.containerId !== undefined) {
      url.searchParams.set('container_id', params.containerId);
    }

    return url.toString();
  }

  async queryNodes(params: QueryNodesParams): Promise<Node[]> {
    try {
      const url = this.buildQueryUrl(params);
      const response = await globalThis.fetch(url);
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, params.parentId ?? 'query', 'queryNodes');
    }
  }
}

/**
 * Detect if running in Tauri environment
 * In Tauri v2, we check for __TAURI__ global object
 */
function isTauriEnvironment(): boolean {
  return (
    typeof window !== 'undefined' &&
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ !== undefined
  );
}

/**
 * Get the appropriate backend adapter based on runtime environment
 *
 * - Tauri desktop app: Returns TauriAdapter (uses IPC)
 * - Web dev mode: Returns HttpAdapter (uses HTTP to port 3001)
 *
 * @returns BackendAdapter instance appropriate for current environment
 */
export function getBackendAdapter(): BackendAdapter {
  if (isTauriEnvironment()) {
    console.log('[BackendAdapter] Using Tauri IPC adapter');
    return new TauriAdapter();
  } else {
    console.log('[BackendAdapter] Using HTTP dev server adapter (port 3001)');
    return new HttpAdapter();
  }
}

/**
 * Singleton instance for convenient access
 * Auto-detects environment and returns appropriate adapter
 */
export const backendAdapter = getBackendAdapter();
