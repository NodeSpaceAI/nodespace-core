/**
 * OptimisticOperationManager Tests
 *
 * Tests the fire-and-forget with rollback pattern for structural operations.
 *
 * Test Coverage:
 * - Successful operations (optimistic update + backend success)
 * - Backend failures (rollback + error event emission)
 * - Optimistic update failures (immediate rollback)
 * - Batch operations (multiple operations with shared snapshot)
 * - Snapshot/restore for structure and data
 * - Error event emission and categorization
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { optimisticOperationManager } from '$lib/services/optimistic-operation-manager.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { nodeData } from '$lib/stores/reactive-node-data.svelte';
import * as tauriEvent from '@tauri-apps/api/event';

// Mock Tauri emit
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn()
}));

describe('OptimisticOperationManager', () => {
  beforeEach(() => {
    // Clear all mocks before each test
    vi.clearAllMocks();

    // Reset stores to clean state
    structureTree.children = new Map();
    nodeData.nodes = new Map();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('executeStructuralChange', () => {
    it('should apply optimistic update immediately', async () => {
      // Arrange
      let optimisticUpdateCalled = false;
      const optimisticUpdate = () => {
        optimisticUpdateCalled = true;
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi.fn().mockResolvedValue(undefined);

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation',
          affectedNodes: ['child', 'parent']
        }
      );

      // Assert
      expect(optimisticUpdateCalled).toBe(true);
      expect(structureTree.getChildren('parent')).toEqual(['child']);
    });

    it('should fire backend operation without waiting', async () => {
      // Arrange
      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      let backendOperationStarted = false;
      let backendOperationCompleted = false;

      const backendOperation = vi.fn().mockImplementation(async () => {
        backendOperationStarted = true;
        await new Promise((resolve) => setTimeout(resolve, 100));
        backendOperationCompleted = true;
      });

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation'
        }
      );

      // Assert - Backend should be called but not awaited
      expect(backendOperation).toHaveBeenCalled();
      expect(backendOperationStarted).toBe(true);
      expect(backendOperationCompleted).toBe(false); // Not awaited!
    });

    it('should rollback on backend failure', async () => {
      // Arrange
      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Backend failed'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation',
          affectedNodes: ['child', 'parent']
        }
      );

      // Wait for backend operation to fail and rollback to complete
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - Rollback should have restored empty state
      expect(structureTree.getChildren('parent')).toEqual([]);
    });

    it('should emit error event on backend failure', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Database locked'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'indent node',
          affectedNodes: ['child', 'parent']
        }
      );

      // Wait for error event emission
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          type: 'error:persistence-failed',
          namespace: 'error',
          message: 'Failed to indent node',
          failedNodeIds: ['child', 'parent'],
          failureReason: 'database-locked',
          canRetry: true,
          affectedOperations: [
            {
              nodeId: 'child',
              operation: 'update',
              error: 'Database locked'
            },
            {
              nodeId: 'parent',
              operation: 'update',
              error: 'Database locked'
            }
          ]
        })
      );
    });

    it('should rollback immediately if optimistic update fails', async () => {
      // Arrange
      const optimisticUpdate = () => {
        throw new Error('Optimistic update failed');
      };

      const backendOperation = vi.fn().mockResolvedValue(undefined);

      // Act & Assert
      await expect(
        optimisticOperationManager.executeStructuralChange(
          optimisticUpdate,
          backendOperation,
          {
            description: 'test operation'
          }
        )
      ).rejects.toThrow('Optimistic update failed');

      // Backend should not be called if optimistic update fails
      expect(backendOperation).not.toHaveBeenCalled();
    });

    it('should snapshot and restore data when snapshotData option is true', async () => {
      // Arrange
      const testNode = {
        id: 'node-1',
        nodeType: 'text',
        content: 'original content',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
      };

      nodeData.nodes.set('node-1', testNode);

      const optimisticUpdate = () => {
        const node = nodeData.nodes.get('node-1');
        if (node) {
          node.content = 'modified content';
          nodeData.nodes.set('node-1', node);
        }
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Backend failed'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation with data',
          snapshotData: true
        }
      );

      // Wait for rollback
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - Content should be rolled back to original
      const rolledBackNode = nodeData.nodes.get('node-1');
      expect(rolledBackNode?.content).toBe('original content');
    });

    it('should not snapshot data by default', async () => {
      // Arrange
      const testNode = {
        id: 'node-1',
        nodeType: 'text',
        content: 'original content',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
      };

      nodeData.nodes.set('node-1', testNode);

      const optimisticUpdate = () => {
        const node = nodeData.nodes.get('node-1');
        if (node) {
          node.content = 'modified content';
          nodeData.nodes.set('node-1', node);
        }

        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Backend failed'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation without data snapshot',
          snapshotData: false // explicit false
        }
      );

      // Wait for rollback
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - Structure should be rolled back, but data should remain modified
      expect(structureTree.getChildren('parent')).toEqual([]);
      const nodeAfterRollback = nodeData.nodes.get('node-1');
      expect(nodeAfterRollback?.content).toBe('modified content'); // Data not rolled back!
    });
  });

  describe('executeBatch', () => {
    it('should apply all optimistic updates before firing backend operations', async () => {
      // Arrange
      let update1Called = false;
      let update2Called = false;

      const operations = [
        {
          optimisticUpdate: () => {
            update1Called = true;
            structureTree.__testOnly_addChild({
              id: 'edge-1',
              in: 'parent',
              out: 'child1',
              edgeType: 'child',
              order: 1.0
            });
          },
          backendOperation: vi.fn().mockResolvedValue(undefined),
          description: 'move child1',
          affectedNodes: ['child1']
        },
        {
          optimisticUpdate: () => {
            update2Called = true;
            structureTree.__testOnly_addChild({
              id: 'edge-2',
              in: 'parent',
              out: 'child2',
              edgeType: 'child',
              order: 2.0
            });
          },
          backendOperation: vi.fn().mockResolvedValue(undefined),
          description: 'move child2',
          affectedNodes: ['child2']
        }
      ];

      // Act
      await optimisticOperationManager.executeBatch(operations);

      // Assert
      expect(update1Called).toBe(true);
      expect(update2Called).toBe(true);
      expect(structureTree.getChildren('parent')).toEqual(['child1', 'child2']);
    });

    it('should rollback all operations if any backend operation fails', async () => {
      // Arrange
      const operations = [
        {
          optimisticUpdate: () => {
            structureTree.__testOnly_addChild({
              id: 'edge-1',
              in: 'parent',
              out: 'child1',
              edgeType: 'child',
              order: 1.0
            });
          },
          backendOperation: vi.fn().mockResolvedValue(undefined),
          description: 'move child1',
          affectedNodes: ['child1']
        },
        {
          optimisticUpdate: () => {
            structureTree.__testOnly_addChild({
              id: 'edge-2',
              in: 'parent',
              out: 'child2',
              edgeType: 'child',
              order: 2.0
            });
          },
          backendOperation: vi
            .fn()
            .mockRejectedValue(new Error('Backend failed')),
          description: 'move child2',
          affectedNodes: ['child2']
        }
      ];

      // Act
      await optimisticOperationManager.executeBatch(operations);

      // Wait for rollback
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - All operations should be rolled back
      expect(structureTree.getChildren('parent')).toEqual([]);
    });

    it('should emit single error event for batch failure', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const operations = [
        {
          optimisticUpdate: () => {
            structureTree.__testOnly_addChild({
              id: 'edge-1',
              in: 'parent',
              out: 'child1',
              edgeType: 'child',
              order: 1.0
            });
          },
          backendOperation: vi.fn().mockResolvedValue(undefined),
          description: 'move child1',
          affectedNodes: ['child1']
        },
        {
          optimisticUpdate: () => {
            structureTree.__testOnly_addChild({
              id: 'edge-2',
              in: 'parent',
              out: 'child2',
              edgeType: 'child',
              order: 2.0
            });
          },
          backendOperation: vi
            .fn()
            .mockRejectedValue(new Error('Backend failed')),
          description: 'move child2',
          affectedNodes: ['child2']
        }
      ];

      // Act
      await optimisticOperationManager.executeBatch(operations);

      // Wait for error event
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledTimes(1);
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          message: 'Failed to move child1, move child2',
          failedNodeIds: ['child1', 'child2']
        })
      );
    });
  });

  describe('Error Categorization', () => {
    it('should categorize timeout errors', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Operation timed out'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation'
        }
      );

      // Wait for error event
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          failureReason: 'timeout'
        })
      );
    });

    it('should categorize foreign key constraint errors', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(
          new Error('FOREIGN KEY constraint failed')
        );

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation'
        }
      );

      // Wait for error event
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          failureReason: 'foreign-key-constraint'
        })
      );
    });

    it('should categorize database locked errors', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Database is locked'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation'
        }
      );

      // Wait for error event
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          failureReason: 'database-locked'
        })
      );
    });

    it('should categorize unknown errors', async () => {
      // Arrange
      const mockEmit = vi.mocked(tauriEvent.emit);

      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Something weird happened'));

      // Act
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation'
        }
      );

      // Wait for error event
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockEmit).toHaveBeenCalledWith(
        'error:persistence-failed',
        expect.objectContaining({
          failureReason: 'unknown'
        })
      );
    });
  });

  describe('Integration with Reactive Stores', () => {
    it('should work with ReactiveStructureTree snapshot/restore', async () => {
      // Arrange - Set up initial structure
      structureTree.__testOnly_addChild({
        id: 'edge-1',
        in: 'root',
        out: 'child1',
        edgeType: 'child',
        order: 1.0
      });

      structureTree.__testOnly_addChild({
        id: 'edge-2',
        in: 'root',
        out: 'child2',
        edgeType: 'child',
        order: 2.0
      });

      expect(structureTree.getChildren('root')).toEqual(['child1', 'child2']);

      // Act - Operation that will fail
      const optimisticUpdate = () => {
        structureTree.__testOnly_addChild({
          id: 'edge-3',
          in: 'root',
          out: 'child3',
          edgeType: 'child',
          order: 3.0
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Backend failed'));

      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'add child3'
        }
      );

      // Wait for rollback
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - Should be rolled back to original state
      expect(structureTree.getChildren('root')).toEqual(['child1', 'child2']);
    });

    it('should preserve structure changes when snapshotData is false', async () => {
      // Arrange
      const optimisticUpdate = () => {
        // Both structure and data changes
        structureTree.__testOnly_addChild({
          id: 'edge-1',
          in: 'parent',
          out: 'child',
          edgeType: 'child',
          order: 1.0
        });

        nodeData.nodes.set('node-1', {
          id: 'node-1',
          nodeType: 'text',
          content: 'new content',
          version: 1,
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {}
        });
      };

      const backendOperation = vi
        .fn()
        .mockRejectedValue(new Error('Backend failed'));

      // Act - snapshotData: false (default)
      await optimisticOperationManager.executeStructuralChange(
        optimisticUpdate,
        backendOperation,
        {
          description: 'test operation',
          snapshotData: false
        }
      );

      // Wait for rollback
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(structureTree.getChildren('parent')).toEqual([]); // Structure rolled back
      expect(nodeData.nodes.get('node-1')?.content).toBe('new content'); // Data preserved
    });
  });
});
