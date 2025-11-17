/**
 * Mock Backend Adapter for Unit Testing
 *
 * Pure in-memory implementation of BackendAdapter interface.
 * No HTTP, no database, no async complexity - just fast, synchronous operations.
 *
 * Purpose: Enable 100% reliable unit testing of sibling chain logic without
 * the flakiness of HTTP/SQLite infrastructure.
 *
 * Key Benefits:
 * - 100% deterministic (no timing/concurrency issues)
 * - 10x faster than HTTP adapter (~0.5s vs ~5s per test)
 * - Zero infrastructure dependencies
 * - Perfect for testing business logic in isolation
 *
 * Usage:
 * ```typescript
 * const adapter = new MockBackendAdapter();
 * const nodeId = await adapter.createNode({ ... });
 * const node = await adapter.getNode(nodeId);
 * ```
 */

import type {
  BackendAdapter,
  QueryNodesParams,
  SearchContainersParams,
  BatchEmbeddingResult,
  CreateContainerNodeInput
} from '$lib/services/backend-adapter';
import type { Node, NodeUpdate } from '$lib/types';
import type {
  SchemaDefinition,
  AddFieldConfig,
  AddFieldResult,
  RemoveFieldResult,
  ExtendEnumResult,
  RemoveEnumValueResult
} from '$lib/types/schema';

/**
 * Mock implementation of BackendAdapter using in-memory Map storage
 */
export class MockBackendAdapter implements BackendAdapter {
  private nodes: Map<string, Node> = new Map();
  private dbPath: string = ':memory:';

  /**
   * Initialize mock database (no-op, always succeeds)
   */
  async initializeDatabase(dbPath?: string): Promise<string> {
    this.dbPath = dbPath ?? ':memory:';
    return this.dbPath;
  }

  /**
   * Create a node in memory
   */
  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt' | 'version'>): Promise<string> {
    const now = new Date().toISOString();
    const fullNode: Node = {
      ...node,
      version: 1,
      createdAt: now,
      modifiedAt: now
    };
    this.nodes.set(node.id, fullNode);
    return node.id;
  }

  /**
   * Get a node from memory
   */
  async getNode(id: string): Promise<Node | null> {
    return this.nodes.get(id) ?? null;
  }

  /**
   * Update a node in memory
   */
  async updateNode(id: string, version: number, update: NodeUpdate): Promise<Node> {
    const existingNode = this.nodes.get(id);
    if (!existingNode) {
      throw new Error(`Node ${id} not found`);
    }

    // Check version for optimistic concurrency control
    if (existingNode.version !== version) {
      throw new Error(
        `Version conflict: expected version ${version}, but current version is ${existingNode.version}`
      );
    }

    const updatedNode: Node = {
      ...existingNode,
      ...update,
      modifiedAt: new Date().toISOString(),
      version: existingNode.version + 1
    };

    this.nodes.set(id, updatedNode);
    return updatedNode; // Return the updated node
  }

  /**
   * Delete a node from memory
   */
  async deleteNode(id: string): Promise<void> {
    const existed = this.nodes.delete(id);
    if (!existed) {
      throw new Error(`Node ${id} not found`);
    }
  }

  /**
   * Set parent relationship (mock - does nothing)
   * NOTE: Mock adapter doesn't track hierarchy
   */
  async setParent(nodeId: string, _parentId: string | null): Promise<void> {
    // Mock implementation - verify node exists
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }
    // In real implementation, this would create has_child edge
    // Mock just validates the operation would succeed
  }

  /**
   * Get child nodes of a parent node
   * NOTE: Since we removed parentId, this now returns empty array
   * Mock adapter doesn't track hierarchy
   */
  async getChildren(_parentId: string): Promise<Node[]> {
    // No longer supported - return empty array
    return [];
  }

  /**
   * Query nodes by container
   * NOTE: Container concept simplified in mock adapter - returns all nodes
   */
  async queryNodes(_params: QueryNodesParams): Promise<Node[]> {
    // Simplified: return all nodes since we don't have container concept in mock
    return Array.from(this.nodes.values());
  }

  /**
   * Get nodes by container ID
   * NOTE: Container concept not supported in mock adapter
   */
  async getNodesByContainerId(_containerId: string): Promise<Node[]> {
    // No longer supported - return empty array
    return [];
  }

  /**
   * Mention autocomplete mock - filters for tasks and containers (excluding schemas)
   */
  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    const lowerQuery = query.toLowerCase();
    const results = Array.from(this.nodes.values()).filter((node) => {
      // Exclude date and schema nodes
      if (node.nodeType === 'date' || node.nodeType === 'schema') return false;

      // Match query in content
      if (!node.content.toLowerCase().includes(lowerQuery)) return false;

      // Include tasks or text nodes (simplified since we don't have container concept)
      return node.nodeType === 'task' || node.nodeType === 'text';
    });

    return limit ? results.slice(0, limit) : results;
  }

  // === Phase 3: Embedding Operations (Not Needed for Sibling Chain Tests) ===

  async generateContainerEmbedding(_containerId: string): Promise<void> {
    // No-op for sibling chain tests
  }

  async searchContainers(_params: SearchContainersParams): Promise<Node[]> {
    return [];
  }

  async updateContainerEmbedding(_containerId: string): Promise<void> {
    // No-op for sibling chain tests
  }

  async batchGenerateEmbeddings(_containerIds: string[]): Promise<BatchEmbeddingResult> {
    return { successCount: 0, failedEmbeddings: [] };
  }

  async getStaleContainerCount(): Promise<number> {
    return 0;
  }

  async onContainerClosed(_containerId: string): Promise<void> {
    // No-op for sibling chain tests
  }

  async onContainerIdle(_containerId: string): Promise<boolean> {
    return false;
  }

  async syncEmbeddings(): Promise<number> {
    return 0;
  }

  // === Phase 3: Node Mention Operations (Not Needed for Sibling Chain Tests) ===

  async createContainerNode(_input: CreateContainerNodeInput): Promise<string> {
    throw new Error('createContainerNode not implemented in MockBackendAdapter');
  }

  async createNodeMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {
    // No-op for sibling chain tests
  }

  async getOutgoingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }

  async getIncomingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }

  async getMentioningContainers(_nodeId: string): Promise<string[]> {
    return [];
  }

  async deleteNodeMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {
    // No-op for sibling chain tests
  }

  // === Schema Management Operations (Not Needed for Sibling Chain Tests) ===

  async getSchema(_schemaId: string): Promise<SchemaDefinition> {
    throw new Error('getSchema not implemented in MockBackendAdapter');
  }

  async addSchemaField(_schemaId: string, _config: AddFieldConfig): Promise<AddFieldResult> {
    throw new Error('addSchemaField not implemented in MockBackendAdapter');
  }

  async removeSchemaField(_schemaId: string, _fieldName: string): Promise<RemoveFieldResult> {
    throw new Error('removeSchemaField not implemented in MockBackendAdapter');
  }

  async extendSchemaEnum(
    _schemaId: string,
    _fieldName: string,
    _value: string
  ): Promise<ExtendEnumResult> {
    throw new Error('extendSchemaEnum not implemented in MockBackendAdapter');
  }

  async removeSchemaEnumValue(
    _schemaId: string,
    _fieldName: string,
    _value: string
  ): Promise<RemoveEnumValueResult> {
    throw new Error('removeSchemaEnumValue not implemented in MockBackendAdapter');
  }

  /**
   * Helper: Clear all nodes (useful for test cleanup)
   */
  clear(): void {
    this.nodes.clear();
  }

  /**
   * Helper: Get all nodes (useful for test assertions)
   */
  getAllNodes(): Node[] {
    return Array.from(this.nodes.values());
  }

  /**
   * Helper: Get node count (useful for test assertions)
   */
  getNodeCount(): number {
    return this.nodes.size;
  }
}
