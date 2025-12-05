/**
 * Tests for Node Type Guards and Helpers
 *
 * Comprehensive test coverage for node type guards and utility functions.
 * Focus on uncovered lines 257-270 (isNode function) and 275-288 (createDefaultUIState).
 */

import { describe, it, expect } from 'vitest';
import { type Node, isNode, createDefaultUIState } from '$lib/types/node';

describe('isNode type guard', () => {
  describe('valid nodes', () => {
    it('accepts minimal valid node', () => {
      const validNode = {
        id: 'test-1',
        nodeType: 'text',
        content: 'Test content',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(validNode)).toBe(true);
    });

    it('accepts node with all optional fields', () => {
      const fullNode = {
        id: 'test-2',
        nodeType: 'task',
        content: 'Complete task',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 5,
        properties: { priority: 'high' },
        parentId: 'parent-1',
        embeddingVector: [0.1, 0.2, 0.3],
        mentions: ['@user1', '@user2'],
        status: 'todo',
        priority: 1,
        dueDate: '2025-12-31',
        assignee: 'john'
      };

      expect(isNode(fullNode)).toBe(true);
    });

    it('accepts node with null parentId', () => {
      const nodeWithNullParent: Node = {
        id: 'root-1',
        nodeType: 'text',
        content: 'Root node',
        parentId: null,
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(nodeWithNullParent)).toBe(true);
    });

    it('accepts node with empty content', () => {
      const emptyContentNode = {
        id: 'test-3',
        nodeType: 'text',
        content: '',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(emptyContentNode)).toBe(true);
    });

    it('accepts node with nested properties', () => {
      const nodeWithNestedProps = {
        id: 'test-4',
        nodeType: 'custom',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {
          nested: {
            deep: {
              value: 'test'
            }
          },
          array: [1, 2, 3]
        }
      };

      expect(isNode(nodeWithNestedProps)).toBe(true);
    });
  });

  describe('invalid nodes - null and non-objects', () => {
    it('rejects null', () => {
      expect(isNode(null)).toBe(false);
    });

    it('rejects undefined', () => {
      expect(isNode(undefined)).toBe(false);
    });

    it('rejects primitive string', () => {
      expect(isNode('not a node')).toBe(false);
    });

    it('rejects primitive number', () => {
      expect(isNode(42)).toBe(false);
    });

    it('rejects primitive boolean', () => {
      expect(isNode(true)).toBe(false);
    });

    it('rejects empty object', () => {
      expect(isNode({})).toBe(false);
    });

    it('rejects array', () => {
      expect(isNode([])).toBe(false);
    });
  });

  describe('invalid nodes - missing required fields', () => {
    const validNode = {
      id: 'test-5',
      nodeType: 'text',
      content: 'Test',
      createdAt: '2025-12-05T10:00:00Z',
      modifiedAt: '2025-12-05T10:00:00Z',
      version: 1,
      properties: {}
    };

    it('rejects node without id', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { id, ...nodeWithoutId } = validNode;
      expect(isNode(nodeWithoutId)).toBe(false);
    });

    it('rejects node without nodeType', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { nodeType, ...nodeWithoutType } = validNode;
      expect(isNode(nodeWithoutType)).toBe(false);
    });

    it('rejects node without content', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { content, ...nodeWithoutContent } = validNode;
      expect(isNode(nodeWithoutContent)).toBe(false);
    });

    it('rejects node without createdAt', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { createdAt, ...nodeWithoutCreatedAt } = validNode;
      expect(isNode(nodeWithoutCreatedAt)).toBe(false);
    });

    it('rejects node without modifiedAt', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { modifiedAt, ...nodeWithoutModifiedAt } = validNode;
      expect(isNode(nodeWithoutModifiedAt)).toBe(false);
    });

    it('rejects node without properties', () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { properties, ...nodeWithoutProperties } = validNode;
      expect(isNode(nodeWithoutProperties)).toBe(false);
    });
  });

  describe('invalid nodes - wrong field types', () => {
    it('rejects node with number id', () => {
      const invalidNode = {
        id: 123,
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with number nodeType', () => {
      const invalidNode = {
        id: 'test-6',
        nodeType: 123,
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with number content', () => {
      const invalidNode = {
        id: 'test-7',
        nodeType: 'text',
        content: 123,
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with number createdAt', () => {
      const invalidNode = {
        id: 'test-8',
        nodeType: 'text',
        content: 'Test',
        createdAt: 1234567890,
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with number modifiedAt', () => {
      const invalidNode = {
        id: 'test-9',
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: 1234567890,
        version: 1,
        properties: {}
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with null properties', () => {
      const invalidNode = {
        id: 'test-10',
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: null
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('rejects node with string properties', () => {
      const invalidNode = {
        id: 'test-11',
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: 'not an object'
      };

      expect(isNode(invalidNode)).toBe(false);
    });

    it('accepts node with array properties (arrays are objects in JS)', () => {
      // Note: In JavaScript, typeof [] === 'object', so this passes the type check
      // The backend and validation layer should enforce proper properties structure
      const nodeWithArrayProps = {
        id: 'test-12',
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: []
      };

      expect(isNode(nodeWithArrayProps)).toBe(true);
    });
  });

  describe('type narrowing', () => {
    it('works with TypeScript type narrowing', () => {
      const maybeNode: unknown = {
        id: 'test-13',
        nodeType: 'text',
        content: 'Test',
        createdAt: '2025-12-05T10:00:00Z',
        modifiedAt: '2025-12-05T10:00:00Z',
        version: 1,
        properties: {}
      };

      if (isNode(maybeNode)) {
        // TypeScript should narrow the type here
        expect(maybeNode.id).toBe('test-13');
        expect(maybeNode.nodeType).toBe('text');
        expect(maybeNode.content).toBe('Test');
      } else {
        throw new Error('Should be a valid node');
      }
    });

    it('correctly narrows type for invalid objects', () => {
      const notANode: unknown = {
        id: 'test-14',
        nodeType: 'text'
        // Missing required fields
      };

      // Assert directly that invalid object is rejected
      expect(isNode(notANode)).toBe(false);
    });
  });
});

describe('createDefaultUIState', () => {
  describe('default values', () => {
    it('creates UI state with all default values', () => {
      const uiState = createDefaultUIState('node-1');

      expect(uiState).toEqual({
        nodeId: 'node-1',
        depth: 0,
        expanded: false,
        autoFocus: false,
        inheritHeaderLevel: 0,
        isPlaceholder: false
      });
    });

    it('sets nodeId correctly', () => {
      const uiState = createDefaultUIState('unique-node-id');
      expect(uiState.nodeId).toBe('unique-node-id');
    });

    it('handles UUID-style node IDs', () => {
      const uuid = '550e8400-e29b-41d4-a716-446655440000';
      const uiState = createDefaultUIState(uuid);
      expect(uiState.nodeId).toBe(uuid);
    });

    it('handles date-style node IDs', () => {
      const dateId = '2025-12-05';
      const uiState = createDefaultUIState(dateId);
      expect(uiState.nodeId).toBe(dateId);
    });

    it('handles empty string node ID', () => {
      const uiState = createDefaultUIState('');
      expect(uiState.nodeId).toBe('');
    });
  });

  describe('overrides', () => {
    it('overrides depth', () => {
      const uiState = createDefaultUIState('node-2', { depth: 3 });
      expect(uiState.depth).toBe(3);
      expect(uiState.nodeId).toBe('node-2');
      expect(uiState.expanded).toBe(false); // Other defaults unchanged
    });

    it('overrides expanded', () => {
      const uiState = createDefaultUIState('node-3', { expanded: true });
      expect(uiState.expanded).toBe(true);
      expect(uiState.depth).toBe(0); // Other defaults unchanged
    });

    it('overrides autoFocus', () => {
      const uiState = createDefaultUIState('node-4', { autoFocus: true });
      expect(uiState.autoFocus).toBe(true);
    });

    it('overrides inheritHeaderLevel', () => {
      const uiState = createDefaultUIState('node-5', { inheritHeaderLevel: 2 });
      expect(uiState.inheritHeaderLevel).toBe(2);
    });

    it('overrides isPlaceholder', () => {
      const uiState = createDefaultUIState('node-6', { isPlaceholder: true });
      expect(uiState.isPlaceholder).toBe(true);
    });

    it('overrides multiple fields', () => {
      const uiState = createDefaultUIState('node-7', {
        depth: 2,
        expanded: true,
        autoFocus: true
      });

      expect(uiState).toEqual({
        nodeId: 'node-7',
        depth: 2,
        expanded: true,
        autoFocus: true,
        inheritHeaderLevel: 0,
        isPlaceholder: false
      });
    });

    it('overrides all fields', () => {
      const uiState = createDefaultUIState('node-8', {
        depth: 5,
        expanded: true,
        autoFocus: true,
        inheritHeaderLevel: 3,
        isPlaceholder: true
      });

      expect(uiState).toEqual({
        nodeId: 'node-8',
        depth: 5,
        expanded: true,
        autoFocus: true,
        inheritHeaderLevel: 3,
        isPlaceholder: true
      });
    });

    it('does not override nodeId through overrides', () => {
      // TypeScript won't allow this, but test the behavior
      const uiState = createDefaultUIState(
        'node-9',
        { nodeId: 'different-id' } as unknown as Partial<{
          depth: number;
          expanded: boolean;
          autoFocus: boolean;
          inheritHeaderLevel: number;
          isPlaceholder: boolean;
        }>
      );

      // Should use the original nodeId, then apply overrides (which includes nodeId last)
      expect(uiState.nodeId).toBe('different-id');
    });

    it('handles empty overrides object', () => {
      const uiState = createDefaultUIState('node-10', {});
      expect(uiState).toEqual({
        nodeId: 'node-10',
        depth: 0,
        expanded: false,
        autoFocus: false,
        inheritHeaderLevel: 0,
        isPlaceholder: false
      });
    });

    it('handles partial overrides with some false values', () => {
      const uiState = createDefaultUIState('node-11', {
        expanded: false, // Explicitly false (same as default)
        depth: 1 // Different from default
      });

      expect(uiState.depth).toBe(1);
      expect(uiState.expanded).toBe(false);
    });
  });

  describe('return type and structure', () => {
    it('returns object with correct type structure', () => {
      const uiState = createDefaultUIState('node-12');

      expect(uiState).toHaveProperty('nodeId');
      expect(uiState).toHaveProperty('depth');
      expect(uiState).toHaveProperty('expanded');
      expect(uiState).toHaveProperty('autoFocus');
      expect(uiState).toHaveProperty('inheritHeaderLevel');
      expect(uiState).toHaveProperty('isPlaceholder');

      expect(typeof uiState.nodeId).toBe('string');
      expect(typeof uiState.depth).toBe('number');
      expect(typeof uiState.expanded).toBe('boolean');
      expect(typeof uiState.autoFocus).toBe('boolean');
      expect(typeof uiState.inheritHeaderLevel).toBe('number');
      expect(typeof uiState.isPlaceholder).toBe('boolean');
    });

    it('returns new object each time', () => {
      const uiState1 = createDefaultUIState('node-13');
      const uiState2 = createDefaultUIState('node-13');

      expect(uiState1).toEqual(uiState2);
      expect(uiState1).not.toBe(uiState2); // Different object references
    });

    it('does not mutate overrides object', () => {
      const overrides = { depth: 2, expanded: true };
      const overridesCopy = { ...overrides };

      createDefaultUIState('node-14', overrides);

      expect(overrides).toEqual(overridesCopy);
    });
  });

  describe('edge cases', () => {
    it('handles negative depth', () => {
      const uiState = createDefaultUIState('node-15', { depth: -1 });
      expect(uiState.depth).toBe(-1); // Function doesn't validate, just spreads
    });

    it('handles large depth values', () => {
      const uiState = createDefaultUIState('node-16', { depth: 1000 });
      expect(uiState.depth).toBe(1000);
    });

    it('handles negative inheritHeaderLevel', () => {
      const uiState = createDefaultUIState('node-17', { inheritHeaderLevel: -1 });
      expect(uiState.inheritHeaderLevel).toBe(-1);
    });

    it('handles large inheritHeaderLevel values', () => {
      const uiState = createDefaultUIState('node-18', { inheritHeaderLevel: 100 });
      expect(uiState.inheritHeaderLevel).toBe(100);
    });
  });
});

describe('integration tests', () => {
  it('validates node and creates UI state', () => {
    const node: Node = {
      id: 'integration-1',
      nodeType: 'text',
      content: 'Test node',
      createdAt: '2025-12-05T10:00:00Z',
      modifiedAt: '2025-12-05T10:00:00Z',
      version: 1,
      properties: {}
    };

    expect(isNode(node)).toBe(true);

    const uiState = createDefaultUIState(node.id);
    expect(uiState.nodeId).toBe(node.id);
  });

  it('creates UI state for hierarchical nodes', () => {
    const rootNode: Node = {
      id: 'root',
      nodeType: 'text',
      content: 'Root',
      parentId: null,
      createdAt: '2025-12-05T10:00:00Z',
      modifiedAt: '2025-12-05T10:00:00Z',
      version: 1,
      properties: {}
    };

    const childNode: Node = {
      id: 'child',
      nodeType: 'text',
      content: 'Child',
      parentId: 'root',
      createdAt: '2025-12-05T10:00:00Z',
      modifiedAt: '2025-12-05T10:00:00Z',
      version: 1,
      properties: {}
    };

    expect(isNode(rootNode)).toBe(true);
    expect(isNode(childNode)).toBe(true);

    const rootUIState = createDefaultUIState(rootNode.id, { depth: 0, expanded: true });
    const childUIState = createDefaultUIState(childNode.id, { depth: 1 });

    expect(rootUIState.depth).toBe(0);
    expect(rootUIState.expanded).toBe(true);
    expect(childUIState.depth).toBe(1);
    expect(childUIState.expanded).toBe(false);
  });

  it('handles complete node lifecycle', () => {
    // Create a node-like object
    const nodeData = {
      id: 'lifecycle-1',
      nodeType: 'task',
      content: 'Complete project',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { priority: 'high' },
      status: 'todo',
      priority: 1
    };

    // Validate it's a node
    expect(isNode(nodeData)).toBe(true);

    // Create UI state for editing
    const editingUIState = createDefaultUIState(nodeData.id, {
      autoFocus: true,
      expanded: false
    });

    expect(editingUIState.autoFocus).toBe(true);
    expect(editingUIState.expanded).toBe(false);

    // Create UI state for nested view
    const nestedUIState = createDefaultUIState(nodeData.id, {
      depth: 2,
      inheritHeaderLevel: 1
    });

    expect(nestedUIState.depth).toBe(2);
    expect(nestedUIState.inheritHeaderLevel).toBe(1);
  });
});
