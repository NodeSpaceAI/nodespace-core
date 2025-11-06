/**
 * TauriNodeService - Backend integration with automatic adapter selection
 *
 * Provides persistent storage through backend adapter pattern:
 * - Tauri desktop mode: Uses IPC via TauriAdapter
 * - Web development mode: Uses HTTP via HttpAdapter (port 3001)
 *
 * The service automatically detects the environment and uses the appropriate adapter.
 *
 * Database Location (automatically handled by backend):
 * - macOS/Linux: ~/.local/share/nodespace/nodespace.db
 * - Windows: %APPDATA%/nodespace/nodespace.db
 */

import { invoke } from '@tauri-apps/api/core';
import type { Node, NodeUpdate } from '$lib/types';
import { DatabaseInitializationError, NodeOperationError, toError } from '$lib/types/errors';
import { getBackendAdapter, type BackendAdapter } from './backend-adapter';

/**
 * Helper function for Phase 2/3 methods that aren't in backend adapter yet
 * These will be migrated to the adapter pattern in future phases.
 *
 * IMPORTANT: These methods only work in Tauri desktop mode (IPC).
 * Web mode will throw an error until Phase 2/3 HTTP endpoints are implemented.
 */
async function universalInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  // Check if we're in HTTP mode (not Tauri)
  if (typeof window !== 'undefined' && !window.__TAURI__) {
    throw new Error(
      `Method '${command}' requires Tauri IPC - HTTP endpoint not yet implemented. ` +
        `This functionality will be added in Phase 2/3 (issues #211, #212). ` +
        `Please use 'bun run tauri:dev' for full functionality.`
    );
  }
  return invoke<T>(command, args);
}

/**
 * TauriNodeService - Clean implementation with backend adapter pattern
 * Automatically uses the correct adapter (Tauri IPC or HTTP) based on environment
 */
export class TauriNodeService {
  private initialized = false;
  private dbPath: string | null = null;
  private adapter: BackendAdapter;

  constructor() {
    // Auto-detect environment and get appropriate adapter
    this.adapter = getBackendAdapter();
  }

  /**
   * Initialize database with default location
   *
   * Backend automatically chooses platform-appropriate location.
   * Call this once at app startup - BLOCKING operation.
   *
   * @param customDbPath - Optional custom database path (useful for testing)
   * @returns Path to the initialized database file
   * @throws DatabaseInitializationError if initialization fails
   */
  async initializeDatabase(customDbPath?: string): Promise<string> {
    // Guard against double initialization (can happen with HMR or React StrictMode)
    // BUT: Allow re-initialization when a different custom DB path is provided (for testing)
    if (this.initialized && this.dbPath) {
      // If requesting a different database, allow re-initialization
      if (customDbPath && customDbPath !== this.dbPath) {
        console.log(
          `[TauriNodeService] Re-initializing with different database: ${customDbPath} (was: ${this.dbPath})`
        );
        this.initialized = false;
      } else {
        console.log('[TauriNodeService] Database already initialized, returning existing path');
        return this.dbPath;
      }
    }

    try {
      this.dbPath = await this.adapter.initializeDatabase(customDbPath);
      this.initialized = true;
      console.log('[TauriNodeService] Database initialized at:', this.dbPath);
      return this.dbPath;
    } catch (error) {
      console.error('[TauriNodeService] Failed to initialize database:', error);
      throw error;
    }
  }

  /**
   * Select database location using native folder picker
   *
   * NOTE: This method only works in Tauri desktop mode (not web/HTTP mode).
   * Presents a native dialog to the user for custom database locations.
   * Most apps should use initializeDatabase() instead.
   *
   * @returns Path to the selected database file
   * @throws Error if not in Tauri mode or if user cancels
   */
  async selectDatabaseLocation(): Promise<string> {
    // This functionality requires Tauri-specific APIs
    // Not available through backend adapter (no HTTP equivalent)
    const { invoke } = await import('@tauri-apps/api/core');

    try {
      this.dbPath = await invoke<string>('select_db_location');
      this.initialized = true;
      console.log('[TauriNodeService] Database location selected:', this.dbPath);
      return this.dbPath;
    } catch (error) {
      console.error('[TauriNodeService] Failed to select database location:', error);
      throw error;
    }
  }

  /**
   * Create a new node
   *
   * Note: Caller must provide node.id (UUID or deterministic like date)
   * Backend automatically generates created_at and modified_at timestamps.
   *
   * @param node - Node data without timestamps (backend generates them)
   * @returns ID of the created node
   * @throws NodeOperationError if creation fails
   */
  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string> {
    this.ensureInitialized();
    const nodeId = await this.adapter.createNode(node);
    return nodeId;
  }

  /**
   * Get a node by ID
   *
   * @param id - Unique identifier of the node
   * @returns Node data if found, null otherwise
   * @throws NodeOperationError if operation fails
   */
  async getNode(id: string): Promise<Node | null> {
    this.ensureInitialized();
    return await this.adapter.getNode(id);
  }

  /**
   * Update an existing node
   *
   * Note: Backend automatically updates modified_at - do NOT include in update.
   *
   * @param id - ID of the node to update
   * @param version - Expected version for optimistic concurrency control
   * @param update - Fields to update (partial)
   * @throws NodeOperationError if node doesn't exist or update fails
   * @throws Version conflict error if version doesn't match current version
   */
  async updateNode(id: string, version: number, update: NodeUpdate): Promise<void> {
    this.ensureInitialized();
    await this.adapter.updateNode(id, version, update);
  }

  /**
   * Delete a node by ID
   *
   * Warning: This is destructive and cannot be undone.
   *
   * @param id - ID of the node to delete
   * @param version - Expected version for optimistic concurrency control
   * @throws NodeOperationError if node doesn't exist or deletion fails
   * @throws Version conflict error if version doesn't match current version
   */
  async deleteNode(id: string, version: number): Promise<void> {
    this.ensureInitialized();
    await this.adapter.deleteNode(id, version);
  }

  /**
   * Get child nodes of a parent node
   *
   * Returns nodes sorted by sibling order (before_sibling_id linked list).
   *
   * @param parentId - ID of the parent node
   * @returns Array of child nodes (empty if no children)
   * @throws NodeOperationError if operation fails
   */
  async getChildren(parentId: string): Promise<Node[]> {
    this.ensureInitialized();
    return await this.adapter.getChildren(parentId);
  }

  /**
   * Bulk fetch all nodes belonging to an origin node (viewer/page)
   *
   * This is the efficient way to load a complete document tree:
   * - Single database query fetches all nodes with the same container_node_id
   * - Frontend can reconstruct hierarchy using parent_id and before_sibling_id
   *
   * @param containerNodeId - ID of the container node (e.g., date page ID like "2025-10-05")
   * @returns Array of all nodes belonging to this container
   * @throws NodeOperationError if operation fails
   */
  async getNodesByContainerId(containerNodeId: string): Promise<Node[]> {
    this.ensureInitialized();
    return await this.adapter.getNodesByContainerId(containerNodeId);
  }

  /**
   * Query nodes with flexible filtering
   *
   * Supports queries by:
   * - id (exact match)
   * - mentionedBy (finds nodes that mention the specified node ID)
   * - contentContains (case-insensitive substring search)
   * - nodeType (filter by type)
   * - includeContainersAndTasks (filter to only tasks or container nodes)
   * - limit (maximum results to return)
   *
   * Query priority: id > mentionedBy > contentContains > nodeType
   *
   * @param query - Query parameters (all fields optional)
   * @returns Array of matching nodes (empty if no matches)
   * @throws NodeOperationError if operation fails
   *
   * @example
   * // Find nodes that mention a specific node (backlinks)
   * const backlinks = await service.queryNodes({ mentionedBy: 'node-123', limit: 50 });
   *
   * @example
   * // Search by content
   * const results = await service.queryNodes({ contentContains: 'project', nodeType: 'text' });
   *
   * @example
   * // Get specific node
   * const nodes = await service.queryNodes({ id: 'node-123' });
   *
   * @example
   * // Search for @mention autocomplete (only tasks and containers)
   * const results = await service.queryNodes({ contentContains: 'proj', includeContainersAndTasks: true });
   */
  async queryNodes(query: {
    id?: string;
    mentionedBy?: string;
    contentContains?: string;
    nodeType?: string;
    /**
     * Filter to only include referenceable nodes (task nodes and container nodes).
     *
     * When `true`, applies backend filter: `(node_type = 'task' OR container_node_id IS NULL)`
     *
     * **Includes:**
     * - Task nodes (all tasks, regardless of hierarchy)
     * - Container/root nodes (top-level documents with no parent)
     *
     * **Excludes:**
     * - Text child nodes and other non-referenceable content fragments
     *
     * **Primary use case:** @mention autocomplete to show only nodes users should reference.
     *
     * @default false
     */
    includeContainersAndTasks?: boolean;
    limit?: number;
  }): Promise<Node[]> {
    this.ensureInitialized();

    try {
      // Use specialized mention_autocomplete for mention queries
      // This provides a dedicated, evolvable API that works in both Tauri and HTTP modes
      if (query.includeContainersAndTasks && query.contentContains !== undefined) {
        return await this.adapter.mentionAutocomplete(query.contentContains, query.limit);
      }

      // Fall back to generic query for other query types (still uses universalInvoke)
      // TODO: Migrate remaining queries to adapter pattern
      const nodes = await universalInvoke<Node[]>('query_nodes_simple', { query });
      return nodes;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to query nodes:', query, err);
      throw new NodeOperationError(err.message, JSON.stringify(query), 'queryNodes');
    }
  }

  /**
   * Save a node with parent creation - unified upsert operation
   *
   * Ensures parent exists, then creates or updates the node in a single transaction.
   * Eliminates database locking issues from multiple sequential queries.
   *
   * @param nodeId - ID of the node to save
   * @param data - Node data including content, node_type, parent_id, origin_node_id, and before_sibling_id
   * @throws NodeOperationError if operation fails
   */
  async saveNodeWithParent(
    nodeId: string,
    data: {
      content: string;
      nodeType: string;
      parentId: string;
      containerNodeId: string;
      beforeSiblingId?: string | null;
    }
  ): Promise<void> {
    this.ensureInitialized();

    try {
      await universalInvoke<void>('save_node_with_parent', {
        nodeId,
        content: data.content,
        nodeType: data.nodeType,
        parentId: data.parentId,
        containerNodeId: data.containerNodeId,
        beforeSiblingId: data.beforeSiblingId || null
      });
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to save node with parent:', nodeId, err);
      throw new NodeOperationError(err.message, nodeId, 'saveWithParent');
    }
  }

  /**
   * Get database path (if initialized)
   */
  getDatabasePath(): string | null {
    return this.dbPath;
  }

  /**
   * Check if database is initialized
   */
  isInitialized(): boolean {
    return this.initialized;
  }

  /**
   * TEST ONLY: Initialize service with existing database path
   *
   * Use this ONLY in tests when database has already been initialized
   * externally and you need to update the singleton's internal state.
   *
   * This avoids making duplicate HTTP requests to the initialization endpoint
   * when the test infrastructure has already initialized the database.
   *
   * @param dbPath - Path to pre-initialized database
   * @throws Error if called outside test environment
   * @internal
   */
  public __testOnly_setInitializedPath(dbPath: string): void {
    if (import.meta.env.MODE !== 'test') {
      throw new Error(
        '__testOnly_setInitializedPath can only be called in test environment. ' +
          'Current mode: ' +
          import.meta.env.MODE
      );
    }
    this.dbPath = dbPath;
    this.initialized = true;
    console.log('[TauriNodeService] Test-only initialization with path:', dbPath);
  }

  /**
   * Ensure database is initialized before operations
   * @private
   */
  private ensureInitialized(): void {
    if (!this.initialized) {
      throw new DatabaseInitializationError(
        'Database not initialized. Call initializeDatabase() first.'
      );
    }
  }
}

/**
 * Singleton instance for easy access throughout the app
 */
export const tauriNodeService = new TauriNodeService();

export default TauriNodeService;
