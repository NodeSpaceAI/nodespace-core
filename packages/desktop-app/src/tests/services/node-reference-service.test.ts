import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NodeReferenceService } from '$lib/services/node-reference-service';
import type { TauriNodeService } from '$lib/services/tauri-node-service';
import type { ReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import type { HierarchyService } from '$lib/services/hierarchy-service';
import type { Node } from '$lib/types';

describe('NodeReferenceService - @mention autocomplete filtering', () => {
  let service: NodeReferenceService;
  let mockDatabaseService: TauriNodeService;
  let mockNodeManager: ReactiveNodeService;
  let mockHierarchyService: HierarchyService;

  beforeEach(() => {
    // Create mock database service with queryNodes spy
    mockDatabaseService = {
      queryNodes: vi.fn().mockResolvedValue([])
    } as unknown as TauriNodeService;

    // Create minimal mock services (NodeReferenceService has many dependencies)
    mockNodeManager = {
      nodes: new Map()
    } as unknown as ReactiveNodeService;

    mockHierarchyService = {} as HierarchyService;

    service = new NodeReferenceService(mockNodeManager, mockHierarchyService, mockDatabaseService);
  });

  describe('searchNodes() with includeContainersAndTasks filter', () => {
    it('should include includeContainersAndTasks: true in query by default', async () => {
      // Mock response with container and task nodes
      const mockResults: Node[] = [
        {
          id: 'container-1',
          nodeType: 'text',
          content: 'Container Document about meetings',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        },
        {
          id: 'task-1',
          nodeType: 'task',
          content: 'Schedule meeting',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: { status: 'pending' }
        }
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      // Call searchNodes
      const results = await service.searchNodes('meeting');

      // Verify the database query was called with includeContainersAndTasks: true
      expect(mockDatabaseService.queryNodes).toHaveBeenCalledWith({
        contentContains: 'meeting',
        nodeType: undefined,
        includeContainersAndTasks: true,
        limit: expect.any(Number)
      });

      // Verify results are returned
      expect(results).toHaveLength(2);
      expect(results[0].id).toBe('container-1');
      expect(results[1].id).toBe('task-1');
    });

    it('should only return task and container nodes (not text children)', async () => {
      // Mock response that simulates backend filtering
      // (Backend returns only task and container nodes, no text children)
      const mockResults: Node[] = [
        {
          id: 'container-1',
          nodeType: 'text',
          content: 'Project planning',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        },
        {
          id: 'task-1',
          nodeType: 'task',
          content: 'Plan project timeline',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        }
        // Note: text child nodes are NOT returned by backend due to filter
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const results = await service.searchNodes('plan');

      // Verify only containers and tasks are in results
      expect(results).toHaveLength(2);
    });

    it('should return early for queries below minQueryLength', async () => {
      // Empty query should return early without calling database
      const results = await service.searchNodes('');

      expect(mockDatabaseService.queryNodes).not.toHaveBeenCalled();
      expect(results).toHaveLength(0);
    });

    it('should respect nodeType filter when provided', async () => {
      const mockResults: Node[] = [
        {
          id: 'task-1',
          nodeType: 'task',
          content: 'Complete report',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        }
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const results = await service.searchNodes('report', 'task');

      expect(mockDatabaseService.queryNodes).toHaveBeenCalledWith({
        contentContains: 'report',
        nodeType: 'task',
        includeContainersAndTasks: true,
        limit: expect.any(Number)
      });
      expect(results).toHaveLength(1);
      expect(results[0].nodeType).toBe('task');
    });

    it('should cache results for performance', async () => {
      const mockResults: Node[] = [
        {
          id: 'container-1',
          nodeType: 'text',
          content: 'Cached document',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        }
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      // First call - should hit database
      const results1 = await service.searchNodes('cached');
      expect(mockDatabaseService.queryNodes).toHaveBeenCalledTimes(1);

      // Second call with same query - should use cache
      const results2 = await service.searchNodes('cached');
      expect(mockDatabaseService.queryNodes).toHaveBeenCalledTimes(1); // Still 1, not 2
      expect(results2).toEqual(results1);
    });

    it('should handle database errors gracefully', async () => {
      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockRejectedValue(
        new Error('Database connection failed')
      );

      // Should not throw, but return empty array
      const results = await service.searchNodes('test');
      expect(results).toEqual([]);
    });

    it('should apply limit multiplier for better filtering results', async () => {
      const mockResults: Node[] = [];
      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      await service.searchNodes('test');

      // Check that limit is multiplied (maxSuggestions * 3 per implementation)
      const callArgs = (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mock
        .calls[0][0];
      expect(callArgs.limit).toBeGreaterThan(10); // Default maxSuggestions is likely 10
    });
  });

  describe('Edge cases and validation', () => {
    it('should handle nodes with null containerNodeId (containers)', async () => {
      const mockResults: Node[] = [
        {
          id: 'root-container',
          nodeType: 'text',
          content: 'Root document',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        }
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const results = await service.searchNodes('root');
      expect(results).toHaveLength(1);
    });

    it('should handle task nodes regardless of hierarchy', async () => {
      const mockResults: Node[] = [
        {
          id: 'task-root',
          nodeType: 'task',
          content: 'Top-level task',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        },
        {
          id: 'task-child',
          nodeType: 'task',
          content: 'Nested task',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        }
      ];

      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const results = await service.searchNodes('task');

      // Both tasks should be returned (filter allows all task nodes)
      expect(results).toHaveLength(2);
      expect(results.every((n) => n.nodeType === 'task')).toBe(true);
    });

    it('should handle special characters in search query', async () => {
      (mockDatabaseService.queryNodes as ReturnType<typeof vi.fn>).mockResolvedValue([]);

      await service.searchNodes('@special #query');

      expect(mockDatabaseService.queryNodes).toHaveBeenCalledWith({
        contentContains: '@special #query',
        nodeType: undefined,
        includeContainersAndTasks: true,
        limit: expect.any(Number)
      });
    });
  });
});
