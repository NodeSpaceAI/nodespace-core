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
import { eventBus } from './event-bus';
import type {
  NodeCreatedEvent,
  NodeUpdatedEvent,
  NodeDeletedEvent,
  HierarchyChangedEvent
} from './event-types';
import type {
  SchemaDefinition,
  AddFieldConfig,
  AddFieldResult,
  RemoveFieldResult,
  ExtendEnumResult,
  RemoveEnumValueResult
} from '$lib/types/schema';

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

// Phase 3: Embedding-related types

/**
 * Search parameters for topic similarity search
 *
 * @example
 * ```typescript
 * const params: SearchContainersParams = {
 *   query: "machine learning",
 *   threshold: 0.7,
 *   limit: 20,
 *   exact: false
 * };
 * const results = await adapter.searchContainers(params);
 * ```
 */
export interface SearchContainersParams {
  query: string;
  threshold?: number;
  limit?: number;
  exact?: boolean;
}

/**
 * Result of batch embedding generation
 *
 * @example
 * ```typescript
 * const result: BatchEmbeddingResult = await adapter.batchGenerateEmbeddings([
 *   "topic-1",
 *   "topic-2",
 *   "topic-3"
 * ]);
 * console.log(`Successfully embedded ${result.successCount} topics`);
 * if (result.failedEmbeddings.length > 0) {
 *   console.error("Failed embeddings:", result.failedEmbeddings);
 * }
 * ```
 */
export interface BatchEmbeddingResult {
  successCount: number;
  failedEmbeddings: Array<{
    containerId: string;
    error: string;
  }>;
}

/**
 * Input for creating a container node (root node with no parent)
 *
 * @example
 * ```typescript
 * const input: CreateContainerNodeInput = {
 *   content: "Project Planning",
 *   nodeType: "topic",
 *   properties: { priority: "high", status: "active" },
 *   mentionedBy: "user-node-123"
 * };
 * const containerId = await adapter.createContainerNode(input);
 * ```
 */
export interface CreateContainerNodeInput {
  content: string;
  nodeType: string;
  properties?: Record<string, unknown>;
  mentionedBy?: string;
}

/**
 * Backend adapter interface - Phase 1, 2, and 3 operations
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
   * @param node - Node data without timestamps or version (backend sets these)
   * @returns ID of the created node
   */
  createNode(node: Omit<Node, 'createdAt' | 'modifiedAt' | 'version'>): Promise<string>;

  /**
   * Get a node by ID
   * @param id - Node ID
   * @returns Node data if found, null otherwise
   */
  getNode(id: string): Promise<Node | null>;

  /**
   * Update an existing node
   * @param id - Node ID
   * @param version - Expected version for optimistic concurrency control
   * @param update - Fields to update (partial)
   * @returns Updated node with new version number
   * @throws Version conflict error if version doesn't match current version
   */
  updateNode(id: string, version: number, update: NodeUpdate): Promise<Node>;

  /**
   * Delete a node by ID
   * @param id - Node ID
   * @param version - Expected version for optimistic concurrency control
   * @throws Version conflict error if version doesn't match current version
   */
  deleteNode(id: string, version: number): Promise<void>;

  /**
   * Get child nodes of a parent node
   * @param parentId - Parent node ID
   * @returns Array of child nodes
   */
  getChildren(parentId: string): Promise<Node[]>;

  /**
   * Set parent relationship for a node (establishes has_child graph edge)
   * @param nodeId - Child node ID
   * @param parentId - New parent ID (null to make node a root)
   */
  setParent(nodeId: string, parentId: string | null): Promise<void>;

  /**
   * Query nodes by parent and/or container (Phase 2)
   * @since Phase 2
   * @param params - Query parameters
   * @returns Array of matching nodes
   */
  queryNodes(params: QueryNodesParams): Promise<Node[]>;

  /**
   * Get nodes by container ID (Phase 2)
   * @since Phase 2
   * @param containerId - Container node ID (e.g., date string like "2025-10-13")
   * @returns Array of nodes belonging to this container
   */
  getNodesByContainerId(containerId: string): Promise<Node[]>;

  /**
   * Mention autocomplete - specialized query for @mention feature
   * @since Phase 2
   * @param query - Search query string
   * @param limit - Maximum number of results (default: 10)
   * @returns Array of matching nodes (tasks and containers only)
   */
  mentionAutocomplete(query: string, limit?: number): Promise<Node[]>;

  // === Phase 3: Embedding Operations ===

  /**
   * Generate embedding for a topic node
   * @param containerId - ID of the topic node to embed
   * @throws {NodeOperationError} If embedding generation fails
   */
  generateContainerEmbedding(containerId: string): Promise<void>;

  /**
   * Search topics by semantic similarity
   * @param params - Search parameters (query, threshold, limit, exact)
   * @returns Array of matching topic nodes sorted by similarity score (highest first)
   * @throws {NodeOperationError} If search operation fails
   */
  searchContainers(params: SearchContainersParams): Promise<Node[]>;

  /**
   * Update topic embedding immediately
   * @param containerId - ID of the topic to update
   * @throws {NodeOperationError} If embedding update fails
   */
  updateContainerEmbedding(containerId: string): Promise<void>;

  /**
   * Batch generate embeddings for multiple topics
   * @param containerIds - Array of topic IDs to embed
   * @returns Result object containing success count and array of failed embeddings with error details
   * @throws {NodeOperationError} If batch operation fails completely
   */
  batchGenerateEmbeddings(containerIds: string[]): Promise<BatchEmbeddingResult>;

  /**
   * Get count of stale topics needing re-embedding
   * @returns Number of topics that have been modified but not yet re-embedded
   * @throws {NodeOperationError} If count retrieval fails
   */
  getStaleContainerCount(): Promise<number>;

  /**
   * Smart trigger: Topic closed/unfocused
   * @param containerId - ID of the topic that was closed
   * @throws {NodeOperationError} If trigger operation fails
   */
  onContainerClosed(containerId: string): Promise<void>;

  /**
   * Smart trigger: Idle timeout (30s of no activity)
   * @param containerId - ID of the topic to check
   * @returns True if re-embedding was triggered, false if topic was not stale
   * @throws {NodeOperationError} If trigger operation fails
   */
  onContainerIdle(containerId: string): Promise<boolean>;

  /**
   * Manually sync all stale topics
   * @returns Number of topics successfully re-embedded during the sync operation
   * @throws {NodeOperationError} If sync operation fails
   */
  syncEmbeddings(): Promise<number>;

  // === Phase 3: Node Mention Operations ===

  /**
   * Create a container node (root node with no parent)
   * @param input - Container node data
   * @returns ID of the created container node
   * @throws {NodeOperationError} If container node creation fails
   */
  createContainerNode(input: CreateContainerNodeInput): Promise<string>;

  /**
   * Create a mention relationship between two nodes
   * @param mentioningNodeId - ID of the node that contains the mention
   * @param mentionedNodeId - ID of the node being mentioned
   * @throws {NodeOperationError} If mention creation fails
   */
  createNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;

  /**
   * Get outgoing mentions (nodes that this node mentions)
   * @param nodeId - ID of the node to query
   * @returns List of mentioned node IDs (empty if no mentions)
   * @throws {NodeOperationError} If query fails
   */
  getOutgoingMentions(nodeId: string): Promise<string[]>;

  /**
   * Get incoming mentions (nodes that mention this node - BACKLINKS)
   * @param nodeId - ID of the node to query for backlinks
   * @returns List of node IDs that mention this node (empty if no backlinks)
   * @throws {NodeOperationError} If query fails
   */
  getIncomingMentions(nodeId: string): Promise<string[]>;

  /**
   * Get containers of nodes that mention this node (backlinks at container level)
   *
   * Unlike `getIncomingMentions` which returns individual mentioning nodes,
   * this resolves to their container nodes and deduplicates automatically.
   *
   * Example: If nodes A and B (both children of Container X) mention this node,
   * returns `['container-x-id']` instead of `['node-a-id', 'node-b-id']`
   *
   * Exceptions: Task and ai-chat nodes are treated as their own containers.
   *
   * @param nodeId - ID of the node to query for backlinks
   * @returns List of unique container node IDs (empty if no backlinks)
   * @throws {NodeOperationError} If query fails
   */
  getMentioningContainers(nodeId: string): Promise<string[]>;

  /**
   * Delete a mention relationship between two nodes
   * @param mentioningNodeId - ID of the node that contains the mention
   * @param mentionedNodeId - ID of the node being mentioned
   * @throws {NodeOperationError} If deletion fails
   */
  deleteNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;

  // === Schema Management Operations ===

  /**
   * Get all schema definitions
   * @returns Array of all schema definitions (both core and custom)
   * @throws {NodeOperationError} If retrieval fails
   */
  getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>>;

  /**
   * Get a schema definition by schema ID
   * @param schemaId - Schema ID (e.g., "task", "person")
   * @returns Complete schema definition with fields and metadata
   * @throws {NodeOperationError} If schema not found
   */
  getSchema(schemaId: string): Promise<SchemaDefinition>;

  /**
   * Add a new field to a schema
   * @param schemaId - Schema ID to modify
   * @param config - Field configuration
   * @returns Result with new schema version
   * @throws {NodeOperationError} If field already exists or validation fails
   */
  addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult>;

  /**
   * Remove a field from a schema (user fields only)
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the field to remove
   * @returns Result with new schema version
   * @throws {NodeOperationError} If field is protected or not found
   */
  removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult>;

  /**
   * Extend an enum field with a new value
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the enum field
   * @param value - Value to add to user_values
   * @returns Result with new schema version
   * @throws {NodeOperationError} If field is not extensible or value exists
   */
  extendSchemaEnum(schemaId: string, fieldName: string, value: string): Promise<ExtendEnumResult>;

  /**
   * Remove a value from an enum field (user values only)
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the enum field
   * @param value - Value to remove from user_values
   * @returns Result with new schema version
   * @throws {NodeOperationError} If value is a core value or not found
   */
  removeSchemaEnumValue(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<RemoveEnumValueResult>;
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

  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt' | 'version'>): Promise<string> {
    try {
      const nodeId = await invoke<string>('create_node', { node });

      // Emit node:created event
      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'TauriAdapter',
        nodeId: nodeId,
        nodeType: node.nodeType
      });

      return nodeId;
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

  async updateNode(id: string, version: number, update: NodeUpdate): Promise<Node> {
    try {
      // IMPORTANT: Backend now returns the updated Node with new version
      const updatedNode = await invoke<Node>('update_node', { id, version, update });

      // Determine update type based on what fields were updated
      let updateType: 'content' | 'hierarchy' | 'status' | 'metadata' | 'nodeType' = 'content';
      const affectedNodes: string[] = [id];

      if ('content' in update) {
        updateType = 'content';
      } else if ('beforeSiblingId' in update) {
        updateType = 'hierarchy';
        // Track affected nodes for hierarchy changes (only if not null)
        if (update.beforeSiblingId !== undefined && update.beforeSiblingId !== null) {
          affectedNodes.push(update.beforeSiblingId);
        }
      } else if ('nodeType' in update) {
        updateType = 'nodeType';
      }

      // Emit node:updated event
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'TauriAdapter',
        nodeId: id,
        updateType: updateType
      });

      // Emit hierarchy:changed event for structural changes
      if (updateType === 'hierarchy') {
        eventBus.emit<HierarchyChangedEvent>({
          type: 'hierarchy:changed',
          namespace: 'lifecycle',
          source: 'TauriAdapter',
          affectedNodes: affectedNodes,
          changeType: 'move'
        });
      }

      return updatedNode;
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'update');
    }
  }

  async deleteNode(id: string, version: number): Promise<void> {
    try {
      // Fetch node before deletion to get metadata for events
      // If node doesn't exist, getNode returns null but doesn't throw
      const nodeBeforeDeletion = await this.getNode(id);

      await invoke<void>('delete_node', { id, version });

      // Only emit events if node existed (successful deletion)
      // Note: This handles the backend's non-idempotent DELETE behavior
      // where attempting to delete a non-existent node returns 500 error
      // Related: Issue #219 - Backend DELETE should be idempotent
      if (nodeBeforeDeletion) {
        // Emit node:deleted event
        eventBus.emit<NodeDeletedEvent>({
          type: 'node:deleted',
          namespace: 'lifecycle',
          source: 'TauriAdapter',
          nodeId: id
        });
      }
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

  async setParent(nodeId: string, parentId: string | null): Promise<void> {
    try {
      await invoke('set_parent', { nodeId, parentId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'setParent');
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

  async getNodesByContainerId(containerId: string): Promise<Node[]> {
    try {
      return await invoke<Node[]>('get_nodes_by_container_id', { containerNodeId: containerId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'getNodesByContainerId');
    }
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    try {
      return await invoke<Node[]>('mention_autocomplete', { query, limit });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, query, 'mentionAutocomplete');
    }
  }

  // === Phase 3: Embedding Operations ===

  async generateContainerEmbedding(containerId: string): Promise<void> {
    try {
      await invoke<void>('generate_topic_embedding', { containerId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'generateContainerEmbedding');
    }
  }

  async searchContainers(params: SearchContainersParams): Promise<Node[]> {
    try {
      return await invoke<Node[]>('search_topics', { params });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, params.query, 'searchContainers');
    }
  }

  async updateContainerEmbedding(containerId: string): Promise<void> {
    try {
      await invoke<void>('update_topic_embedding', { containerId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'updateContainerEmbedding');
    }
  }

  async batchGenerateEmbeddings(containerIds: string[]): Promise<BatchEmbeddingResult> {
    try {
      return await invoke<BatchEmbeddingResult>('batch_generate_embeddings', { containerIds });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerIds.join(','), 'batchGenerateEmbeddings');
    }
  }

  async getStaleContainerCount(): Promise<number> {
    try {
      return await invoke<number>('get_stale_topic_count');
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, '', 'getStaleContainerCount');
    }
  }

  async onContainerClosed(containerId: string): Promise<void> {
    try {
      await invoke<void>('on_topic_closed', { containerId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'onContainerClosed');
    }
  }

  async onContainerIdle(containerId: string): Promise<boolean> {
    try {
      return await invoke<boolean>('on_topic_idle', { containerId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'onContainerIdle');
    }
  }

  async syncEmbeddings(): Promise<number> {
    try {
      return await invoke<number>('sync_embeddings');
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, '', 'syncEmbeddings');
    }
  }

  // === Phase 3: Node Mention Operations ===

  async createContainerNode(input: CreateContainerNodeInput): Promise<string> {
    try {
      return await invoke<string>('create_container_node', { input });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, input.content, 'createContainerNode');
    }
  }

  async createNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    try {
      await invoke<void>('create_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(
        err.message,
        `${mentioningNodeId} -> ${mentionedNodeId}`,
        'createNodeMention'
      );
    }
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    try {
      return await invoke<string[]>('get_outgoing_mentions', { nodeId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getOutgoingMentions');
    }
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    try {
      return await invoke<string[]>('get_incoming_mentions', { nodeId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getIncomingMentions');
    }
  }

  async getMentioningContainers(nodeId: string): Promise<string[]> {
    try {
      return await invoke<string[]>('get_mentioning_containers', { nodeId });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getMentioningContainers');
    }
  }

  async deleteNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    try {
      await invoke<void>('delete_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      });
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(
        err.message,
        `${mentioningNodeId} -> ${mentionedNodeId}`,
        'deleteNodeMention'
      );
    }
  }

  // === Schema Management Operations ===

  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    try {
      // Call Tauri command to get all schemas
      const schemas = await invoke<Array<{
        id: string;
        is_core: boolean;
        version: number;
        description: string;
        fields: Array<{
          name: string;
          type: string;
          protection: string;
          core_values?: string[];
          user_values?: string[];
          indexed: boolean;
          required?: boolean;
          extensible?: boolean;
          default?: unknown;
          description?: string;
          item_type?: string;
        }>;
      }>>('get_all_schemas');

      // Convert from snake_case (Rust) to camelCase (TypeScript)
      return schemas.map((schema) => ({
        id: schema.id,
        isCore: schema.is_core,
        version: schema.version,
        description: schema.description,
        fields: schema.fields.map((field) => ({
          name: field.name,
          type: field.type,
          protection: field.protection as 'core' | 'user' | 'system',
          coreValues: field.core_values,
          userValues: field.user_values,
          indexed: field.indexed,
          required: field.required,
          extensible: field.extensible,
          default: field.default,
          description: field.description,
          itemType: field.item_type
        }))
      }));
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, 'all', 'getAllSchemas');
    }
  }

  async getSchema(schemaId: string): Promise<SchemaDefinition> {
    try {
      // Call actual Tauri command (created by #388)
      // Note: Rust uses #[serde(rename = "type")] so field_type is already renamed to "type"
      const schema = await invoke<{
        is_core: boolean;
        version: number;
        description: string;
        fields: Array<{
          name: string;
          type: string; // Already renamed by Rust's #[serde(rename = "type")]
          protection: string;
          core_values?: string[];
          user_values?: string[];
          indexed: boolean;
          required?: boolean;
          extensible?: boolean;
          default?: unknown;
          description?: string;
          item_type?: string;
        }>;
      }>('get_schema_definition', {
        schemaId
      });

      // Convert from snake_case (Rust) to camelCase (TypeScript)
      return {
        isCore: schema.is_core,
        version: schema.version,
        description: schema.description,
        fields: schema.fields.map((field) => ({
          name: field.name,
          type: field.type, // Use renamed field directly
          protection: field.protection as 'core' | 'user' | 'system',
          coreValues: field.core_values,
          userValues: field.user_values,
          indexed: field.indexed,
          required: field.required,
          extensible: field.extensible,
          default: field.default,
          description: field.description,
          itemType: field.item_type
        }))
      };
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, schemaId, 'getSchema');
    }
  }

  async addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult> {
    try {
      // Call actual Tauri command (created by #388)
      const result = await invoke<{
        schema_id: string;
        new_version: number;
      }>('add_schema_field', {
        schemaId,
        field: {
          name: config.fieldName,
          fieldType: config.fieldType,
          indexed: config.indexed ?? false,
          required: config.required,
          default: config.default,
          description: config.description,
          itemType: config.itemType,
          enumValues: config.enumValues,
          extensible: config.extensible
        }
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: true
      };
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, schemaId, 'addSchemaField');
    }
  }

  async removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult> {
    try {
      // Call actual Tauri command (created by #388)
      const result = await invoke<{
        schema_id: string;
        new_version: number;
      }>('remove_schema_field', {
        schemaId,
        fieldName
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: true
      };
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, schemaId, 'removeSchemaField');
    }
  }

  async extendSchemaEnum(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<ExtendEnumResult> {
    try {
      // Call actual Tauri command (created by #388)
      const result = await invoke<{
        schema_id: string;
        new_version: number;
      }>('extend_schema_enum', {
        schemaId,
        fieldName,
        value
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: true
      };
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, schemaId, 'extendSchemaEnum');
    }
  }

  async removeSchemaEnumValue(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<RemoveEnumValueResult> {
    try {
      // Call actual Tauri command (created by #388)
      const result = await invoke<{
        schema_id: string;
        new_version: number;
      }>('remove_schema_enum_value', {
        schemaId,
        fieldName,
        value
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: true
      };
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, schemaId, 'removeSchemaEnumValue');
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
      // Try to parse error as ApiError format (from dev-proxy)
      // ApiError structure: { message: string, code: string, details?: string }
      try {
        const errorData = await response.json();
        // Use message field from ApiError response
        const errorMessage = errorData.message || `HTTP ${response.status}: ${response.statusText}`;
        throw new Error(errorMessage);
      } catch (parseError) {
        // If JSON parsing fails, fall back to generic HTTP error
        // Note: Don't catch the Error we just threw above - only catch JSON parse errors
        if (parseError instanceof SyntaxError) {
          throw new Error(`HTTP ${response.status}: ${response.statusText}`);
        }
        // Re-throw our constructed error with the message from errorData
        throw parseError;
      }
    }

    // Handle 204 No Content responses
    if (response.status === 204 || response.headers.get('content-length') === '0') {
      return undefined as T;
    }

    return await response.json();
  }

  /**
   * Retries a fetch operation with exponential backoff on HTTP 500 errors.
   *
   * Used to handle transient SQLite "database is locked" errors from dev-server
   * during high concurrency (parallel tests). These errors are test infrastructure
   * artifacts and don't reflect production behavior (Tauri IPC, single-user, human-speed).
   *
   * @param operation - The fetch operation to retry
   * @param maxRetries - Maximum retry attempts (default: 3)
   * @param baseDelayMs - Base delay for exponential backoff (default: 50ms)
   * @returns The result of the operation
   * @throws The last error if all retries exhausted
   */
  private async retryOnTransientError<T>(
    operation: () => Promise<T>,
    maxRetries: number = 3,
    baseDelayMs: number = 50
  ): Promise<T> {
    let lastError: Error | undefined;

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        return await operation();
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));

        // Only retry on HTTP 500 errors (SQLite "database is locked")
        // Don't retry validation errors (400), not found (404), etc.
        const isTransientError =
          lastError.message.includes('500') || lastError.message.includes('database is locked');

        if (!isTransientError || attempt === maxRetries) {
          throw lastError;
        }

        // Exponential backoff: 50ms, 100ms, 200ms
        const delayMs = baseDelayMs * Math.pow(2, attempt - 1);
        await new Promise((resolve) => setTimeout(resolve, delayMs));
      }
    }

    throw lastError!;
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

  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt' | 'version'> & { _parentId?: string }): Promise<string> {
    try {
      // Wrap the HTTP write operation with retry logic
      const nodeId = await this.retryOnTransientError(async () => {
        // CRITICAL FIX (Issue #528): Extract transient _parentId field and send to backend
        // The _parentId field is added by promotePlaceholderToNode() and needs to be
        // passed to the backend so it can create parent-child edges
        // Note (Issue #533): _containerId removed - backend auto-derives root from parent chain
        const requestBody: Record<string, unknown> = {
          id: node.id,
          nodeType: node.nodeType,
          content: node.content,
          beforeSiblingId: node.beforeSiblingId,
          properties: node.properties,
          mentions: node.mentions
        };

        // Add parent field if present (for promoted placeholders)
        if ((node as { _parentId?: string })._parentId) {
          requestBody.parentId = (node as { _parentId?: string })._parentId;
        }

        const response = await globalThis.fetch(`${this.baseUrl}/api/nodes`, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json'
          },
          body: JSON.stringify(requestBody)
        });

        return await this.handleResponse<string>(response);
      });

      // Emit node:created event
      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'HttpAdapter',
        nodeId: nodeId,
        nodeType: node.nodeType
      });

      return nodeId;
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

  async updateNode(id: string, version: number, update: NodeUpdate): Promise<Node> {
    try {
      // Wrap the HTTP write operation with retry logic
      // IMPORTANT: Backend now returns the updated Node with new version
      const updatedNode = await this.retryOnTransientError(async () => {
        const response = await globalThis.fetch(
          `${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`,
          {
            method: 'PATCH',
            headers: {
              'Content-Type': 'application/json'
            },
            body: JSON.stringify({ ...update, version })
          }
        );

        return await this.handleResponse<Node>(response);
      });

      // Determine update type based on what fields were updated
      let updateType: 'content' | 'hierarchy' | 'status' | 'metadata' | 'nodeType' = 'content';
      const affectedNodes: string[] = [id];

      if ('content' in update) {
        updateType = 'content';
      } else if ('beforeSiblingId' in update) {
        updateType = 'hierarchy';
        // Track affected nodes for hierarchy changes (only if not null)
        if (update.beforeSiblingId !== undefined && update.beforeSiblingId !== null) {
          affectedNodes.push(update.beforeSiblingId);
        }
      } else if ('nodeType' in update) {
        updateType = 'nodeType';
      }

      // Emit node:updated event
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'HttpAdapter',
        nodeId: id,
        updateType: updateType
      });

      // Emit hierarchy:changed event for structural changes
      if (updateType === 'hierarchy') {
        eventBus.emit<HierarchyChangedEvent>({
          type: 'hierarchy:changed',
          namespace: 'lifecycle',
          source: 'HttpAdapter',
          affectedNodes: affectedNodes,
          changeType: 'move'
        });
      }

      return updatedNode;
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, id, 'update');
    }
  }

  async deleteNode(id: string, version: number): Promise<void> {
    try {
      // Fetch node before deletion to get metadata for events
      // If node doesn't exist, getNode returns null but doesn't throw
      const nodeBeforeDeletion = await this.getNode(id);

      // Wrap the HTTP write operation with retry logic
      await this.retryOnTransientError(async () => {
        const response = await globalThis.fetch(
          `${this.baseUrl}/api/nodes/${encodeURIComponent(id)}`,
          {
            method: 'DELETE',
            headers: {
              'Content-Type': 'application/json'
            },
            body: JSON.stringify({ version })
          }
        );

        return await this.handleResponse<void>(response);
      });

      // Only emit events if node existed (successful deletion)
      // Note: This handles the backend's non-idempotent DELETE behavior
      // where attempting to delete a non-existent node returns 500 error
      // Related: Issue #219 - Backend DELETE should be idempotent
      if (nodeBeforeDeletion) {
        // Emit node:deleted event
        eventBus.emit<NodeDeletedEvent>({
          type: 'node:deleted',
          namespace: 'lifecycle',
          source: 'HttpAdapter',
          nodeId: id
        });
      }
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

  async setParent(nodeId: string, parentId: string | null): Promise<void> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/parent`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json'
          },
          body: JSON.stringify({ parentId })
        }
      );
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}: ${response.statusText}`);
      }
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'setParent');
    }
  }

  async queryNodes(params: QueryNodesParams): Promise<Node[]> {
    try {
      // dev-proxy expects POST /api/query with JSON body (NodeFilter)
      // NOT GET /api/nodes/query with query params
      const response = await globalThis.fetch(`${this.baseUrl}/api/query`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify(params)
      });
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, params.parentId ?? 'query', 'queryNodes');
    }
  }

  async getNodesByContainerId(containerId: string): Promise<Node[]> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/by-container/${encodeURIComponent(containerId)}`
      );
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'getNodesByContainerId');
    }
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/mentions/autocomplete`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ query, limit })
      });
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, query, 'mentionAutocomplete');
    }
  }

  // === Phase 3: Embedding Operations ===

  /**
   * Fetch with automatic timeout and error handling
   *
   * @param url - The URL to fetch
   * @param options - Fetch options (method, headers, body, etc.)
   * @param operationName - Name of the operation (for error messages)
   * @param contextId - Context identifier for error tracking
   * @returns Response object
   * @throws {NodeOperationError} If request times out or fails
   * @private
   */
  private async fetchWithTimeout(
    url: string,
    options: RequestInit,
    operationName: string,
    contextId: string = ''
  ): Promise<Response> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 30000); // 30s timeout

    try {
      const response = await globalThis.fetch(url, {
        ...options,
        signal: controller.signal
      });
      return response;
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new NodeOperationError(
          `${operationName} timeout - operation took too long`,
          contextId,
          operationName
        );
      }
      throw error; // Re-throw for caller to handle
    } finally {
      clearTimeout(timeoutId); // Always cleanup
    }
  }

  async generateContainerEmbedding(containerId: string): Promise<void> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/generate`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ containerId })
        },
        'Embedding generation',
        containerId
      );
      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'generateContainerEmbedding');
    }
  }

  async searchContainers(params: SearchContainersParams): Promise<Node[]> {
    // Validate query parameter
    if (!params.query || params.query.trim() === '') {
      throw new NodeOperationError('Search query cannot be empty', params.query, 'searchContainers');
    }

    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/search`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(params)
        },
        'Search',
        params.query
      );
      return await this.handleResponse<Node[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, params.query, 'searchContainers');
    }
  }

  async updateContainerEmbedding(containerId: string): Promise<void> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/${encodeURIComponent(containerId)}`,
        {
          method: 'PATCH'
        },
        'Embedding update',
        containerId
      );
      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'updateContainerEmbedding');
    }
  }

  async batchGenerateEmbeddings(containerIds: string[]): Promise<BatchEmbeddingResult> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/batch`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ containerIds })
        },
        'Batch embedding',
        'batch'
      );
      return await this.handleResponse<BatchEmbeddingResult>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, 'batch', 'batchGenerateEmbeddings');
    }
  }

  async getStaleContainerCount(): Promise<number> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/embeddings/stale-count`);
      return await this.handleResponse<number>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, 'stale-count', 'getStaleContainerCount');
    }
  }

  async onContainerClosed(containerId: string): Promise<void> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/on-topic-closed`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ containerId })
        },
        'Topic closed trigger',
        containerId
      );
      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'onContainerClosed');
    }
  }

  async onContainerIdle(containerId: string): Promise<boolean> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/on-topic-idle`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ containerId })
        },
        'Topic idle trigger',
        containerId
      );
      return await this.handleResponse<boolean>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, containerId, 'onContainerIdle');
    }
  }

  async syncEmbeddings(): Promise<number> {
    try {
      const response = await this.fetchWithTimeout(
        `${this.baseUrl}/api/embeddings/sync`,
        {
          method: 'POST'
        },
        'Embeddings sync',
        'sync'
      );
      return await this.handleResponse<number>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, 'sync', 'syncEmbeddings');
    }
  }

  // === Phase 3: Node Mention Operations ===

  async createContainerNode(input: CreateContainerNodeInput): Promise<string> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/nodes/container`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(input)
      });
      return await this.handleResponse<string>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, input.content, 'createContainerNode');
    }
  }

  async createNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/nodes/mention`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mentioningNodeId, mentionedNodeId })
      });
      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(
        err.message,
        `${mentioningNodeId} -> ${mentionedNodeId}`,
        'createNodeMention'
      );
    }
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/outgoing-mentions`,
        {
          method: 'GET'
        }
      );
      return await this.handleResponse<string[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getOutgoingMentions');
    }
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/incoming-mentions`,
        {
          method: 'GET'
        }
      );
      return await this.handleResponse<string[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getIncomingMentions');
    }
  }

  async getMentioningContainers(nodeId: string): Promise<string[]> {
    try {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/nodes/${encodeURIComponent(nodeId)}/mentions/containers`,
        {
          method: 'GET'
        }
      );
      return await this.handleResponse<string[]>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(err.message, nodeId, 'getMentioningContainers');
    }
  }

  async deleteNodeMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    try {
      const response = await globalThis.fetch(`${this.baseUrl}/api/nodes/mention`, {
        method: 'DELETE',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mentioningNodeId, mentionedNodeId })
      });
      await this.handleResponse<void>(response);
    } catch (error) {
      const err = toError(error);
      throw new NodeOperationError(
        err.message,
        `${mentioningNodeId} -> ${mentionedNodeId}`,
        'deleteNodeMention'
      );
    }
  }

  // === Schema Management Operations ===
  //
  // NOTE: Schema operations use retryOnTransientError wrapper to handle SQLite write lock contention.
  // This is consistent with the established pattern for mutation operations (see node and embedding endpoints).
  // Read operations also use retry for consistency, though they're less likely to encounter lock errors.

  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(`${this.baseUrl}/api/schemas`, {
        method: 'GET',
        headers: { 'Content-Type': 'application/json' }
      });

      const schemas = await this.handleResponse<Array<{
        id: string;
        is_core: boolean;
        version: number;
        description: string;
        fields: Array<{
          name: string;
          type: string;
          protection: string;
          core_values?: string[];
          user_values?: string[];
          indexed: boolean;
          required?: boolean;
          extensible?: boolean;
          default?: unknown;
          description?: string;
          item_type?: string;
        }>;
      }>>(response);

      // Convert from snake_case (HTTP response) to camelCase (TypeScript)
      return schemas.map((schema) => ({
        id: schema.id,
        isCore: schema.is_core,
        version: schema.version,
        description: schema.description,
        fields: schema.fields.map((field) => ({
          name: field.name,
          type: field.type,
          protection: field.protection as 'core' | 'user' | 'system',
          coreValues: field.core_values,
          userValues: field.user_values,
          indexed: field.indexed,
          required: field.required,
          extensible: field.extensible,
          default: field.default,
          description: field.description,
          itemType: field.item_type
        }))
      }));
    });
  }

  async getSchema(schemaId: string): Promise<SchemaDefinition> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}`,
        {
          method: 'GET',
          headers: { 'Content-Type': 'application/json' }
        }
      );

      const schema = await this.handleResponse<{
        is_core: boolean;
        version: number;
        description: string;
        fields: Array<{
          name: string;
          type: string;
          protection: string;
          core_values?: string[];
          user_values?: string[];
          indexed: boolean;
          required?: boolean;
          extensible?: boolean;
          default?: unknown;
          description?: string;
          item_type?: string;
        }>;
      }>(response);

      // Convert from snake_case (stored in DB) to camelCase (TypeScript)
      return {
        isCore: schema.is_core,
        version: schema.version,
        description: schema.description,
        fields: schema.fields.map((field) => ({
          name: field.name,
          type: field.type,
          protection: field.protection as 'core' | 'user' | 'system',
          coreValues: field.core_values,
          userValues: field.user_values,
          indexed: field.indexed,
          required: field.required,
          extensible: field.extensible,
          default: field.default,
          description: field.description,
          itemType: field.item_type
        }))
      };
    });
  }

  async addSchemaField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(config)
        }
      );

      return await this.handleResponse<AddFieldResult>(response);
    });
  }

  async removeSchemaField(schemaId: string, fieldName: string): Promise<RemoveFieldResult> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}`,
        {
          method: 'DELETE',
          headers: { 'Content-Type': 'application/json' }
        }
      );

      return await this.handleResponse<RemoveFieldResult>(response);
    });
  }

  async extendSchemaEnum(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<ExtendEnumResult> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}/enum-values`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ value })
        }
      );

      return await this.handleResponse<ExtendEnumResult>(response);
    });
  }

  async removeSchemaEnumValue(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<RemoveEnumValueResult> {
    return await this.retryOnTransientError(async () => {
      const response = await globalThis.fetch(
        `${this.baseUrl}/api/schemas/${encodeURIComponent(schemaId)}/fields/${encodeURIComponent(fieldName)}/enum-values/${encodeURIComponent(value)}`,
        {
          method: 'DELETE',
          headers: { 'Content-Type': 'application/json' }
        }
      );

      return await this.handleResponse<RemoveEnumValueResult>(response);
    });
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
