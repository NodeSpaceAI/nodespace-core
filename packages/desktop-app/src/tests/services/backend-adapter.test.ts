/**
 * Backend Adapter Tests
 *
 * Comprehensive test suite for the backend adapter module that provides
 * a unified interface for backend communication across Tauri (IPC), HTTP (dev server),
 * and mock (test) environments.
 *
 * Tests cover:
 * - TauriAdapter (IPC communication) - via mocked Tauri environment
 * - HttpAdapter (HTTP fetch communication) - via mocked non-Tauri environment
 * - MockAdapter (test environment) - via NODE_ENV=test
 * - Environment detection functions
 * - Error handling and edge cases
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Node, TaskNode, TaskNodeUpdate, NodeWithChildren } from '$lib/types';
import type {
  CreateNodeInput,
  UpdateNodeInput,
  DeleteResult,
  NodeQuery,
  CreateContainerInput
} from '$lib/services/backend-adapter';
import type { SchemaNode } from '$lib/types/schema-node';

// Declare globals for eslint (these are available in Happy-DOM/browser environment)
declare const Headers: typeof globalThis.Headers;

describe('Backend Adapter - MockAdapter (Test Environment)', () => {
  let originalEnv: string | undefined;

  beforeEach(() => {
    originalEnv = process.env.NODE_ENV;
    vi.stubEnv('NODE_ENV', 'test');
    vi.resetModules();
  });

  afterEach(() => {
    if (originalEnv) {
      vi.stubEnv('NODE_ENV', originalEnv);
    }
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  describe('Node CRUD Operations', () => {
    it('should return mock ID from createNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const input: CreateNodeInput = {
        id: 'test-id',
        nodeType: 'text',
        content: 'Test content'
      };

      const result = await adapter.createNode(input);

      expect(result).toBe('mock-id');
    });

    it('should return null from getNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getNode('any-id');

      expect(result).toBeNull();
    });

    it('should return empty node object from updateNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();
      const update: UpdateNodeInput = { content: 'Updated' };

      const result = await adapter.updateNode('node-1', 1, update);

      expect(result).toBeDefined();
      expect(typeof result).toBe('object');
    });

    it('should return empty task node from updateTaskNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();
      const update: TaskNodeUpdate = { status: 'done' };

      const result = await adapter.updateTaskNode('task-1', 1, update);

      expect(result).toBeDefined();
      expect(typeof result).toBe('object');
    });

    it('should return delete result with correct ID', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.deleteNode('delete-me', 1);

      expect(result).toEqual({
        deletedId: 'delete-me',
        deletedChildCount: 0
      });
    });
  });

  describe('Hierarchy Operations', () => {
    it('should return empty array from getChildren', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getChildren('parent-id');

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return empty array from getDescendants', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getDescendants('root-id');

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return tree structure from getChildrenTree with valid parent', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getChildrenTree('parent-id');

      expect(result).not.toBeNull();
      expect(result).toHaveProperty('id', 'parent-id');
      expect(result).toHaveProperty('children');
      expect(Array.isArray(result?.children)).toBe(true);
    });

    it('should return null from getChildrenTree with non-existent parent', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getChildrenTree('non-existent');

      expect(result).toBeNull();
    });

    it('should handle moveNode and return updated node', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      // moveNode now returns the updated Node with new version
      const result = await adapter.moveNode('node-1', 1, 'new-parent', null);
      expect(result).toBeDefined();
    });
  });

  describe('Mention Operations', () => {
    it('should handle createMention without error', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      await expect(
        adapter.createMention('source', 'target')
      ).resolves.toBeUndefined();
    });

    it('should handle deleteMention without error', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      await expect(
        adapter.deleteMention('source', 'target')
      ).resolves.toBeUndefined();
    });

    it('should return empty array from getOutgoingMentions', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getOutgoingMentions('node-id');

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return empty array from getIncomingMentions', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getIncomingMentions('node-id');

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return empty array from getMentioningContainers', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getMentioningContainers('node-id');

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });
  });

  describe('Query Operations', () => {
    it('should return empty array from queryNodes', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();
      const query: NodeQuery = { contentContains: 'test' };

      const result = await adapter.queryNodes(query);

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return empty array from mentionAutocomplete', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.mentionAutocomplete('search', 10);

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });
  });

  describe('Composite Operations', () => {
    it('should return mock container ID from createContainerNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();
      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'text'
      };

      const result = await adapter.createContainerNode(input);

      expect(result).toBe('mock-container-id');
    });
  });

  describe('Schema Operations', () => {
    it('should return empty array from getAllSchemas', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getAllSchemas();

      expect(Array.isArray(result)).toBe(true);
      expect(result).toHaveLength(0);
    });

    it('should return mock schema from getSchema', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const result = await adapter.getSchema('test-schema');

      expect(result).toBeDefined();
      expect(result.id).toBe('test-schema');
      expect(result.nodeType).toBe('schema');
      expect(result.isCore).toBe(false);
      expect(result.schemaVersion).toBe(1);
      expect(Array.isArray(result.fields)).toBe(true);
    });
  });
});

describe('Backend Adapter - HttpAdapter (Browser Dev Mode)', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let originalFetch: typeof globalThis.fetch;
  let originalEnv: string | undefined;

  beforeEach(() => {
    originalEnv = process.env.NODE_ENV;
    originalFetch = globalThis.fetch;
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as typeof globalThis.fetch;

    // Not in test mode, not in Tauri -> should use HttpAdapter
    vi.stubEnv('NODE_ENV', 'development');

    // Ensure no Tauri environment
    delete (globalThis.window as Window & { __TAURI__?: unknown }).__TAURI__;
    delete (globalThis.window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    vi.resetModules();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    if (originalEnv) {
      vi.stubEnv('NODE_ENV', originalEnv);
    }
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  describe('Node CRUD Operations', () => {
    it('should create node with POST request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => 'created-node-id'
      });

      const input: CreateNodeInput = {
        id: 'test-id',
        nodeType: 'text',
        content: 'Test content'
      };

      const result = await adapter.createNode(input);

      expect(result).toBe('created-node-id');
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' }
        })
      );
    });

    it('should get node with GET request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockNode: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {}
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockNode
      });

      const result = await adapter.getNode('node-1');

      expect(result).toEqual(mockNode);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1'
      );
    });

    it('should return null for 404 response', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 404,
        statusText: 'Not Found'
      });

      const result = await adapter.getNode('non-existent');

      expect(result).toBeNull();
    });

    it('should update node with PATCH request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockNode: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Updated content',
        version: 2,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-02T00:00:00Z',
        properties: {}
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockNode
      });

      const update: UpdateNodeInput = { content: 'Updated content' };
      const result = await adapter.updateNode('node-1', 1, update);

      expect(result).toEqual(mockNode);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1',
        expect.objectContaining({
          method: 'PATCH'
        })
      );
    });

    it('should update task node with PATCH to /api/tasks', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockTaskNode: TaskNode = {
        id: 'task-1',
        nodeType: 'task',
        content: 'Task content',
        version: 2,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-02T00:00:00Z',
        status: 'done',
        priority: 'high'
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockTaskNode
      });

      const update: TaskNodeUpdate = { status: 'done' };
      const result = await adapter.updateTaskNode('task-1', 1, update);

      expect(result).toEqual(mockTaskNode);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/tasks/task-1',
        expect.objectContaining({
          method: 'PATCH'
        })
      );
    });

    it('should delete node with DELETE request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResult: DeleteResult = {
        deletedId: 'node-1',
        deletedChildCount: 3
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockResult
      });

      const result = await adapter.deleteNode('node-1', 1);

      expect(result).toEqual(mockResult);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1',
        expect.objectContaining({
          method: 'DELETE'
        })
      );
    });

    it('should encode special characters in node IDs', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 404
      });

      await adapter.getNode('node/with/slashes');

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node%2Fwith%2Fslashes'
      );
    });
  });

  describe('Hierarchy Operations', () => {
    it('should get children with GET request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockChildren: Node[] = [
        {
          id: 'child-1',
          nodeType: 'text',
          content: 'Child 1',
          version: 1,
          createdAt: '2025-01-01T00:00:00Z',
          modifiedAt: '2025-01-01T00:00:00Z',
          properties: {}
        }
      ];

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockChildren
      });

      const result = await adapter.getChildren('parent-1');

      expect(result).toEqual(mockChildren);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/parent-1/children'
      );
    });

    it('should get descendants recursively', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const level1: Node[] = [
        { id: 'child-1', nodeType: 'text', content: 'Child 1', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} },
        { id: 'child-2', nodeType: 'text', content: 'Child 2', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];
      const level2a: Node[] = [
        { id: 'grandchild-1', nodeType: 'text', content: 'Grandchild 1', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];
      const level2b: Node[] = [];

      mockFetch
        .mockResolvedValueOnce({ ok: true, status: 200, headers: new Headers(), json: async () => level1 })
        .mockResolvedValueOnce({ ok: true, status: 200, headers: new Headers(), json: async () => level2a })
        .mockResolvedValueOnce({ ok: true, status: 200, headers: new Headers(), json: async () => level2b })
        .mockResolvedValueOnce({ ok: true, status: 200, headers: new Headers(), json: async () => [] });

      const result = await adapter.getDescendants('root-1');

      expect(result).toHaveLength(3);
      expect(result.map(n => n.id)).toEqual(['child-1', 'child-2', 'grandchild-1']);
    });

    it('should get children tree structure', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockTree: NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {},
        children: []
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockTree
      });

      const result = await adapter.getChildrenTree('parent-1');

      expect(result).toEqual(mockTree);
    });

    it('should return null for empty tree response', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => ({})
      });

      const result = await adapter.getChildrenTree('non-existent');

      expect(result).toBeNull();
    });

    it('should move node with POST request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 204,
        headers: new Headers({ 'content-length': '0' })
      });

      await adapter.moveNode('node-1', 1, 'new-parent', 'sibling-1');

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1/parent',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({
            version: 1,
            parentId: 'new-parent',
            insertAfterNodeId: 'sibling-1'
          })
        })
      );
    });
  });

  describe('Mention Operations', () => {
    it('should create mention with POST request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 204,
        headers: new Headers({ 'content-length': '0' })
      });

      await adapter.createMention('source-1', 'target-1');

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/mentions',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({ sourceId: 'source-1', targetId: 'target-1' })
        })
      );
    });

    it('should delete mention with DELETE request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 204,
        headers: new Headers({ 'content-length': '0' })
      });

      await adapter.deleteMention('source-1', 'target-1');

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/mentions',
        expect.objectContaining({
          method: 'DELETE'
        })
      );
    });

    it('should get outgoing mentions', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => ['target-1', 'target-2']
      });

      const result = await adapter.getOutgoingMentions('node-1');

      expect(result).toEqual(['target-1', 'target-2']);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1/mentions/outgoing'
      );
    });

    it('should get incoming mentions', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => ['source-1', 'source-2']
      });

      const result = await adapter.getIncomingMentions('node-1');

      expect(result).toEqual(['source-1', 'source-2']);
    });

    it('should get mentioning containers', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => ['root-1', 'root-2']
      });

      const result = await adapter.getMentioningContainers('node-1');

      expect(result).toEqual(['root-1', 'root-2']);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes/node-1/mentions/roots'
      );
    });
  });

  describe('Query Operations', () => {
    it('should query nodes with POST request', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResults: Node[] = [
        { id: 'result-1', nodeType: 'text', content: 'Result', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockResults
      });

      const query: NodeQuery = { contentContains: 'test', limit: 10 };
      const result = await adapter.queryNodes(query);

      expect(result).toEqual(mockResults);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/query',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify(query)
        })
      );
    });

    it('should perform mention autocomplete', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResults: Node[] = [
        { id: 'match-1', nodeType: 'text', content: 'Match', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockResults
      });

      const result = await adapter.mentionAutocomplete('match', 5);

      expect(result).toEqual(mockResults);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/mentions/autocomplete',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({ query: 'match', limit: 5 })
        })
      );
    });
  });

  describe('Composite Operations', () => {
    it('should create container node by delegating to createNode', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => 'container-id'
      });

      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'text',
        properties: { key: 'value' }
      };

      const result = await adapter.createContainerNode(input);

      expect(typeof result).toBe('string');
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/nodes',
        expect.objectContaining({
          method: 'POST'
        })
      );
    });
  });

  describe('Schema Operations', () => {
    it('should get all schemas', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockSchemas: SchemaNode[] = [
        {
          id: 'schema-1',
          nodeType: 'schema',
          content: 'Schema 1',
          version: 1,
          createdAt: '2025-01-01T00:00:00Z',
          modifiedAt: '2025-01-01T00:00:00Z',
          isCore: true,
          schemaVersion: 1,
          description: 'Test schema',
          fields: []
        }
      ];

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockSchemas
      });

      const result = await adapter.getAllSchemas();

      expect(result).toEqual(mockSchemas);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/schemas'
      );
    });

    it('should get specific schema', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockSchema: SchemaNode = {
        id: 'schema-1',
        nodeType: 'schema',
        content: 'Schema 1',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        isCore: false,
        schemaVersion: 1,
        description: 'Test schema',
        fields: []
      };

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers(),
        json: async () => mockSchema
      });

      const result = await adapter.getSchema('schema-1');

      expect(result).toEqual(mockSchema);
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/api/schemas/schema-1'
      );
    });
  });

  describe('Error Handling', () => {
    it('should throw error with message from JSON response', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 400,
        statusText: 'Bad Request',
        json: async () => ({ message: 'Invalid node data' })
      });

      await expect(
        adapter.createNode({ id: 'test', nodeType: 'text', content: 'Test' })
      ).rejects.toThrow('Invalid node data');
    });

    it('should throw error with status text when JSON parse fails', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: async () => {
          throw new SyntaxError('Invalid JSON');
        }
      });

      await expect(
        adapter.getNode('test-id')
      ).rejects.toThrow('HTTP 500: Internal Server Error');
    });

    it('should re-throw non-SyntaxError exceptions', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const customError = new Error('Custom error');

      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: async () => {
          throw customError;
        }
      });

      await expect(
        adapter.getNode('test-id')
      ).rejects.toThrow('Custom error');
    });

    it('should handle 204 No Content responses', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 204,
        headers: new Headers({ 'content-length': '0' })
      });

      const result = await adapter.moveNode('node-1', 1, 'parent-1', null);

      expect(result).toBeUndefined();
    });

    it('should handle responses with content-length 0', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        headers: new Headers({ 'content-length': '0' })
      });

      const result = await adapter.createMention('source', 'target');

      expect(result).toBeUndefined();
    });
  });
});

describe('Backend Adapter - TauriAdapter (Tauri IPC Mode)', () => {
  let mockInvoke: ReturnType<typeof vi.fn>;
  let originalEnv: string | undefined;

  beforeEach(async () => {
    originalEnv = process.env.NODE_ENV;

    // Not in test mode -> allows TauriAdapter selection
    vi.stubEnv('NODE_ENV', 'development');

    // Set up Tauri environment
    (globalThis.window as Window & { __TAURI__?: unknown }).__TAURI__ = {};

    mockInvoke = vi.fn();

    // Mock the Tauri core module BEFORE importing backend-adapter
    vi.doMock('@tauri-apps/api/core', () => ({
      invoke: mockInvoke
    }));

    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis.window as Window & { __TAURI__?: unknown }).__TAURI__;
    delete (globalThis.window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    if (originalEnv) {
      vi.stubEnv('NODE_ENV', originalEnv);
    }
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  describe('Node CRUD Operations', () => {
    it('should create node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce('created-node-id');

      const input: CreateNodeInput = {
        id: 'test-id',
        nodeType: 'text',
        content: 'Test content',
        properties: { key: 'value' },
        mentions: ['mention-1'],
        parentId: 'parent-1',
        insertAfterNodeId: 'sibling-1'
      };

      const result = await adapter.createNode(input);

      expect(result).toBe('created-node-id');
      expect(mockInvoke).toHaveBeenCalledWith('create_node', {
        node: {
          id: 'test-id',
          nodeType: 'text',
          content: 'Test content',
          properties: { key: 'value' },
          mentions: ['mention-1'],
          parentId: 'parent-1',
          insertAfterNodeId: 'sibling-1'
        }
      });
    });

    it('should handle createNode with default values', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce('node-id');

      const input: CreateNodeInput = {
        id: 'test-id',
        nodeType: 'text',
        content: 'Test'
      };

      await adapter.createNode(input);

      expect(mockInvoke).toHaveBeenCalledWith('create_node', {
        node: expect.objectContaining({
          properties: {},
          mentions: [],
          parentId: null,
          insertAfterNodeId: null
        })
      });
    });

    it('should get node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockNode: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {}
      };

      mockInvoke.mockResolvedValueOnce(mockNode);

      const result = await adapter.getNode('node-1');

      expect(result).toEqual(mockNode);
      expect(mockInvoke).toHaveBeenCalledWith('get_node', { id: 'node-1' });
    });

    it('should update node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockNode: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Updated',
        version: 2,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-02T00:00:00Z',
        properties: {}
      };

      mockInvoke.mockResolvedValueOnce(mockNode);

      const update: UpdateNodeInput = { content: 'Updated' };
      const result = await adapter.updateNode('node-1', 1, update);

      expect(result).toEqual(mockNode);
      expect(mockInvoke).toHaveBeenCalledWith('update_node', {
        id: 'node-1',
        version: 1,
        update
      });
    });

    it('should update task node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockTaskNode: TaskNode = {
        id: 'task-1',
        nodeType: 'task',
        content: 'Task',
        version: 2,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-02T00:00:00Z',
        status: 'done',
        priority: 'high'
      };

      mockInvoke.mockResolvedValueOnce(mockTaskNode);

      const update: TaskNodeUpdate = { status: 'done' };
      const result = await adapter.updateTaskNode('task-1', 1, update);

      expect(result).toEqual(mockTaskNode);
      expect(mockInvoke).toHaveBeenCalledWith('update_task_node', {
        id: 'task-1',
        version: 1,
        update
      });
    });

    it('should delete node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResult: DeleteResult = {
        deletedId: 'node-1',
        deletedChildCount: 5
      };

      mockInvoke.mockResolvedValueOnce(mockResult);

      const result = await adapter.deleteNode('node-1', 1);

      expect(result).toEqual(mockResult);
      expect(mockInvoke).toHaveBeenCalledWith('delete_node', {
        id: 'node-1',
        version: 1
      });
    });
  });

  describe('Hierarchy Operations', () => {
    it('should get children via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockChildren: Node[] = [
        { id: 'child-1', nodeType: 'text', content: 'Child', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockInvoke.mockResolvedValueOnce(mockChildren);

      const result = await adapter.getChildren('parent-1');

      expect(result).toEqual(mockChildren);
      expect(mockInvoke).toHaveBeenCalledWith('get_children', { parentId: 'parent-1' });
    });

    it('should get descendants recursively via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const level1: Node[] = [
        { id: 'child-1', nodeType: 'text', content: 'Child 1', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} },
        { id: 'child-2', nodeType: 'text', content: 'Child 2', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];
      const level2: Node[] = [
        { id: 'grandchild-1', nodeType: 'text', content: 'Grandchild', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockInvoke
        .mockResolvedValueOnce(level1)
        .mockResolvedValueOnce(level2)
        .mockResolvedValueOnce([])
        .mockResolvedValueOnce([]);

      const result = await adapter.getDescendants('root-1');

      expect(result).toHaveLength(3);
      expect(mockInvoke).toHaveBeenCalledTimes(4);
    });

    it('should get children tree via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockTree: NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {},
        children: []
      };

      mockInvoke.mockResolvedValueOnce(mockTree);

      const result = await adapter.getChildrenTree('parent-1');

      expect(result).toEqual(mockTree);
      expect(mockInvoke).toHaveBeenCalledWith('get_children_tree', { parentId: 'parent-1' });
    });

    it('should return null for empty tree response from IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce({});

      const result = await adapter.getChildrenTree('non-existent');

      expect(result).toBeNull();
    });

    it('should move node via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(undefined);

      await adapter.moveNode('node-1', 1, 'new-parent', 'sibling-1');

      expect(mockInvoke).toHaveBeenCalledWith('move_node', {
        nodeId: 'node-1',
        version: 1,
        newParentId: 'new-parent',
        insertAfterNodeId: 'sibling-1'
      });
    });
  });

  describe('Mention Operations', () => {
    it('should create mention via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(undefined);

      await adapter.createMention('source-1', 'target-1');

      expect(mockInvoke).toHaveBeenCalledWith('create_node_mention', {
        mentioningNodeId: 'source-1',
        mentionedNodeId: 'target-1'
      });
    });

    it('should delete mention via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(undefined);

      await adapter.deleteMention('source-1', 'target-1');

      expect(mockInvoke).toHaveBeenCalledWith('delete_node_mention', {
        mentioningNodeId: 'source-1',
        mentionedNodeId: 'target-1'
      });
    });

    it('should get outgoing mentions via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(['target-1', 'target-2']);

      const result = await adapter.getOutgoingMentions('node-1');

      expect(result).toEqual(['target-1', 'target-2']);
      expect(mockInvoke).toHaveBeenCalledWith('get_outgoing_mentions', { nodeId: 'node-1' });
    });

    it('should get incoming mentions via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(['source-1', 'source-2']);

      const result = await adapter.getIncomingMentions('node-1');

      expect(result).toEqual(['source-1', 'source-2']);
      expect(mockInvoke).toHaveBeenCalledWith('get_incoming_mentions', { nodeId: 'node-1' });
    });

    it('should get mentioning containers via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(['root-1', 'root-2']);

      const result = await adapter.getMentioningContainers('node-1');

      expect(result).toEqual(['root-1', 'root-2']);
      expect(mockInvoke).toHaveBeenCalledWith('get_mentioning_roots', { nodeId: 'node-1' });
    });
  });

  describe('Query Operations', () => {
    it('should query nodes via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResults: Node[] = [
        { id: 'result-1', nodeType: 'text', content: 'Result', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockInvoke.mockResolvedValueOnce(mockResults);

      const query: NodeQuery = { contentContains: 'test' };
      const result = await adapter.queryNodes(query);

      expect(result).toEqual(mockResults);
      expect(mockInvoke).toHaveBeenCalledWith('query_nodes_simple', { query });
    });

    it('should perform mention autocomplete via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockResults: Node[] = [
        { id: 'match-1', nodeType: 'text', content: 'Match', version: 1, createdAt: '2025-01-01T00:00:00Z', modifiedAt: '2025-01-01T00:00:00Z', properties: {} }
      ];

      mockInvoke.mockResolvedValueOnce(mockResults);

      const result = await adapter.mentionAutocomplete('match', 5);

      expect(result).toEqual(mockResults);
      expect(mockInvoke).toHaveBeenCalledWith('mention_autocomplete', {
        query: 'match',
        limit: 5
      });
    });
  });

  describe('Composite Operations', () => {
    it('should create container node via IPC with snake_case fields', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce('container-id');

      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'text',
        properties: { key: 'value' },
        mentionedBy: 'parent-1'
      };

      const result = await adapter.createContainerNode(input);

      expect(result).toBe('container-id');
      expect(mockInvoke).toHaveBeenCalledWith('create_root_node', {
        input: {
          content: 'Container',
          node_type: 'text',
          properties: { key: 'value' },
          mentioned_by: 'parent-1'
        }
      });
    });

    it('should handle createContainerNode with minimal input', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce('container-id');

      const input: CreateContainerInput = {
        content: 'Container',
        nodeType: 'text'
      };

      await adapter.createContainerNode(input);

      expect(mockInvoke).toHaveBeenCalledWith('create_root_node', {
        input: expect.objectContaining({
          properties: {},
          mentioned_by: undefined
        })
      });
    });
  });

  describe('Schema Operations', () => {
    it('should get all schemas via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockSchemas: SchemaNode[] = [
        {
          id: 'schema-1',
          nodeType: 'schema',
          content: 'Schema',
          version: 1,
          createdAt: '2025-01-01T00:00:00Z',
          modifiedAt: '2025-01-01T00:00:00Z',
          isCore: true,
          schemaVersion: 1,
          description: 'Test schema',
          fields: []
        }
      ];

      mockInvoke.mockResolvedValueOnce(mockSchemas);

      const result = await adapter.getAllSchemas();

      expect(result).toEqual(mockSchemas);
      expect(mockInvoke).toHaveBeenCalledWith('get_all_schemas');
    });

    it('should get specific schema via IPC', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      const mockSchema: SchemaNode = {
        id: 'schema-1',
        nodeType: 'schema',
        content: 'Schema',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        isCore: false,
        schemaVersion: 1,
        description: 'Test schema',
        fields: []
      };

      mockInvoke.mockResolvedValueOnce(mockSchema);

      const result = await adapter.getSchema('schema-1');

      expect(result).toEqual(mockSchema);
      expect(mockInvoke).toHaveBeenCalledWith('get_schema_definition', { schemaId: 'schema-1' });
    });
  });

  describe('Lazy Loading of Tauri API', () => {
    it('should lazy load invoke function on first use', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValueOnce(null);

      await adapter.getNode('test-id');

      expect(mockInvoke).toHaveBeenCalled();
    });

    it('should reuse cached invoke function', async () => {
      const { getBackendAdapter } = await import('$lib/services/backend-adapter');
      const adapter = getBackendAdapter();

      mockInvoke.mockResolvedValue(null);

      await adapter.getNode('test-1');
      await adapter.getNode('test-2');

      expect(mockInvoke).toHaveBeenCalledTimes(2);
    });
  });
});

describe('Backend Adapter - Environment Detection', () => {
  let originalEnv: string | undefined;

  beforeEach(() => {
    originalEnv = process.env.NODE_ENV;
    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis.window as Window & { __TAURI__?: unknown }).__TAURI__;
    delete (globalThis.window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    if (originalEnv) {
      vi.stubEnv('NODE_ENV', originalEnv);
    }
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it('should return MockAdapter in test environment', async () => {
    vi.stubEnv('NODE_ENV', 'test');

    const { getBackendAdapter } = await import('$lib/services/backend-adapter');
    const adapter = getBackendAdapter();

    const result = await adapter.getNode('any-id');
    expect(result).toBeNull();
  });

  it('should prioritize test environment over Tauri', async () => {
    vi.stubEnv('NODE_ENV', 'test');
    (globalThis.window as Window & { __TAURI__?: unknown }).__TAURI__ = {};

    const { getBackendAdapter } = await import('$lib/services/backend-adapter');
    const adapter = getBackendAdapter();

    const result = await adapter.createNode({ id: 'test', nodeType: 'text', content: 'test' });
    expect(result).toBe('mock-id');
  });

  it('should export singleton backendAdapter instance', async () => {
    const module = await import('$lib/services/backend-adapter');

    expect(module.backendAdapter).toBeDefined();
    expect(typeof module.backendAdapter.createNode).toBe('function');
    expect(typeof module.backendAdapter.getNode).toBe('function');
  });

  it('should implement complete BackendAdapter interface', async () => {
    const { backendAdapter } = await import('$lib/services/backend-adapter');

    expect(typeof backendAdapter.createNode).toBe('function');
    expect(typeof backendAdapter.getNode).toBe('function');
    expect(typeof backendAdapter.updateNode).toBe('function');
    expect(typeof backendAdapter.updateTaskNode).toBe('function');
    expect(typeof backendAdapter.deleteNode).toBe('function');
    expect(typeof backendAdapter.getChildren).toBe('function');
    expect(typeof backendAdapter.getDescendants).toBe('function');
    expect(typeof backendAdapter.getChildrenTree).toBe('function');
    expect(typeof backendAdapter.moveNode).toBe('function');
    expect(typeof backendAdapter.createMention).toBe('function');
    expect(typeof backendAdapter.deleteMention).toBe('function');
    expect(typeof backendAdapter.getOutgoingMentions).toBe('function');
    expect(typeof backendAdapter.getIncomingMentions).toBe('function');
    expect(typeof backendAdapter.getMentioningContainers).toBe('function');
    expect(typeof backendAdapter.queryNodes).toBe('function');
    expect(typeof backendAdapter.mentionAutocomplete).toBe('function');
    expect(typeof backendAdapter.createContainerNode).toBe('function');
    expect(typeof backendAdapter.getAllSchemas).toBe('function');
    expect(typeof backendAdapter.getSchema).toBe('function');
  });
});
