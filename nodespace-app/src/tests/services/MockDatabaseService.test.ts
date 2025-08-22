/**
 * MockDatabaseService Test Suite
 *
 * Comprehensive tests for MockDatabaseService focusing on:
 * - Exact NodeSpaceNode schema compliance
 * - CRUD operations correctness
 * - Query functionality and filtering
 * - Mentions array bidirectionality
 * - Sibling ordering with before_sibling_id
 * - Index maintenance and performance
 * - Hierarchy operations
 * - Data integrity constraints
 */

import { describe, test, expect, beforeEach } from 'vitest';
import { MockDatabaseService, type NodeSpaceNode } from '$lib/services/MockDatabaseService';

describe('MockDatabaseService', () => {
  let db: MockDatabaseService;

  beforeEach(() => {
    db = new MockDatabaseService();
  });

  // ========================================================================
  // Schema Validation
  // ========================================================================

  describe('Schema Validation', () => {
    test('accepts valid NodeSpaceNode', async () => {
      const validNode: NodeSpaceNode = {
        id: 'test-1',
        type: 'text',
        content: 'Test content',
        parent_id: null,
        root_id: 'test-1',
        before_sibling_id: null,
        created_at: new Date().toISOString(),
        mentions: ['other-node'],
        metadata: { tags: ['test'] },
        embedding_vector: null
      };

      await expect(db.insertNode(validNode)).resolves.toBeUndefined();
    });

    test('rejects node with invalid id', async () => {
      const invalidNode = {
        id: '', // Invalid: empty string
        type: 'text',
        content: 'Test content',
        parent_id: null,
        root_id: 'test-1',
        before_sibling_id: null,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      } as NodeSpaceNode;

      await expect(db.insertNode(invalidNode)).rejects.toThrow(
        'Node ID must be a non-empty string'
      );
    });

    test('rejects node with invalid type', async () => {
      const invalidNode = {
        id: 'test-1',
        type: '', // Invalid: empty string
        content: 'Test content',
        parent_id: null,
        root_id: 'test-1',
        before_sibling_id: null,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      } as NodeSpaceNode;

      await expect(db.insertNode(invalidNode)).rejects.toThrow(
        'Node type must be a non-empty string'
      );
    });

    test('rejects node with invalid mentions array', async () => {
      const invalidNode = {
        id: 'test-1',
        type: 'text',
        content: 'Test content',
        parent_id: null,
        root_id: 'test-1',
        before_sibling_id: null,
        created_at: new Date().toISOString(),
        mentions: 'not-an-array', // Invalid: should be array
        metadata: {},
        embedding_vector: null
      } as unknown as NodeSpaceNode;

      await expect(db.insertNode(invalidNode)).rejects.toThrow('Node mentions must be an array');
    });

    test('rejects node with invalid metadata', async () => {
      const invalidNode = {
        id: 'test-1',
        type: 'text',
        content: 'Test content',
        parent_id: null,
        root_id: 'test-1',
        before_sibling_id: null,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: null, // Invalid: should be object
        embedding_vector: null
      } as unknown as NodeSpaceNode;

      await expect(db.insertNode(invalidNode)).rejects.toThrow('Node metadata must be an object');
    });
  });

  // ========================================================================
  // CRUD Operations
  // ========================================================================

  describe('CRUD Operations', () => {
    const sampleNode: NodeSpaceNode = {
      id: 'node-1',
      type: 'text',
      content: 'Sample content',
      parent_id: null,
      root_id: 'node-1',
      before_sibling_id: null,
      created_at: '2023-01-01T00:00:00.000Z',
      mentions: ['node-2'],
      metadata: { tag: 'sample' },
      embedding_vector: null
    };

    test('insertNode creates new node', async () => {
      await db.insertNode(sampleNode);

      const retrieved = await db.getNode('node-1');
      expect(retrieved).toEqual(sampleNode);
    });

    test('insertNode rejects duplicate ID', async () => {
      await db.insertNode(sampleNode);

      await expect(db.insertNode(sampleNode)).rejects.toThrow('Node with ID node-1 already exists');
    });

    test('getNode returns null for non-existent node', async () => {
      const result = await db.getNode('non-existent');
      expect(result).toBeNull();
    });

    test('updateNode modifies existing node', async () => {
      await db.insertNode(sampleNode);

      const updated = await db.updateNode('node-1', {
        content: 'Updated content',
        metadata: { tag: 'updated', newField: 'value' }
      });

      expect(updated).not.toBeNull();
      expect(updated!.content).toBe('Updated content');
      expect(updated!.metadata).toEqual({ tag: 'updated', newField: 'value' });
      expect(updated!.id).toBe('node-1'); // ID should remain unchanged
    });

    test('updateNode returns null for non-existent node', async () => {
      const result = await db.updateNode('non-existent', { content: 'new content' });
      expect(result).toBeNull();
    });

    test('deleteNode removes node', async () => {
      await db.insertNode(sampleNode);

      const deleted = await db.deleteNode('node-1');
      expect(deleted).toBe(true);

      const retrieved = await db.getNode('node-1');
      expect(retrieved).toBeNull();
    });

    test('deleteNode returns false for non-existent node', async () => {
      const result = await db.deleteNode('non-existent');
      expect(result).toBe(false);
    });
  });

  // ========================================================================
  // Query Operations
  // ========================================================================

  describe('Query Operations', () => {
    beforeEach(async () => {
      // Set up test data
      const nodes: NodeSpaceNode[] = [
        {
          id: 'root-1',
          type: 'document',
          content: 'Root document with important information',
          parent_id: null,
          root_id: 'root-1',
          before_sibling_id: null,
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: ['child-1'],
          metadata: { priority: 'high', tags: ['work'] },
          embedding_vector: null
        },
        {
          id: 'child-1',
          type: 'note',
          content: 'Child note referencing parent',
          parent_id: 'root-1',
          root_id: 'root-1',
          before_sibling_id: null,
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: ['root-1'],
          metadata: { priority: 'medium' },
          embedding_vector: null
        },
        {
          id: 'child-2',
          type: 'note',
          content: 'Second child with different content',
          parent_id: 'root-1',
          root_id: 'root-1',
          before_sibling_id: 'child-1',
          created_at: '2023-01-03T00:00:00.000Z',
          mentions: [],
          metadata: { priority: 'low' },
          embedding_vector: null
        },
        {
          id: 'root-2',
          type: 'task',
          content: 'Independent task node',
          parent_id: null,
          root_id: 'root-2',
          before_sibling_id: 'root-1',
          created_at: '2023-01-04T00:00:00.000Z',
          mentions: ['child-1'],
          metadata: { completed: false },
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }
    });

    test('queryNodes returns all nodes when no filter', async () => {
      const results = await db.queryNodes();
      expect(results).toHaveLength(4);
    });

    test('queryNodes filters by ID', async () => {
      const results = await db.queryNodes({ id: 'root-1' });
      expect(results).toHaveLength(1);
      expect(results[0].id).toBe('root-1');
    });

    test('queryNodes filters by type', async () => {
      const results = await db.queryNodes({ type: 'note' });
      expect(results).toHaveLength(2);
      expect(results.every((n) => n.type === 'note')).toBe(true);
    });

    test('queryNodes filters by parent_id', async () => {
      const results = await db.queryNodes({ parent_id: 'root-1' });
      expect(results).toHaveLength(2);
      expect(results.every((n) => n.parent_id === 'root-1')).toBe(true);
    });

    test('queryNodes filters by root_id', async () => {
      const results = await db.queryNodes({ root_id: 'root-1' });
      expect(results).toHaveLength(3); // root-1, child-1, child-2
      expect(results.every((n) => n.root_id === 'root-1')).toBe(true);
    });

    test('queryNodes filters by content', async () => {
      const results = await db.queryNodes({ content_contains: 'important' });
      expect(results).toHaveLength(1);
      expect(results[0].content).toContain('important');
    });

    test('queryNodes filters by mentioned_by', async () => {
      const results = await db.queryNodes({ mentioned_by: 'child-1' });
      expect(results).toHaveLength(2); // root-1 and root-2 mention child-1
      expect(results.every((n) => n.mentions.includes('child-1'))).toBe(true);
    });

    test('queryNodes filters by mentions', async () => {
      const results = await db.queryNodes({ mentions: 'child-1' });
      expect(results).toHaveLength(1); // Only root-1 is mentioned by child-1
      expect(results[0].id).toBe('root-1');
    });

    test('queryNodes applies pagination', async () => {
      const page1 = await db.queryNodes({ limit: 2 });
      expect(page1).toHaveLength(2);

      const page2 = await db.queryNodes({ limit: 2, offset: 2 });
      expect(page2).toHaveLength(2);

      // Ensure different results
      const page1Ids = page1.map((n) => n.id);
      const page2Ids = page2.map((n) => n.id);
      expect(page1Ids).not.toEqual(page2Ids);
    });

    test('queryNodes combines multiple filters', async () => {
      const results = await db.queryNodes({
        type: 'note',
        parent_id: 'root-1',
        content_contains: 'referencing'
      });

      expect(results).toHaveLength(1);
      expect(results[0].id).toBe('child-1');
    });
  });

  // ========================================================================
  // Hierarchy Operations
  // ========================================================================

  describe('Hierarchy Operations', () => {
    beforeEach(async () => {
      // Create hierarchical structure
      const nodes: NodeSpaceNode[] = [
        {
          id: 'root',
          type: 'document',
          content: 'Root document',
          parent_id: null,
          root_id: 'root',
          before_sibling_id: null,
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'child-1',
          type: 'section',
          content: 'First child',
          parent_id: 'root',
          root_id: 'root',
          before_sibling_id: null,
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'child-2',
          type: 'section',
          content: 'Second child',
          parent_id: 'root',
          root_id: 'root',
          before_sibling_id: 'child-1',
          created_at: '2023-01-03T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'grandchild-1',
          type: 'note',
          content: 'Grandchild of root',
          parent_id: 'child-1',
          root_id: 'root',
          before_sibling_id: null,
          created_at: '2023-01-04T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'other-root',
          type: 'document',
          content: 'Another root',
          parent_id: null,
          root_id: 'other-root',
          before_sibling_id: 'root',
          created_at: '2023-01-05T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }
    });

    test('getChildren returns direct children only', async () => {
      const children = await db.getChildren('root');
      expect(children).toHaveLength(2);
      expect(children.map((n) => n.id)).toEqual(['child-1', 'child-2']);
    });

    test('getChildren respects sibling ordering', async () => {
      const children = await db.getChildren('root');
      expect(children[0].id).toBe('child-1');
      expect(children[1].id).toBe('child-2');
      expect(children[1].before_sibling_id).toBe('child-1');
    });

    test('getRootNodes returns nodes with no parent', async () => {
      const roots = await db.getRootNodes();
      expect(roots).toHaveLength(2);
      expect(roots.map((n) => n.id)).toEqual(['root', 'other-root']);
    });

    test('getRootNodes respects sibling ordering for roots', async () => {
      const roots = await db.getRootNodes();
      expect(roots[0].id).toBe('root');
      expect(roots[1].id).toBe('other-root');
      expect(roots[1].before_sibling_id).toBe('root');
    });

    test('getDescendants returns all descendants recursively', async () => {
      const descendants = await db.getDescendants('root');
      expect(descendants).toHaveLength(3); // child-1, child-2, grandchild-1

      const descendantIds = descendants.map((n) => n.id);
      expect(descendantIds).toContain('child-1');
      expect(descendantIds).toContain('child-2');
      expect(descendantIds).toContain('grandchild-1');
    });

    test('getDescendants maintains hierarchy order', async () => {
      const descendants = await db.getDescendants('root');

      // child-1 should come before grandchild-1 (breadth-first traversal)
      const child1Index = descendants.findIndex((n) => n.id === 'child-1');
      const grandchild1Index = descendants.findIndex((n) => n.id === 'grandchild-1');

      expect(child1Index).toBeLessThan(grandchild1Index);
    });
  });

  // ========================================================================
  // Mentions and Backlinks
  // ========================================================================

  describe('Mentions and Backlinks', () => {
    beforeEach(async () => {
      const nodes: NodeSpaceNode[] = [
        {
          id: 'article',
          type: 'document',
          content: 'Article content',
          parent_id: null,
          root_id: 'article',
          before_sibling_id: null,
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: ['reference-1', 'reference-2'],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'note-1',
          type: 'note',
          content: 'Note mentioning article',
          parent_id: null,
          root_id: 'note-1',
          before_sibling_id: null,
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: ['article'],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'note-2',
          type: 'note',
          content: 'Another note mentioning article',
          parent_id: null,
          root_id: 'note-2',
          before_sibling_id: null,
          created_at: '2023-01-03T00:00:00.000Z',
          mentions: ['article'],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'reference-1',
          type: 'reference',
          content: 'First reference',
          parent_id: null,
          root_id: 'reference-1',
          before_sibling_id: null,
          created_at: '2023-01-04T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'reference-2',
          type: 'reference',
          content: 'Second reference',
          parent_id: null,
          root_id: 'reference-2',
          before_sibling_id: null,
          created_at: '2023-01-05T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }
    });

    test('getBacklinks returns nodes that mention the target', async () => {
      const backlinks = await db.getBacklinks('article');
      expect(backlinks).toHaveLength(2);

      const backlinkIds = backlinks.map((n) => n.id);
      expect(backlinkIds).toContain('note-1');
      expect(backlinkIds).toContain('note-2');
    });

    test('getMentions returns nodes mentioned by the source', async () => {
      const mentions = await db.getMentions('article');
      expect(mentions).toHaveLength(2);

      const mentionIds = mentions.map((n) => n.id);
      expect(mentionIds).toContain('reference-1');
      expect(mentionIds).toContain('reference-2');
    });

    test('updateNodeMentions maintains bidirectional consistency', async () => {
      await db.updateNodeMentions('note-1', ['reference-1', 'reference-2']);

      // Verify forward links
      const updatedNote = await db.getNode('note-1');
      expect(updatedNote!.mentions).toEqual(['reference-1', 'reference-2']);

      // Verify backlinks were updated
      const ref1Backlinks = await db.getBacklinks('reference-1');
      const ref2Backlinks = await db.getBacklinks('reference-2');

      expect(ref1Backlinks.some((n) => n.id === 'note-1')).toBe(true);
      expect(ref2Backlinks.some((n) => n.id === 'note-1')).toBe(true);

      // Verify old backlinks were removed
      const articleBacklinks = await db.getBacklinks('article');
      expect(articleBacklinks.some((n) => n.id === 'note-1')).toBe(false);
    });

    test('mentions are cleaned up when node is deleted', async () => {
      // Get initial backlinks
      const initialBacklinks = await db.getBacklinks('article');
      expect(initialBacklinks).toHaveLength(2);

      // Delete a node that mentions article
      await db.deleteNode('note-1');

      // Backlinks should be updated
      const updatedBacklinks = await db.getBacklinks('article');
      expect(updatedBacklinks).toHaveLength(1);
      expect(updatedBacklinks[0].id).toBe('note-2');
    });

    test('updating mentions removes old backlink references', async () => {
      await db.updateNodeMentions('note-1', []); // Remove all mentions

      const articleBacklinks = await db.getBacklinks('article');
      expect(articleBacklinks.some((n) => n.id === 'note-1')).toBe(false);
      expect(articleBacklinks).toHaveLength(1); // Only note-2 should remain
    });
  });

  // ========================================================================
  // Sibling Ordering
  // ========================================================================

  describe('Sibling Ordering', () => {
    test('maintains sibling order with before_sibling_id', async () => {
      const nodes: NodeSpaceNode[] = [
        {
          id: 'first',
          type: 'text',
          content: 'First sibling',
          parent_id: 'parent',
          root_id: 'root',
          before_sibling_id: null, // First in chain
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'second',
          type: 'text',
          content: 'Second sibling',
          parent_id: 'parent',
          root_id: 'root',
          before_sibling_id: 'first',
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'third',
          type: 'text',
          content: 'Third sibling',
          parent_id: 'parent',
          root_id: 'root',
          before_sibling_id: 'second',
          created_at: '2023-01-03T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }

      const children = await db.getChildren('parent');
      expect(children.map((n) => n.id)).toEqual(['first', 'second', 'third']);
    });

    test('handles broken sibling chain gracefully', async () => {
      const nodes: NodeSpaceNode[] = [
        {
          id: 'orphan',
          type: 'text',
          content: 'Orphaned sibling',
          parent_id: 'parent',
          root_id: 'root',
          before_sibling_id: 'missing', // Reference to non-existent node
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'normal',
          type: 'text',
          content: 'Normal sibling',
          parent_id: 'parent',
          root_id: 'root',
          before_sibling_id: null,
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }

      const children = await db.getChildren('parent');
      expect(children).toHaveLength(2);
      // Should still return all nodes even with broken chain
    });

    test('handles circular sibling references', async () => {
      const node1: NodeSpaceNode = {
        id: 'node-1',
        type: 'text',
        content: 'Node 1',
        parent_id: 'parent',
        root_id: 'root',
        before_sibling_id: 'node-2', // Will create circular ref
        created_at: '2023-01-01T00:00:00.000Z',
        mentions: [],
        metadata: {},
        embedding_vector: null
      };

      const node2: NodeSpaceNode = {
        id: 'node-2',
        type: 'text',
        content: 'Node 2',
        parent_id: 'parent',
        root_id: 'root',
        before_sibling_id: null,
        created_at: '2023-01-02T00:00:00.000Z',
        mentions: [],
        metadata: {},
        embedding_vector: null
      };

      await db.insertNode(node2);
      await db.insertNode(node1);

      // Update node-2 to create circular reference
      await db.updateNode('node-2', { before_sibling_id: 'node-1' });

      const children = await db.getChildren('parent');
      expect(children).toHaveLength(2);
      // Should handle circular reference without infinite loop
    });
  });

  // ========================================================================
  // Database Statistics and Utilities
  // ========================================================================

  describe('Database Statistics and Utilities', () => {
    beforeEach(async () => {
      const nodes: NodeSpaceNode[] = [
        {
          id: 'doc-1',
          type: 'document',
          content: 'Document 1',
          parent_id: null,
          root_id: 'doc-1',
          before_sibling_id: null,
          created_at: '2023-01-01T00:00:00.000Z',
          mentions: ['note-1', 'note-2'],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'note-1',
          type: 'note',
          content: 'Note 1',
          parent_id: null,
          root_id: 'note-1',
          before_sibling_id: null,
          created_at: '2023-01-02T00:00:00.000Z',
          mentions: ['doc-1'],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'note-2',
          type: 'note',
          content: 'Note 2',
          parent_id: null,
          root_id: 'note-2',
          before_sibling_id: null,
          created_at: '2023-01-03T00:00:00.000Z',
          mentions: [],
          metadata: {},
          embedding_vector: null
        },
        {
          id: 'task-1',
          type: 'task',
          content: 'Task 1',
          parent_id: null,
          root_id: 'task-1',
          before_sibling_id: null,
          created_at: '2023-01-04T00:00:00.000Z',
          mentions: ['note-1'],
          metadata: {},
          embedding_vector: null
        }
      ];

      for (const node of nodes) {
        await db.insertNode(node);
      }
    });

    test('getStats provides accurate statistics', () => {
      const stats = db.getStats();

      expect(stats.totalNodes).toBe(4);
      expect(stats.nodesByType).toEqual({
        document: 1,
        note: 2,
        task: 1
      });
      expect(stats.totalMentions).toBe(4); // 2 + 1 + 0 + 1
      expect(stats.averageMentionsPerNode).toBe(1); // 4 mentions / 4 nodes
    });

    test('clear removes all data', async () => {
      await db.clear();

      const nodes = await db.queryNodes();
      expect(nodes).toHaveLength(0);

      const stats = db.getStats();
      expect(stats.totalNodes).toBe(0);
    });

    test('exportData returns all nodes', () => {
      const exported = db.exportData();
      expect(exported).toHaveLength(4);

      const exportedIds = exported.map((n) => n.id);
      expect(exportedIds).toContain('doc-1');
      expect(exportedIds).toContain('note-1');
      expect(exportedIds).toContain('note-2');
      expect(exportedIds).toContain('task-1');
    });

    test('importData restores nodes correctly', async () => {
      const originalData = db.exportData();

      await db.clear();
      expect(db.getStats().totalNodes).toBe(0);

      await db.importData(originalData);

      const stats = db.getStats();
      expect(stats.totalNodes).toBe(4);

      // Verify backlinks are restored
      const doc1Backlinks = await db.getBacklinks('doc-1');
      expect(doc1Backlinks).toHaveLength(1);
      expect(doc1Backlinks[0].id).toBe('note-1');
    });
  });

  // ========================================================================
  // Performance and Index Integrity
  // ========================================================================

  describe('Performance and Index Integrity', () => {
    test('maintains consistent indexes during operations', async () => {
      const node: NodeSpaceNode = {
        id: 'test-node',
        type: 'test',
        content: 'Test content',
        parent_id: 'parent',
        root_id: 'root',
        before_sibling_id: null,
        created_at: '2023-01-01T00:00:00.000Z',
        mentions: ['target-1', 'target-2'],
        metadata: {},
        embedding_vector: null
      };

      // Insert
      await db.insertNode(node);

      let byType = await db.queryNodes({ type: 'test' });
      expect(byType).toHaveLength(1);

      let byParent = await db.queryNodes({ parent_id: 'parent' });
      expect(byParent).toHaveLength(1);

      // Update
      await db.updateNode('test-node', {
        type: 'updated-type',
        parent_id: null,
        mentions: ['target-3']
      });

      byType = await db.queryNodes({ type: 'test' });
      expect(byType).toHaveLength(0);

      const byUpdatedType = await db.queryNodes({ type: 'updated-type' });
      expect(byUpdatedType).toHaveLength(1);

      byParent = await db.queryNodes({ parent_id: 'parent' });
      expect(byParent).toHaveLength(0);

      const rootNodes = await db.queryNodes({ parent_id: null });
      expect(rootNodes.some((n) => n.id === 'test-node')).toBe(true);

      // Delete
      await db.deleteNode('test-node');

      const afterDelete = await db.queryNodes({ type: 'updated-type' });
      expect(afterDelete).toHaveLength(0);
    });

    test('handles large datasets efficiently', async () => {
      const nodes: NodeSpaceNode[] = [];

      // Create 1000 nodes with mentions
      for (let i = 0; i < 1000; i++) {
        nodes.push({
          id: `node-${i}`,
          type: i % 5 === 0 ? 'special' : 'regular',
          content: `Content ${i}`,
          parent_id: i % 100 === 0 ? null : `node-${Math.floor(i / 10) * 10}`,
          root_id: `node-${Math.floor(i / 100) * 100}`,
          before_sibling_id: i % 10 === 0 ? null : `node-${i - 1}`,
          created_at: new Date(2023, 0, 1, 0, 0, i).toISOString(),
          mentions: i % 7 === 0 ? [`node-${(i + 1) % 1000}`] : [],
          metadata: { index: i },
          embedding_vector: null
        });
      }

      const startTime = performance.now();

      for (const node of nodes) {
        await db.insertNode(node);
      }

      const insertTime = performance.now() - startTime;

      // Query performance
      const queryStartTime = performance.now();

      const specialNodes = await db.queryNodes({ type: 'special' });
      await db.queryNodes({ mentioned_by: 'node-0' });
      await db.queryNodes({ content_contains: '500' });

      const queryTime = performance.now() - queryStartTime;

      expect(specialNodes.length).toBeGreaterThan(0);
      expect(insertTime).toBeLessThan(1000); // Should insert 1000 nodes in under 1 second
      expect(queryTime).toBeLessThan(100); // Queries should be fast
    });
  });
});
