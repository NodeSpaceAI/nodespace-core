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
  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string> {
    const now = new Date().toISOString();
    const fullNode: Node = {
      ...node,
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
  async updateNode(id: string, update: NodeUpdate): Promise<void> {
    const existingNode = this.nodes.get(id);
    if (!existingNode) {
      throw new Error(`Node ${id} not found`);
    }

    const updatedNode: Node = {
      ...existingNode,
      ...update,
      modifiedAt: new Date().toISOString()
    };

    this.nodes.set(id, updatedNode);
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
   * Get children of a parent node
   */
  async getChildren(parentId: string): Promise<Node[]> {
    return Array.from(this.nodes.values()).filter((node) => node.parentId === parentId);
  }

  /**
   * Query nodes by parent and/or container
   */
  async queryNodes(params: QueryNodesParams): Promise<Node[]> {
    return Array.from(this.nodes.values()).filter((node) => {
      if (params.parentId !== undefined && node.parentId !== params.parentId) {
        return false;
      }
      if (params.containerId !== undefined && node.containerNodeId !== params.containerId) {
        return false;
      }
      return true;
    });
  }

  /**
   * Get nodes by container ID
   */
  async getNodesByContainerId(containerId: string): Promise<Node[]> {
    return Array.from(this.nodes.values()).filter((node) => node.containerNodeId === containerId);
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
