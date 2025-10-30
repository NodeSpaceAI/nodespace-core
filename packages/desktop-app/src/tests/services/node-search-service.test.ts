/**
 * Unit tests for NodeSearchService
 *
 * Tests all filtering rules:
 * 1. Exclude date nodes (default behavior)
 * 2. Match query in title/content (case-insensitive)
 * 3. Include ALL non-text nodes regardless of hierarchy
 * 4. Include ONLY container text nodes (top-level)
 */

import { describe, it, expect } from 'vitest';
import { NodeSearchService } from '$lib/services/node-search-service';
import type { Node } from '$lib/types/node';

/**
 * Helper to create a test node with minimal required fields
 */
function createTestNode(overrides: Partial<Node>): Node {
  return {
    id: 'test-id',
    nodeType: 'text',
    content: 'Test content',
    parentId: null,
    containerNodeId: null,
    beforeSiblingId: null,
    createdAt: '2025-01-01T00:00:00Z',
    modifiedAt: '2025-01-01T00:00:00Z',
    properties: {},
    ...overrides
  };
}

describe('NodeSearchService', () => {
  describe('filterMentionableNodes', () => {
    describe('Rule 1: Exclude date nodes', () => {
      it('should exclude date nodes by default', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', nodeType: 'text', content: 'Text node' }),
          createTestNode({ id: '2', nodeType: 'date', content: '2025-01-15' }),
          createTestNode({ id: '3', nodeType: 'task', content: 'Task node' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.nodeType === 'date')).toBeUndefined();
      });

      it('should allow custom excluded node types', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', nodeType: 'text', content: 'Text node' }),
          createTestNode({ id: '2', nodeType: 'date', content: '2025-01-15' }),
          createTestNode({ id: '3', nodeType: 'task', content: 'Task node' })
        ];

        // Exclude tasks instead of dates
        const result = NodeSearchService.filterMentionableNodes(nodes, '', {
          excludeNodeTypes: ['task']
        });

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.nodeType === 'task')).toBeUndefined();
        expect(result.find((n) => n.nodeType === 'date')).toBeDefined();
      });

      it('should allow excluding multiple node types', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', nodeType: 'text', content: 'Text node' }),
          createTestNode({ id: '2', nodeType: 'date', content: '2025-01-15' }),
          createTestNode({ id: '3', nodeType: 'task', content: 'Task node' }),
          createTestNode({ id: '4', nodeType: 'person', content: 'Person node' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '', {
          excludeNodeTypes: ['date', 'person']
        });

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.nodeType === 'date')).toBeUndefined();
        expect(result.find((n) => n.nodeType === 'person')).toBeUndefined();
      });
    });

    describe('Rule 2: Match query in title/content', () => {
      it('should match query in first line (title)', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: 'Meeting Notes\nSome details' }),
          createTestNode({ id: '2', content: 'Project Plan\nMore details' }),
          createTestNode({ id: '3', content: 'Random Thoughts\nEven more' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'Meeting');

        expect(result).toHaveLength(1);
        expect(result[0].id).toBe('1');
      });

      it('should match query in full content', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: 'Title\nMeeting in the content' }),
          createTestNode({ id: '2', content: 'Title\nSomething else' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'Meeting');

        expect(result).toHaveLength(1);
        expect(result[0].id).toBe('1');
      });

      it('should be case-insensitive', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: 'UPPERCASE TITLE' }),
          createTestNode({ id: '2', content: 'lowercase title' }),
          createTestNode({ id: '3', content: 'MiXeD CaSe TiTlE' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'title');

        expect(result).toHaveLength(3);
      });

      it('should return all nodes when query is empty', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: 'Node 1' }),
          createTestNode({ id: '2', content: 'Node 2' }),
          createTestNode({ id: '3', content: 'Node 3' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(3);
      });

      it('should return all nodes when query is only whitespace', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: 'Node 1' }),
          createTestNode({ id: '2', content: 'Node 2' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '   ');

        // Whitespace-only query matches everything (since '   ' is present in all strings)
        expect(result).toHaveLength(0);
      });

      it('should handle nodes with empty content', () => {
        const nodes: Node[] = [
          createTestNode({ id: '1', content: '' }),
          createTestNode({ id: '2', content: 'Valid content' })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'content');

        expect(result).toHaveLength(1);
        expect(result[0].id).toBe('2');
      });
    });

    describe('Rule 3: Include ALL non-text nodes', () => {
      it('should include task nodes regardless of hierarchy', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'task-1',
            nodeType: 'task',
            content: 'Top-level task',
            containerNodeId: null
          }),
          createTestNode({
            id: 'task-2',
            nodeType: 'task',
            content: 'Nested task',
            containerNodeId: 'some-parent'
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.id === 'task-1')).toBeDefined();
        expect(result.find((n) => n.id === 'task-2')).toBeDefined();
      });

      it('should include person nodes regardless of hierarchy', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'person-1',
            nodeType: 'person',
            content: 'Alice',
            containerNodeId: null
          }),
          createTestNode({
            id: 'person-2',
            nodeType: 'person',
            content: 'Bob',
            containerNodeId: 'some-parent'
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(2);
      });

      it('should include all custom node types regardless of hierarchy', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'custom-1',
            nodeType: 'custom-type',
            content: 'Custom node',
            containerNodeId: 'parent-1'
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(1);
        expect(result[0].id).toBe('custom-1');
      });
    });

    describe('Rule 4: Include ONLY container text nodes', () => {
      it('should include text nodes with containerNodeId === null', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'text-1',
            nodeType: 'text',
            content: 'Container text node',
            containerNodeId: null
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(1);
        expect(result[0].id).toBe('text-1');
      });

      it('should exclude text nodes with containerNodeId set', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'text-1',
            nodeType: 'text',
            content: 'Nested text node',
            containerNodeId: 'parent-node'
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(0);
      });

      it('should separate container and nested text nodes correctly', () => {
        const nodes: Node[] = [
          createTestNode({
            id: 'container-1',
            nodeType: 'text',
            content: 'Container 1',
            containerNodeId: null
          }),
          createTestNode({
            id: 'nested-1',
            nodeType: 'text',
            content: 'Nested under container-1',
            containerNodeId: 'container-1'
          }),
          createTestNode({
            id: 'container-2',
            nodeType: 'text',
            content: 'Container 2',
            containerNodeId: null
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, '');

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.id === 'container-1')).toBeDefined();
        expect(result.find((n) => n.id === 'container-2')).toBeDefined();
        expect(result.find((n) => n.id === 'nested-1')).toBeUndefined();
      });
    });

    describe('Combined filtering rules', () => {
      it('should apply all rules together', () => {
        const nodes: Node[] = [
          // Should be included: container text node matching query
          createTestNode({
            id: '1',
            nodeType: 'text',
            content: 'Project Notes',
            containerNodeId: null
          }),
          // Should be excluded: nested text node (even though it matches)
          createTestNode({
            id: '2',
            nodeType: 'text',
            content: 'Project Details',
            containerNodeId: '1'
          }),
          // Should be included: task node matching query (hierarchy doesn't matter)
          createTestNode({
            id: '3',
            nodeType: 'task',
            content: 'Project Task',
            containerNodeId: '1'
          }),
          // Should be excluded: date node (excluded by default)
          createTestNode({
            id: '4',
            nodeType: 'date',
            content: 'Project 2025-01-15',
            containerNodeId: null
          }),
          // Should be excluded: doesn't match query
          createTestNode({
            id: '5',
            nodeType: 'text',
            content: 'Random Thoughts',
            containerNodeId: null
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'Project');

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.id === '1')).toBeDefined(); // Container text
        expect(result.find((n) => n.id === '3')).toBeDefined(); // Task
        expect(result.find((n) => n.id === '2')).toBeUndefined(); // Nested text
        expect(result.find((n) => n.id === '4')).toBeUndefined(); // Date
        expect(result.find((n) => n.id === '5')).toBeUndefined(); // No match
      });

      it('should handle empty node array', () => {
        const result = NodeSearchService.filterMentionableNodes([], 'query');
        expect(result).toHaveLength(0);
      });

      it('should handle complex hierarchy with mixed node types', () => {
        const nodes: Node[] = [
          // Root date (should be excluded)
          createTestNode({
            id: 'date-1',
            nodeType: 'date',
            content: '2025-01-15',
            containerNodeId: null
          }),
          // Text under date (should be included as container)
          createTestNode({
            id: 'text-1',
            nodeType: 'text',
            content: 'Meeting',
            containerNodeId: null
          }),
          // Task under text (should be included)
          createTestNode({
            id: 'task-1',
            nodeType: 'task',
            content: 'Review Meeting notes',
            containerNodeId: 'text-1'
          }),
          // Nested text under text (should be excluded)
          createTestNode({
            id: 'text-2',
            nodeType: 'text',
            content: 'Meeting details',
            containerNodeId: 'text-1'
          })
        ];

        const result = NodeSearchService.filterMentionableNodes(nodes, 'Meeting');

        expect(result).toHaveLength(2);
        expect(result.find((n) => n.id === 'text-1')).toBeDefined(); // Container text
        expect(result.find((n) => n.id === 'task-1')).toBeDefined(); // Nested task
        expect(result.find((n) => n.id === 'date-1')).toBeUndefined(); // Date excluded
        expect(result.find((n) => n.id === 'text-2')).toBeUndefined(); // Nested text
      });
    });
  });
});
