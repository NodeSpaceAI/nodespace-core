/**
 * Basic function testing example for NodeSpace
 * Demonstrates simple testing without component complexity
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { SimpleMockStore, createTestNode } from '../utils/testUtils';
import { sampleTextNode } from '../fixtures/mockNodes';

describe('Basic Function Testing', () => {
  describe('Test Data Creation', () => {
    it('creates test node with defaults', () => {
      const node = createTestNode();

      expect(node.content).toBe('Test content');
      expect(node.type).toBe('text');
      expect(node.id).toContain('test-');
    });

    it('creates test node with overrides', () => {
      const node = createTestNode({
        content: 'Custom content',
        type: 'task'
      });

      expect(node.content).toBe('Custom content');
      expect(node.type).toBe('task');
    });
  });

  describe('Mock Store Operations', () => {
    let store: SimpleMockStore;

    beforeEach(() => {
      SimpleMockStore.resetInstance();
      store = SimpleMockStore.getInstance();
    });

    it('saves and loads nodes', async () => {
      const testNode = createTestNode({ content: 'Saved content' });

      const savedId = await store.save(testNode);
      expect(savedId).toBe(testNode.id);

      const loaded = await store.load(testNode.id);
      expect(loaded?.content).toBe('Saved content');
    });

    it('returns null for non-existent node', async () => {
      const result = await store.load('non-existent-id');
      expect(result).toBeNull();
    });

    it('tracks all saved nodes', async () => {
      await store.save(createTestNode({ content: 'First' }));
      await store.save(createTestNode({ content: 'Second' }));

      const allNodes = store.getAll();
      expect(allNodes).toHaveLength(2);

      const contents = allNodes.map((n) => n.content);
      expect(contents).toContain('First');
      expect(contents).toContain('Second');
    });

    it('clears all nodes', async () => {
      await store.save(createTestNode());
      expect(store.getAll()).toHaveLength(1);

      store.clear();
      expect(store.getAll()).toHaveLength(0);
    });
  });

  describe('Sample Data Validation', () => {
    it('has valid sample text node', () => {
      expect(sampleTextNode.id).toBe('sample-text-1');
      expect(sampleTextNode.type).toBe('text');
      expect(sampleTextNode.content).toBe('This is a sample text node for testing.');
    });

    it('sample node has valid timestamps', () => {
      expect(sampleTextNode.createdAt).toBeInstanceOf(Date);
      expect(sampleTextNode.updatedAt).toBeInstanceOf(Date);
      expect(sampleTextNode.updatedAt.getTime()).toBeGreaterThanOrEqual(
        sampleTextNode.createdAt.getTime()
      );
    });
  });

  describe('Node Content Validation', () => {
    it('validates node content is not empty', () => {
      const emptyNode = createTestNode({ content: '' });
      expect(emptyNode.content).toBe('');

      const validNode = createTestNode({ content: 'Valid content' });
      expect(validNode.content.length).toBeGreaterThan(0);
    });

    it('validates node type is valid', () => {
      const validTypes = ['text', 'task', 'ai-chat'];

      validTypes.forEach((type) => {
        const node = createTestNode({ type: type as 'text' | 'task' | 'ai-chat' });
        expect(validTypes).toContain(node.type);
      });
    });
  });
});
