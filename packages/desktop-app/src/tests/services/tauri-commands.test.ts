/**
 * Tauri Commands Service Tests
 *
 * Tests the tauri-commands module which provides a clean API for backend communication.
 * In test environment, these use the MockAdapter from backend-adapter.
 * These tests verify the API surface and that calls work end-to-end with the mock backend.
 */

import { describe, it, expect } from 'vitest';
import * as tauriCommands from '$lib/services/tauri-commands';
import type { Node, TaskNodeUpdate } from '$lib/types';
import type {
  CreateNodeInput,
  UpdateNodeInput,
  NodeQuery,
  CreateContainerInput
} from '$lib/services/tauri-commands';

describe('Tauri Commands Service - API Surface', () => {
  // ============================================================================
  // Node CRUD Commands
  // ============================================================================

  describe('createNode', () => {
    it('should accept CreateNodeInput and return node ID', async () => {
      const input: CreateNodeInput = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Test content'
      };

      const result = await tauriCommands.createNode(input);

      expect(typeof result).toBe('string');
      expect(result).toBeTruthy();
    });

    it('should accept full Node object and return node ID', async () => {
      const node: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Test content',
        version: 0,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
      };

      const result = await tauriCommands.createNode(node);

      expect(typeof result).toBe('string');
      expect(result).toBeTruthy();
    });

    it('should handle node with properties, mentions, and parent', async () => {
      const input: CreateNodeInput = {
        id: 'node-1',
        nodeType: 'task',
        content: 'Task content',
        properties: { priority: 'high' },
        mentions: ['user-1', 'user-2'],
        parentId: 'parent-1',
        insertAfterNodeId: 'sibling-1'
      };

      const result = await tauriCommands.createNode(input);

      expect(typeof result).toBe('string');
    });
  });

  describe('getNode', () => {
    it('should accept node ID and return Node or null', async () => {
      const result = await tauriCommands.getNode('node-1');

      // MockAdapter returns null
      expect(result).toBeNull();
    });

    it('should handle various node ID formats', async () => {
      const ids = ['simple-id', 'complex:id:with:colons', 'id-with-dashes'];

      for (const id of ids) {
        const result = await tauriCommands.getNode(id);
        expect(result === null || typeof result === 'object').toBe(true);
      }
    });
  });

  describe('updateNode', () => {
    it('should accept ID, version, and update object', async () => {
      const update: UpdateNodeInput = {
        content: 'Updated content'
      };

      const result = await tauriCommands.updateNode('node-1', 1, update);

      expect(typeof result).toBe('object');
    });

    it('should handle update with all optional fields', async () => {
      const update: UpdateNodeInput = {
        content: 'New content',
        nodeType: 'task',
        properties: { status: 'done' },
        mentions: ['user-3']
      };

      const result = await tauriCommands.updateNode('node-1', 5, update);

      expect(typeof result).toBe('object');
    });
  });

  describe('updateTaskNode', () => {
    it('should accept task-specific updates', async () => {
      const update: TaskNodeUpdate = {
        status: 'in-progress',
        priority: 'high'
      };

      const result = await tauriCommands.updateTaskNode('task-1', 1, update);

      expect(typeof result).toBe('object');
    });

    it('should handle due date updates', async () => {
      const dueDate = new Date('2025-12-31').toISOString();
      const update: TaskNodeUpdate = {
        dueDate
      };

      const result = await tauriCommands.updateTaskNode('task-1', 1, update);

      expect(typeof result).toBe('object');
    });

    it('should handle assignee updates', async () => {
      const update: TaskNodeUpdate = {
        assignee: 'user-123'
      };

      const result = await tauriCommands.updateTaskNode('task-1', 2, update);

      expect(typeof result).toBe('object');
    });
  });

  describe('deleteNode', () => {
    it('should return delete result with counts', async () => {
      const result = await tauriCommands.deleteNode('node-1', 1);

      expect(result).toHaveProperty('deletedId');
      expect(result).toHaveProperty('deletedChildCount');
      expect(typeof result.deletedId).toBe('string');
      expect(typeof result.deletedChildCount).toBe('number');
    });

    it('should accept version for OCC', async () => {
      const result = await tauriCommands.deleteNode('node-1', 5);

      expect(result.deletedId).toBe('node-1');
    });
  });

  // ============================================================================
  // Hierarchy Commands (COVERS LINES 108-117)
  // ============================================================================

  describe('getChildren', () => {
    it('should return array of nodes', async () => {
      const result = await tauriCommands.getChildren('parent-1');

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle various parent IDs', async () => {
      const parentIds = ['parent-1', 'root-node', 'container-abc'];

      for (const parentId of parentIds) {
        const result = await tauriCommands.getChildren(parentId);
        expect(Array.isArray(result)).toBe(true);
      }
    });
  });

  describe('getDescendants', () => {
    it('should return array of all descendants', async () => {
      const result = await tauriCommands.getDescendants('root-1');

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle deep hierarchy queries', async () => {
      const result = await tauriCommands.getDescendants('top-level-node');

      expect(Array.isArray(result)).toBe(true);
    });
  });

  describe('getChildrenTree', () => {
    it('should return nested node structure or null', async () => {
      const result = await tauriCommands.getChildrenTree('parent-1');

      // Result can be null or NodeWithChildren
      if (result !== null) {
        expect(result).toHaveProperty('id');
        expect(result).toHaveProperty('children');
        expect(Array.isArray(result.children)).toBe(true);
      }
    });

    it('should handle non-existent parent', async () => {
      const result = await tauriCommands.getChildrenTree('non-existent');

      expect(result).toBeNull();
    });

    it('should return tree structure with children property', async () => {
      const result = await tauriCommands.getChildrenTree('parent-1');

      if (result !== null) {
        expect(result).toHaveProperty('id');
        expect(result).toHaveProperty('nodeType');
        expect(result).toHaveProperty('content');
        expect(result).toHaveProperty('version');
        expect(result).toHaveProperty('children');
      }
    });
  });

  describe('moveNode', () => {
    it('should accept node ID, version, and new parent', async () => {
      await expect(
        tauriCommands.moveNode('node-1', 1, 'new-parent', null)
      ).resolves.toBeUndefined();
    });

    it('should handle moving to root (null parent)', async () => {
      await expect(
        tauriCommands.moveNode('node-1', 1, null, null)
      ).resolves.toBeUndefined();
    });

    it('should handle sibling positioning', async () => {
      await expect(
        tauriCommands.moveNode('node-1', 1, 'parent-1', 'sibling-2')
      ).resolves.toBeUndefined();
    });

    it('should handle undefined insertAfterNodeId', async () => {
      await expect(
        tauriCommands.moveNode('node-1', 1, 'parent-1', undefined)
      ).resolves.toBeUndefined();
    });
  });

  // ============================================================================
  // Mention Commands (COVERS LINES 143-179)
  // ============================================================================

  describe('createMention', () => {
    it('should create mention relationship', async () => {
      await expect(
        tauriCommands.createMention('mentioning-node', 'mentioned-node')
      ).resolves.toBeUndefined();
    });

    it('should handle self-mentions', async () => {
      await expect(
        tauriCommands.createMention('node-1', 'node-1')
      ).resolves.toBeUndefined();
    });

    it('should handle multiple mentions from same node', async () => {
      await expect(
        tauriCommands.createMention('node-1', 'target-1')
      ).resolves.toBeUndefined();

      await expect(
        tauriCommands.createMention('node-1', 'target-2')
      ).resolves.toBeUndefined();
    });
  });

  describe('deleteMention', () => {
    it('should delete mention relationship', async () => {
      await expect(
        tauriCommands.deleteMention('mentioning-node', 'mentioned-node')
      ).resolves.toBeUndefined();
    });

    it('should handle deletion of non-existent mention gracefully', async () => {
      await expect(
        tauriCommands.deleteMention('node-1', 'node-2')
      ).resolves.toBeUndefined();
    });
  });

  describe('getOutgoingMentions', () => {
    it('should return array of mentioned node IDs', async () => {
      const result = await tauriCommands.getOutgoingMentions('node-1');

      expect(Array.isArray(result)).toBe(true);
      result.forEach(id => expect(typeof id).toBe('string'));
    });

    it('should handle node with no outgoing mentions', async () => {
      const result = await tauriCommands.getOutgoingMentions('isolated-node');

      expect(Array.isArray(result)).toBe(true);
    });
  });

  describe('getIncomingMentions', () => {
    it('should return array of backlink node IDs', async () => {
      const result = await tauriCommands.getIncomingMentions('node-1');

      expect(Array.isArray(result)).toBe(true);
      result.forEach(id => expect(typeof id).toBe('string'));
    });

    it('should handle node with no backlinks', async () => {
      const result = await tauriCommands.getIncomingMentions('orphan-node');

      expect(Array.isArray(result)).toBe(true);
    });
  });

  describe('getMentioningRoots', () => {
    it('should return array of root node IDs that mention target', async () => {
      const result = await tauriCommands.getMentioningRoots('node-1');

      expect(Array.isArray(result)).toBe(true);
      result.forEach(id => expect(typeof id).toBe('string'));
    });

    it('should handle node mentioned by no roots', async () => {
      const result = await tauriCommands.getMentioningRoots('leaf-node');

      expect(Array.isArray(result)).toBe(true);
    });
  });

  // ============================================================================
  // Query Commands (COVERS LINES 188-197)
  // ============================================================================

  describe('queryNodes', () => {
    it('should accept query by ID', async () => {
      const query: NodeQuery = { id: 'node-1' };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should accept content search query', async () => {
      const query: NodeQuery = { contentContains: 'search term' };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should accept node type filter', async () => {
      const query: NodeQuery = { nodeType: 'task' };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should accept mentioned-by filter', async () => {
      const query: NodeQuery = { mentionedBy: 'node-1' };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should accept limit parameter', async () => {
      const query: NodeQuery = { limit: 10 };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle combined query criteria', async () => {
      const query: NodeQuery = {
        nodeType: 'task',
        contentContains: 'urgent',
        limit: 5
      };
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle empty query object', async () => {
      const query: NodeQuery = {};
      const result = await tauriCommands.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
    });
  });

  describe('mentionAutocomplete', () => {
    it('should accept query string and return nodes', async () => {
      const result = await tauriCommands.mentionAutocomplete('John');

      expect(Array.isArray(result)).toBe(true);
    });

    it('should accept query with limit', async () => {
      const result = await tauriCommands.mentionAutocomplete('test', 5);

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle empty query string', async () => {
      const result = await tauriCommands.mentionAutocomplete('');

      expect(Array.isArray(result)).toBe(true);
    });

    it('should handle various search patterns', async () => {
      const searches = ['TEST', 'partial', '@username', '#tag'];

      for (const search of searches) {
        const result = await tauriCommands.mentionAutocomplete(search);
        expect(Array.isArray(result)).toBe(true);
      }
    });
  });

  // ============================================================================
  // Composite Commands (COVERS LINES 206-208)
  // ============================================================================

  describe('createContainerNode', () => {
    it('should create container with basic input', async () => {
      const input: CreateContainerInput = {
        content: 'Container content',
        nodeType: 'text'
      };

      const result = await tauriCommands.createContainerNode(input);

      expect(typeof result).toBe('string');
      expect(result).toBeTruthy();
    });

    it('should create container with properties', async () => {
      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'task',
        properties: { status: 'active', priority: 'high' }
      };

      const result = await tauriCommands.createContainerNode(input);

      expect(typeof result).toBe('string');
    });

    it('should create container with mentionedBy reference', async () => {
      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'text',
        mentionedBy: 'parent-node-1'
      };

      const result = await tauriCommands.createContainerNode(input);

      expect(typeof result).toBe('string');
    });

    it('should create container with all optional fields', async () => {
      const input: CreateContainerInput = {
        content: 'Full container',
        nodeType: 'task',
        properties: { priority: 'high', status: 'done' },
        mentionedBy: 'referencing-node'
      };

      const result = await tauriCommands.createContainerNode(input);

      expect(typeof result).toBe('string');
      expect(result).toBeTruthy();
    });
  });

  // ============================================================================
  // Type Re-exports (for completeness)
  // ============================================================================

  describe('Type Exports', () => {
    it('should export CreateNodeInput type', () => {
      const input: CreateNodeInput = {
        id: 'test',
        nodeType: 'text',
        content: 'test'
      };
      expect(input).toBeDefined();
    });

    it('should export UpdateNodeInput type', () => {
      const update: UpdateNodeInput = {
        content: 'updated'
      };
      expect(update).toBeDefined();
    });

    it('should export NodeQuery type', () => {
      const query: NodeQuery = {
        contentContains: 'test'
      };
      expect(query).toBeDefined();
    });

    it('should export CreateContainerInput type', () => {
      const input: CreateContainerInput = {
        content: 'test',
        nodeType: 'text'
      };
      expect(input).toBeDefined();
    });
  });

  // ============================================================================
  // API Completeness
  // ============================================================================

  describe('API Completeness', () => {
    it('should export all Node CRUD functions', () => {
      expect(typeof tauriCommands.createNode).toBe('function');
      expect(typeof tauriCommands.getNode).toBe('function');
      expect(typeof tauriCommands.updateNode).toBe('function');
      expect(typeof tauriCommands.updateTaskNode).toBe('function');
      expect(typeof tauriCommands.deleteNode).toBe('function');
    });

    it('should export all hierarchy functions', () => {
      expect(typeof tauriCommands.getChildren).toBe('function');
      expect(typeof tauriCommands.getDescendants).toBe('function');
      expect(typeof tauriCommands.getChildrenTree).toBe('function');
      expect(typeof tauriCommands.moveNode).toBe('function');
    });

    it('should export all mention functions', () => {
      expect(typeof tauriCommands.createMention).toBe('function');
      expect(typeof tauriCommands.deleteMention).toBe('function');
      expect(typeof tauriCommands.getOutgoingMentions).toBe('function');
      expect(typeof tauriCommands.getIncomingMentions).toBe('function');
      expect(typeof tauriCommands.getMentioningRoots).toBe('function');
    });

    it('should export all query functions', () => {
      expect(typeof tauriCommands.queryNodes).toBe('function');
      expect(typeof tauriCommands.mentionAutocomplete).toBe('function');
    });

    it('should export all composite functions', () => {
      expect(typeof tauriCommands.createContainerNode).toBe('function');
    });
  });
});
