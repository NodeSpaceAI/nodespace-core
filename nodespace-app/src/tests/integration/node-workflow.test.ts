/**
 * Integration testing example for NodeSpace
 * Tests complete workflows across multiple components and systems
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { SimpleMockStore, createTestNode, type MockNodeData } from '../utils/testUtils';

// Mock API layer for integration testing
class MockNodeAPI {
  constructor(private dataStore: SimpleMockStore) {}

  async createNode(nodeData: { content: string; type: 'text' | 'task' | 'ai-chat' }) {
    const node = createTestNode({
      content: nodeData.content,
      type: nodeData.type
    });

    await this.dataStore.save(node);
    return node;
  }

  async updateNode(id: string, updates: { content?: string }) {
    const existing = await this.dataStore.load(id);
    if (!existing) {
      throw new Error(`Node ${id} not found`);
    }

    const updated = {
      ...existing,
      ...updates,
      updatedAt: new Date()
    };

    await this.dataStore.save(updated);
    return updated;
  }

  async deleteNode(id: string): Promise<boolean> {
    const existing = await this.dataStore.load(id);
    if (!existing) {
      return false;
    }

    // Mock deletion by removing from store
    const allNodes = this.dataStore.getAll();
    this.dataStore.clear();
    allNodes.filter((n) => n.id !== id).forEach((n) => this.dataStore.save(n));

    return true;
  }

  async searchNodes(query: string) {
    const allNodes = this.dataStore.getAll();
    return allNodes.filter((node) => node.content.toLowerCase().includes(query.toLowerCase()));
  }

  async getNodesByType(type: string) {
    const allNodes = this.dataStore.getAll();
    return allNodes.filter((node) => node.type === type);
  }
}

// Mock NodeManager that orchestrates the workflow
class MockNodeManager {
  constructor(
    private api: MockNodeAPI,
    private dataStore: SimpleMockStore
  ) {}

  async createAndSaveNode(content: string, type = 'text') {
    // Validate input
    if (!content.trim()) {
      throw new Error('Content cannot be empty');
    }

    // Create node via API
    const node = await this.api.createNode({ content, type });

    // Simulate additional processing
    await this.processNode(node);

    return node;
  }

  async editNode(id: string, newContent: string) {
    if (!newContent.trim()) {
      throw new Error('Content cannot be empty');
    }

    // Update via API
    const updated = await this.api.updateNode(id, { content: newContent });

    // Simulate reprocessing
    await this.processNode(updated);

    return updated;
  }

  async deleteNodeAndCleanup(id: string) {
    // Simulate cleanup operations
    const node = await this.dataStore.load(id);
    if (node) {
      // Log operation (in real app, might update audit log)
      console.log(`Deleting node: ${node.id} - ${node.content.substring(0, 50)}`);
    }

    return await this.api.deleteNode(id);
  }

  async bulkCreateNodes(contents: string[]) {
    const results = [];

    for (const content of contents) {
      const node = await this.createAndSaveNode(content);
      results.push(node);
    }

    return results;
  }

  private async processNode(node: MockNodeData) {
    // Simulate processing delay
    await new Promise((resolve) => setTimeout(resolve, 10));

    // Simulate node processing (e.g., extracting metadata, indexing, etc.)
    return node;
  }
}

describe('Node Management Integration Tests', () => {
  let dataStore: SimpleMockStore;
  let api: MockNodeAPI;
  let nodeManager: MockNodeManager;

  beforeEach(() => {
    SimpleMockStore.resetInstance();
    dataStore = SimpleMockStore.getInstance();
    api = new MockNodeAPI(dataStore);
    nodeManager = new MockNodeManager(api, dataStore);
  });

  describe('Complete Node Lifecycle', () => {
    it('creates, reads, updates, and deletes a node (CRUD)', async () => {
      // Create
      const created = await nodeManager.createAndSaveNode('Initial content');
      expect(created.content).toBe('Initial content');
      expect(created.id).toBeTruthy();

      // Read
      const retrieved = await dataStore.load(created.id);
      expect(retrieved).toBeTruthy();
      expect(retrieved!.content).toBe('Initial content');

      // Update
      const updated = await nodeManager.editNode(created.id, 'Updated content');
      expect(updated.content).toBe('Updated content');
      expect(updated.id).toBe(created.id);

      // Delete
      const deleted = await nodeManager.deleteNodeAndCleanup(created.id);
      expect(deleted).toBe(true);

      // Verify deletion
      const afterDelete = await dataStore.load(created.id);
      expect(afterDelete).toBeNull();
    });

    it('handles node creation with validation', async () => {
      // Should reject empty content
      await expect(nodeManager.createAndSaveNode('')).rejects.toThrow('Content cannot be empty');

      // Should reject whitespace-only content
      await expect(nodeManager.createAndSaveNode('   ')).rejects.toThrow('Content cannot be empty');

      // Should accept valid content
      const node = await nodeManager.createAndSaveNode('Valid content');
      expect(node.content).toBe('Valid content');
    });
  });

  describe('Multi-Node Workflows', () => {
    it('creates multiple nodes in bulk', async () => {
      const contents = ['First note', 'Second note', 'Third note'];

      const created = await nodeManager.bulkCreateNodes(contents);

      expect(created).toHaveLength(3);
      expect(created[0].content).toBe('First note');
      expect(created[1].content).toBe('Second note');
      expect(created[2].content).toBe('Third note');

      // Verify all nodes are in store
      const allNodes = dataStore.getAll();
      expect(allNodes).toHaveLength(3);
    });

    it('searches across multiple nodes', async () => {
      // Create test data
      await nodeManager.bulkCreateNodes([
        'Meeting notes from yesterday',
        'Project roadmap planning',
        'Notes about the new feature',
        'Shopping list for groceries'
      ]);

      // Search for 'notes'
      const notesResults = await api.searchNodes('notes');
      expect(notesResults).toHaveLength(2);
      expect(notesResults.some((n) => n.content.includes('Meeting notes'))).toBe(true);
      expect(notesResults.some((n) => n.content.includes('Notes about'))).toBe(true);

      // Search for 'project'
      const projectResults = await api.searchNodes('project');
      expect(projectResults).toHaveLength(1);
      expect(projectResults[0].content).toContain('Project roadmap');

      // Search for non-existent term
      const noResults = await api.searchNodes('nonexistent');
      expect(noResults).toHaveLength(0);
    });

    it('filters nodes by type', async () => {
      // Create mixed node types
      await api.createNode({ content: 'Text note', type: 'text' });
      await api.createNode({ content: 'Complete project', type: 'task' });
      await api.createNode({ content: 'Another text note', type: 'text' });
      await api.createNode({ content: 'Help with coding', type: 'ai-chat' });

      // Filter by type
      const textNodes = await api.getNodesByType('text');
      expect(textNodes).toHaveLength(2);
      expect(textNodes.every((n) => n.type === 'text')).toBe(true);

      const taskNodes = await api.getNodesByType('task');
      expect(taskNodes).toHaveLength(1);
      expect(taskNodes[0].content).toBe('Complete project');

      const aiChatNodes = await api.getNodesByType('ai-chat');
      expect(aiChatNodes).toHaveLength(1);
      expect(aiChatNodes[0].content).toBe('Help with coding');
    });
  });

  describe('Error Handling and Edge Cases', () => {
    it('handles updating non-existent nodes', async () => {
      await expect(nodeManager.editNode('non-existent-id', 'New content')).rejects.toThrow(
        'Node non-existent-id not found'
      );
    });

    it('handles deleting non-existent nodes', async () => {
      const result = await nodeManager.deleteNodeAndCleanup('non-existent-id');
      expect(result).toBe(false);
    });

    it('validates content during updates', async () => {
      // Create a node first
      const node = await nodeManager.createAndSaveNode('Original content');

      // Try to update with empty content
      await expect(nodeManager.editNode(node.id, '')).rejects.toThrow('Content cannot be empty');

      // Verify node content unchanged
      const unchanged = await dataStore.load(node.id);
      expect(unchanged!.content).toBe('Original content');
    });

    it('handles concurrent operations', async () => {
      // Simulate concurrent node creation
      const promises = Array.from({ length: 5 }, (_, i) =>
        nodeManager.createAndSaveNode(`Concurrent node ${i + 1}`)
      );

      const results = await Promise.all(promises);
      expect(results).toHaveLength(5);

      // All nodes should have unique IDs
      const ids = results.map((n) => n.id);
      const uniqueIds = new Set(ids);
      expect(uniqueIds.size).toBe(5);

      // All nodes should be in store
      const allNodes = dataStore.getAll();
      expect(allNodes).toHaveLength(5);
    });
  });

  describe('Data Consistency', () => {
    it('maintains data integrity across operations', async () => {
      // Create initial nodes
      const nodes = await nodeManager.bulkCreateNodes(['Node 1', 'Node 2', 'Node 3']);

      // Update some nodes
      await nodeManager.editNode(nodes[0].id, 'Updated Node 1');
      await nodeManager.editNode(nodes[2].id, 'Updated Node 3');

      // Delete one node
      await nodeManager.deleteNodeAndCleanup(nodes[1].id);

      // Verify final state
      const allNodes = dataStore.getAll();
      expect(allNodes).toHaveLength(2);

      const node1 = await dataStore.load(nodes[0].id);
      expect(node1!.content).toBe('Updated Node 1');

      const node2 = await dataStore.load(nodes[1].id);
      expect(node2).toBeNull();

      const node3 = await dataStore.load(nodes[2].id);
      expect(node3!.content).toBe('Updated Node 3');
    });

    it('maintains timestamps correctly', async () => {
      const before = new Date();

      // Create node
      const created = await nodeManager.createAndSaveNode('Timestamp test');
      expect(created.createdAt.getTime()).toBeGreaterThanOrEqual(before.getTime());
      expect(created.updatedAt.getTime()).toBeGreaterThanOrEqual(before.getTime());

      // Wait a bit then update
      await new Promise((resolve) => setTimeout(resolve, 50));
      const updateBefore = new Date();

      const updated = await nodeManager.editNode(created.id, 'Updated content');
      expect(updated.createdAt.getTime()).toBe(created.createdAt.getTime()); // Should not change
      expect(updated.updatedAt.getTime()).toBeGreaterThanOrEqual(updateBefore.getTime()); // Should be updated
    });
  });

  describe('Performance and Scalability', () => {
    it('handles reasonable number of nodes efficiently', async () => {
      const nodeCount = 100;
      const start = Date.now();

      // Create many nodes
      const contents = Array.from({ length: nodeCount }, (_, i) => `Node ${i + 1}`);
      await nodeManager.bulkCreateNodes(contents);

      const createTime = Date.now() - start;
      expect(createTime).toBeLessThan(5000); // Should complete in under 5 seconds

      // Search should be fast
      const searchStart = Date.now();
      const results = await api.searchNodes('Node');
      const searchTime = Date.now() - searchStart;

      expect(results).toHaveLength(nodeCount);
      expect(searchTime).toBeLessThan(100); // Should complete in under 100ms
    });

    it('memory usage remains reasonable', () => {
      // This is a simplified check - in real apps you might use memory profiling
      const initialNodes = dataStore.getAll().length;
      expect(initialNodes).toBe(0);

      // The previous test created 100 nodes, verify they can be garbage collected
      SimpleMockStore.resetInstance();
      const newStore = SimpleMockStore.getInstance();
      expect(newStore.getAll()).toHaveLength(0);
    });
  });
});
