/**
 * TauriNodeService - Real backend integration via Tauri commands
 *
 * Provides persistent storage with libsql through Tauri invoke() commands.
 * Uses unified Node type that matches Rust backend exactly.
 *
 * Database Location (automatically handled by backend):
 * - macOS/Linux: ~/.local/share/nodespace/nodespace.db
 * - Windows: %APPDATA%/nodespace/nodespace.db
 */

import { invoke } from '@tauri-apps/api/core';
import type { Node, NodeUpdate } from '$lib/types';
import { toError, DatabaseInitializationError, NodeOperationError } from '$lib/types/errors';

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
 * Invoke Tauri command - only works in Tauri environment
 * In web mode, throws an error that should be handled by caller
 */
async function universalInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriEnvironment()) {
    throw new DatabaseInitializationError(
      'Tauri API not available - running in web browser mode. Database operations require Tauri desktop app.',
      undefined
    );
  }

  return invoke<T>(command, args);
}

/**
 * TauriNodeService - Clean implementation with unified types
 * Works in both Tauri desktop and web browser environments
 */
export class TauriNodeService {
  private initialized = false;
  private dbPath: string | null = null;

  /**
   * Initialize database with default location
   *
   * Backend automatically chooses platform-appropriate location.
   * Call this once at app startup - BLOCKING operation.
   *
   * @returns Path to the initialized database file
   * @throws DatabaseInitializationError if initialization fails
   */
  async initializeDatabase(): Promise<string> {
    // Guard against double initialization (can happen with HMR or React StrictMode)
    if (this.initialized && this.dbPath) {
      console.log('[TauriNodeService] Database already initialized, returning existing path');
      return this.dbPath;
    }

    try {
      this.dbPath = await universalInvoke<string>('initialize_database');
      this.initialized = true;
      console.log('[TauriNodeService] Database initialized at:', this.dbPath);
      return this.dbPath;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to initialize database:', err);
      throw new DatabaseInitializationError(err.message, err.stack);
    }
  }

  /**
   * Select database location using native folder picker
   *
   * Presents a native dialog to the user for custom database locations.
   * Most apps should use initializeDatabase() instead.
   *
   * @returns Path to the selected database file
   * @throws DatabaseInitializationError if user cancels or initialization fails
   */
  async selectDatabaseLocation(): Promise<string> {
    try {
      this.dbPath = await universalInvoke<string>('select_db_location');
      this.initialized = true;
      console.log('[TauriNodeService] Database location selected:', this.dbPath);
      return this.dbPath;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to select database location:', err);
      throw new DatabaseInitializationError(err.message, err.stack);
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

    try {
      // Backend will add timestamps automatically
      const nodeId = await universalInvoke<string>('create_node', { node });
      console.log('[TauriNodeService] Created node:', nodeId);
      return nodeId;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to create node:', err);
      throw new NodeOperationError(err.message, node.id, 'create');
    }
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

    try {
      const node = await universalInvoke<Node | null>('get_node', { id });
      return node;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to get node:', id, err);
      throw new NodeOperationError(err.message, id, 'get');
    }
  }

  /**
   * Update an existing node
   *
   * Note: Backend automatically updates modified_at - do NOT include in update.
   *
   * @param id - ID of the node to update
   * @param update - Fields to update (partial)
   * @throws NodeOperationError if node doesn't exist or update fails
   */
  async updateNode(id: string, update: NodeUpdate): Promise<void> {
    this.ensureInitialized();

    try {
      // Backend will auto-update modified_at
      await universalInvoke<void>('update_node', { id, update });
      console.log('[TauriNodeService] Updated node:', id);
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to update node:', id, err);
      throw new NodeOperationError(err.message, id, 'update');
    }
  }

  /**
   * Delete a node by ID
   *
   * Warning: This is destructive and cannot be undone.
   *
   * @param id - ID of the node to delete
   * @throws NodeOperationError if node doesn't exist or deletion fails
   */
  async deleteNode(id: string): Promise<void> {
    this.ensureInitialized();

    try {
      await universalInvoke<void>('delete_node', { id });
      console.log('[TauriNodeService] Deleted node:', id);
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to delete node:', id, err);
      throw new NodeOperationError(err.message, id, 'delete');
    }
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

    try {
      const children = await universalInvoke<Node[]>('get_children', { parentId });
      return children;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to get children:', parentId, err);
      throw new NodeOperationError(err.message, parentId, 'getChildren');
    }
  }

  /**
   * Bulk fetch all nodes belonging to an origin node (viewer/page)
   *
   * This is the efficient way to load a complete document tree:
   * - Single database query fetches all nodes with the same origin_node_id
   * - Frontend can reconstruct hierarchy using parent_id and before_sibling_id
   *
   * @param originNodeId - ID of the origin node (e.g., date page ID like "2025-10-05")
   * @returns Array of all nodes belonging to this origin
   * @throws NodeOperationError if operation fails
   */
  async getNodesByOriginId(originNodeId: string): Promise<Node[]> {
    this.ensureInitialized();

    try {
      const nodes = await universalInvoke<Node[]>('get_nodes_by_origin_id', { originNodeId });
      return nodes;
    } catch (error) {
      const err = toError(error);
      console.error('[TauriNodeService] Failed to get nodes by origin ID:', originNodeId, err);
      throw new NodeOperationError(err.message, originNodeId, 'getNodesByOriginId');
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
      originNodeId: string;
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
        originNodeId: data.originNodeId,
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
