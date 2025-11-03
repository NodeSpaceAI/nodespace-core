/**
 * Version Conflict Resolver Tests
 *
 * Tests auto-merge strategies and conflict resolution logic for OCC.
 */

import { describe, it, expect } from 'vitest';
import { ConflictResolver } from '$lib/services/version-conflict-resolver';
import type { Node } from '$lib/types';

describe('ConflictResolver', () => {
  // Helper to create a test node
  const createNode = (overrides: Partial<Node> = {}): Node => ({
    id: 'test-node',
    nodeType: 'text',
    content: 'Original content',
    properties: { status: 'draft', priority: 'low' },
    parentId: null,
    containerNodeId: null,
    beforeSiblingId: null,
    createdAt: '2024-01-01T00:00:00Z',
    modifiedAt: '2024-01-01T00:00:00Z',
    version: 1,
    ...overrides
  });

  describe('Auto-merge Strategy 1: Non-overlapping changes', () => {
    it('should auto-merge structural changes (parent/sibling)', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Modified content',
        properties: { status: 'published' }
      });
      const yourChanges: Partial<Node> = {
        parentId: 'new-parent',
        beforeSiblingId: 'new-sibling'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.strategy).toBe('auto-merged');
      expect(result.mergedNode?.parentId).toBe('new-parent');
      expect(result.mergedNode?.beforeSiblingId).toBe('new-sibling');
      expect(result.mergedNode?.content).toBe('Modified content');
      expect(result.mergedNode?.properties).toEqual({ status: 'published' });
    });

    it('should deep merge properties during non-overlapping auto-merge', () => {
      const currentNode = createNode({
        version: 2,
        parentId: 'updated-parent', // Structural change only
        properties: { status: 'published', priority: 'high' }
      });
      const yourChanges: Partial<Node> = {
        beforeSiblingId: 'new-sibling'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.parentId).toBe('updated-parent');
      expect(result.mergedNode?.beforeSiblingId).toBe('new-sibling');
      expect(result.mergedNode?.properties).toEqual({ status: 'published', priority: 'high' });
    });
  });

  describe('Auto-merge Strategy 2: Properties-only changes', () => {
    it('should merge properties when both changed same object', () => {
      const currentNode = createNode({
        version: 2,
        properties: { status: 'published', priority: 'high' }
      });
      const yourChanges: Partial<Node> = {
        properties: { assignee: 'alice' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({
        status: 'published',
        priority: 'high',
        assignee: 'alice'
      });
    });

    it('should handle conflicting property keys by using your value', () => {
      const currentNode = createNode({
        version: 2,
        properties: { status: 'published', priority: 'high' }
      });
      const yourChanges: Partial<Node> = {
        properties: { status: 'draft', assignee: 'alice' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({
        status: 'draft', // Your value takes precedence
        priority: 'high',
        assignee: 'alice'
      });
    });

    it('should deep merge nested properties', () => {
      const currentNode = createNode({
        version: 2,
        properties: {
          status: 'published',
          metadata: { views: 100, author: 'bob' }
        }
      });
      const yourChanges: Partial<Node> = {
        properties: {
          assignee: 'alice',
          metadata: { edited: true }
        }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({
        status: 'published',
        assignee: 'alice',
        metadata: { views: 100, author: 'bob', edited: true }
      });
    });
  });

  describe('Auto-merge Strategy 3: Content-only changes (last-write-wins)', () => {
    it('should use your content when both modified content', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Their content'
      });
      const yourChanges: Partial<Node> = {
        content: 'Your content'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.strategy).toBe('last-write-wins');
      expect(result.mergedNode?.content).toBe('Your content');
      expect(result.explanation).toContain('last-write-wins');
    });
  });

  describe('Strategy 4: Manual resolution required', () => {
    it('should require manual merge when version gap > 1', () => {
      const currentNode = createNode({
        version: 5, // Multiple updates happened
        content: 'Latest content'
      });
      const yourChanges: Partial<Node> = {
        parentId: 'new-parent'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(false);
      expect(result.strategy).toBe('user-choice-required');
    });

    it('should require manual merge for multiple conflicting changes', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Their content',
        properties: { status: 'published' }
      });
      const yourChanges: Partial<Node> = {
        content: 'Your content',
        properties: { status: 'draft' },
        parentId: 'new-parent'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(false);
      expect(result.explanation).toContain('Manual resolution required');
    });
  });

  describe('Overlapping change detection', () => {
    it('should detect content changes as overlapping', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Modified content'
      });
      const yourChanges: Partial<Node> = {
        content: 'Your content'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      // Content changes should be detected as overlapping and use last-write-wins
      expect(result.strategy).toBe('last-write-wins');
    });

    it('should not consider structural changes as overlapping', () => {
      const currentNode = createNode({
        version: 2,
        parentId: 'parent-1'
      });
      const yourChanges: Partial<Node> = {
        beforeSiblingId: 'sibling-1'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.strategy).toBe('auto-merged');
    });

    it('should allow auto-merge for properties-only changes', () => {
      const currentNode = createNode({
        version: 2,
        properties: { status: 'published' }
      });
      const yourChanges: Partial<Node> = {
        properties: { priority: 'high' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.strategy).toBe('auto-merged');
    });
  });

  describe('getUserChoiceOptions', () => {
    it('should provide both resolution options', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Their content',
        properties: { status: 'published' }
      });
      const yourChanges: Partial<Node> = {
        content: 'Your content',
        properties: { priority: 'high' }
      };

      const options = ConflictResolver.getUserChoiceOptions(yourChanges, currentNode);

      // "Use yours" should apply your changes over current
      expect(options.useYours.content).toBe('Your content');
      expect(options.useYours.properties).toEqual({ priority: 'high' });
      expect(options.useYours.version).toBe(2);

      // "Use current" should keep current state
      expect(options.useCurrent).toBe(currentNode);

      // Descriptions should be informative
      expect(options.yourChangesDescription).toContain('content');
      expect(options.yourChangesDescription).toContain('Properties');
      expect(options.currentChangesDescription).toContain('Version 2');
    });

    it('should describe content changes in user-friendly format', () => {
      const currentNode = createNode({ version: 2 });
      const yourChanges: Partial<Node> = {
        content: 'New text'
      };

      const options = ConflictResolver.getUserChoiceOptions(yourChanges, currentNode);

      expect(options.yourChangesDescription).toContain('Content:');
      expect(options.yourChangesDescription).toContain('New text');
    });

    it('should describe property changes with JSON', () => {
      const currentNode = createNode({ version: 2 });
      const yourChanges: Partial<Node> = {
        properties: { status: 'done', priority: 'high' }
      };

      const options = ConflictResolver.getUserChoiceOptions(yourChanges, currentNode);

      expect(options.yourChangesDescription).toContain('Properties:');
      expect(options.yourChangesDescription).toContain('status');
      expect(options.yourChangesDescription).toContain('done');
    });
  });

  describe('Edge cases', () => {
    it('should handle undefined properties gracefully', () => {
      const currentNode = createNode({
        version: 2,
        properties: undefined
      });
      const yourChanges: Partial<Node> = {
        properties: { status: 'published' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({ status: 'published' });
    });

    it('should handle empty properties objects', () => {
      const currentNode = createNode({
        version: 2,
        properties: {}
      });
      const yourChanges: Partial<Node> = {
        properties: { status: 'published' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({ status: 'published' });
    });

    it('should handle null values in properties', () => {
      const currentNode = createNode({
        version: 2,
        properties: { status: 'published', metadata: null }
      });
      const yourChanges: Partial<Node> = {
        properties: { priority: 'high' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({
        status: 'published',
        metadata: null,
        priority: 'high'
      });
    });

    it('should handle arrays in properties without deep merging them', () => {
      const currentNode = createNode({
        version: 2,
        properties: { tags: ['a', 'b'] }
      });
      const yourChanges: Partial<Node> = {
        properties: { tags: ['c', 'd'] }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      // Arrays should be replaced, not merged
      expect(result.mergedNode?.properties).toEqual({ tags: ['c', 'd'] });
    });

    it('should handle Date objects in properties', () => {
      const date1 = new Date('2024-01-01');
      const date2 = new Date('2024-02-01');

      const currentNode = createNode({
        version: 2,
        properties: { dueDate: date1 }
      });
      const yourChanges: Partial<Node> = {
        properties: { completedDate: date2 }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      // Date objects should be preserved (not deep merged)
      expect(result.mergedNode?.properties?.dueDate).toBe(date1);
      expect(result.mergedNode?.properties?.completedDate).toBe(date2);
    });

    it('should handle empty yourChanges with structural defaults', () => {
      const currentNode = createNode({ version: 2 });
      const yourChanges: Partial<Node> = {
        parentId: null // Structural field
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.parentId).toBe(null);
    });
  });

  describe('Real-world scenarios', () => {
    it('should handle AI assistant adding properties while user edits content', () => {
      const currentNode = createNode({
        version: 2,
        content: 'Original content',
        properties: {
          aiGenerated: true,
          summary: 'AI-generated summary',
          tags: ['important']
        }
      });
      const yourChanges: Partial<Node> = {
        content: 'User edited content'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      // Content changes use last-write-wins
      expect(result.autoMerged).toBe(true);
      expect(result.strategy).toBe('last-write-wins');
      expect(result.mergedNode?.content).toBe('User edited content');
      expect(result.mergedNode?.properties).toEqual({
        aiGenerated: true,
        summary: 'AI-generated summary',
        tags: ['important']
      });
    });

    it('should handle user reordering while AI updates content', () => {
      const currentNode = createNode({
        version: 2,
        content: 'AI-updated content'
      });
      const yourChanges: Partial<Node> = {
        beforeSiblingId: 'new-position'
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.content).toBe('AI-updated content');
      expect(result.mergedNode?.beforeSiblingId).toBe('new-position');
    });

    it('should handle bulk property updates from MCP while user edits one property', () => {
      const currentNode = createNode({
        version: 2,
        properties: {
          status: 'published',
          priority: 'high',
          assignee: 'alice',
          dueDate: '2024-12-31'
        }
      });
      const yourChanges: Partial<Node> = {
        properties: { status: 'in-progress' }
      };

      const result = ConflictResolver.tryAutoMerge(yourChanges, currentNode, 1);

      expect(result.autoMerged).toBe(true);
      expect(result.mergedNode?.properties).toEqual({
        status: 'in-progress', // Your change takes precedence
        priority: 'high',
        assignee: 'alice',
        dueDate: '2024-12-31'
      });
    });
  });

  describe('resolveFromConflictData', () => {
    it('should resolve using conflict data from MCP error', () => {
      const currentNode = createNode({
        version: 3,
        content: 'Latest content'
      });
      const conflictData = {
        type: 'VersionConflict' as const,
        node_id: 'test-node',
        expected_version: 1,
        actual_version: 3,
        current_node: currentNode
      };
      const yourChanges: Partial<Node> = {
        beforeSiblingId: 'new-position'
      };

      const result = ConflictResolver.resolveFromConflictData(conflictData, yourChanges);

      // Should delegate to tryAutoMerge - version gap > 1 blocks auto-merge
      expect(result.autoMerged).toBe(false);
      expect(result.strategy).toBe('user-choice-required');
    });
  });
});
