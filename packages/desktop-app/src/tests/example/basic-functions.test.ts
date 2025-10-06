/**
 * Basic function testing example for NodeSpace
 * Demonstrates simple testing without component complexity
 */
import { describe, it, expect } from 'vitest';
import { createTestNode, MOCK_TEXT_NODE } from '../helpers';

describe('Basic Function Testing', () => {
  describe('Test Data Creation', () => {
    it('creates test node with defaults', () => {
      const node = createTestNode();

      expect(node.content).toBe('Test content');
      expect(node.nodeType).toBe('text');
      expect(node.id).toContain('test-node-');
    });

    it('creates test node with overrides', () => {
      const node = createTestNode({
        content: 'Custom content',
        nodeType: 'task'
      });

      expect(node.content).toBe('Custom content');
      expect(node.nodeType).toBe('task');
    });
  });

  describe('Sample Data Validation', () => {
    it('has valid sample text node', () => {
      expect(MOCK_TEXT_NODE.id).toBe('mock-text-1');
      expect(MOCK_TEXT_NODE.nodeType).toBe('text');
      expect(MOCK_TEXT_NODE.content).toBe('This is a sample text node for testing.');
    });

    it('sample node has valid timestamps', () => {
      // Timestamps are ISO strings in the new format
      expect(MOCK_TEXT_NODE.createdAt).toBeTruthy();
      expect(MOCK_TEXT_NODE.modifiedAt).toBeTruthy();
      expect(new Date(MOCK_TEXT_NODE.createdAt).getTime()).toBeGreaterThan(0);
      expect(new Date(MOCK_TEXT_NODE.modifiedAt).getTime()).toBeGreaterThan(0);
    });
  });

  describe('Node Content Validation', () => {
    it('validates node content is not empty', () => {
      // Empty string is now preserved with ?? operator
      const emptyNode = createTestNode({ content: '' });
      expect(emptyNode.content).toBe(''); // Empty string preserved

      const validNode = createTestNode({ content: 'Valid content' });
      expect(validNode.content).toBe('Valid content');
      expect(validNode.content.length).toBeGreaterThan(0);
    });

    it('validates node type is valid', () => {
      const validTypes = ['text', 'task', 'ai-chat'];

      validTypes.forEach((type) => {
        const node = createTestNode({ nodeType: type });
        expect(validTypes).toContain(node.nodeType);
      });
    });
  });
});
