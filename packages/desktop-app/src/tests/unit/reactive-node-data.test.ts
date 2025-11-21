import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { nodeData } from '$lib/stores/reactive-node-data.svelte';
import type { Node } from '$lib/types';
import type { NodeEventData } from '$lib/services/event-types';

/**
 * Tests for ReactiveNodeData store
 *
 * Tests cover:
 * - Node CRUD operations
 * - Content debouncing (400ms)
 * - Property immediate updates
 * - Event subscription handling
 * - Multi-client synchronization
 * - Optimistic rollback with snapshots
 *
 * Note: These tests do NOT use Tauri event listeners since they're complex
 * to mock. Instead, we test the core logic directly through the public API.
 */

describe('ReactiveNodeData', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    // Clear any previous state
    nodeData.nodes.clear();
    // Clear any pending updates from previous tests
    const nodeDataObj = nodeData as unknown as Record<string, unknown>;
    const pendingMap = nodeDataObj.pendingContentUpdates as Map<string, { timeoutId: ReturnType<typeof setTimeout> }> | undefined;
    if (pendingMap) {
      for (const update of pendingMap.values()) {
        clearTimeout(update.timeoutId);
      }
      pendingMap.clear();
    }
  });

  afterEach(() => {
    vi.useRealTimers();
    // Cleanup
    nodeData.nodes.clear();
  });

  describe('getNode', () => {
    it('should return undefined for node that does not exist', () => {
      const node = nodeData.getNode('nonexistent');
      expect(node).toBeUndefined();
    });

    it('should return node data for existing node', () => {
      const nodeId = 'test-node-1';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Test content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {}
      };

      nodeData.nodes.set(nodeId, testNode);

      const retrieved = nodeData.getNode(nodeId);
      expect(retrieved).toEqual(testNode);
    });

    it('should be reactive - changes to node are immediately visible', () => {
      const nodeId = 'reactive-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Initial content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {}
      };

      nodeData.nodes.set(nodeId, testNode);

      // Modify the node in place (simulating reactive update)
      const node = nodeData.getNode(nodeId);
      expect(node).toBeDefined();
      if (node) {
        node.content = 'Updated content';
        nodeData.nodes.set(nodeId, node);
      }

      const updated = nodeData.getNode(nodeId);
      expect(updated?.content).toBe('Updated content');
    });
  });

  describe('updateContent', () => {
    beforeEach(() => {
      const nodeId = 'content-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Original content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);
    });

    it('should throw error when updating nonexistent node', () => {
      expect(() => {
        nodeData.updateContent('nonexistent', 'new content', 1);
      }).toThrow('[ReactiveNodeData] Cannot update content: node nonexistent not found');
    });

    it('should update local state immediately', () => {
      const nodeId = 'content-node';
      nodeData.updateContent(nodeId, 'New content', 2);

      const node = nodeData.getNode(nodeId);
      expect(node?.content).toBe('New content');
      expect(node?.version).toBe(2);
    });

    it('should schedule debounced persistence at 400ms', () => {
      const nodeId = 'content-node';
      const nodeDataPrivate = nodeData as unknown as Record<string, unknown>;
      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeDataPrivate, 'persistContent');

      nodeData.updateContent(nodeId, 'New content', 2);

      // Should not persist immediately
      expect(persistSpy).not.toHaveBeenCalled();

      // Advance by 399ms - should still not persist
      vi.advanceTimersByTime(399);
      expect(persistSpy).not.toHaveBeenCalled();

      // Advance by 1ms to reach 400ms - should persist
      vi.advanceTimersByTime(1);
      expect(persistSpy).toHaveBeenCalledWith('content-node', 'New content', 2);

      persistSpy.mockRestore();
    });

    it('should cancel pending update when new update arrives', () => {
      const nodeId = 'content-node';
      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeData as unknown as Record<string, unknown>, 'persistContent');

      // First update
      nodeData.updateContent(nodeId, 'First update', 2);

      // Advance by 300ms (within debounce window)
      vi.advanceTimersByTime(300);
      expect(persistSpy).not.toHaveBeenCalled();

      // Second update - should cancel first
      nodeData.updateContent(nodeId, 'Second update', 2);

      // Advance by 100ms - no persist yet (only 100ms into second update)
      vi.advanceTimersByTime(100);
      expect(persistSpy).not.toHaveBeenCalled();

      // Advance by 300ms more - should persist second update only
      vi.advanceTimersByTime(300);
      expect(persistSpy).toHaveBeenCalledTimes(1);
      expect(persistSpy).toHaveBeenCalledWith('content-node', 'Second update', 2);

      const node = nodeData.getNode(nodeId);
      expect(node?.content).toBe('Second update');

      persistSpy.mockRestore();
    });

    it('should track pending content updates', () => {
      const nodeId = 'content-node';

      nodeData.updateContent(nodeId, 'New content', 2);

      const pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.has(nodeId)).toBe(true);
      expect(pending.get(nodeId)?.content).toBe('New content');
      expect(pending.get(nodeId)?.version).toBe(2);
    });
  });

  describe('updateProperties', () => {
    beforeEach(() => {
      const nodeId = 'prop-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'task',
        content: 'Task content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T00:00:00Z',
        properties: { completed: false }
      };
      nodeData.nodes.set(nodeId, testNode);
    });

    it('should throw error when updating nonexistent node', () => {
      expect(() => {
        nodeData.updateProperties('nonexistent', { completed: true }, 1);
      }).toThrow('[ReactiveNodeData] Cannot update properties: node nonexistent not found');
    });

    it('should update local state immediately without debounce', () => {
      const nodeId = 'prop-node';
      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeData as unknown as Record<string, unknown>, 'persistProperties');

      nodeData.updateProperties(nodeId, { completed: true, dueDate: '2025-01-05' }, 2);

      // Verify local state updated immediately
      const node = nodeData.getNode(nodeId);
      expect(node?.properties.completed).toBe(true);
      expect(node?.properties.dueDate).toBe('2025-01-05');
      expect(node?.version).toBe(2);

      // Verify persistence was called immediately
      expect(persistSpy).toHaveBeenCalledWith('prop-node', expect.any(Object), 2);

      persistSpy.mockRestore();
    });

    it('should merge properties with existing ones', () => {
      const nodeId = 'prop-node';
      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeData as unknown as Record<string, unknown>, 'persistProperties').mockResolvedValue(
        undefined
      );

      nodeData.updateProperties(nodeId, { priority: 'high' }, 2);

      const node = nodeData.getNode(nodeId);
      expect(node?.properties.completed).toBe(false); // Original property preserved
      expect(node?.properties.priority).toBe('high'); // New property added

      persistSpy.mockRestore();
    });
  });

  describe('node CRUD operations', () => {
    it('should add node from LIVE SELECT event', () => {
      const eventData: NodeEventData = {
        id: 'new-node',
        nodeType: 'text',
        content: 'New node content',
        version: 1,
        modifiedAt: '2025-01-01T10:00:00Z'
      };

      // Call private method through the API
      // @ts-expect-error - Accessing private method for testing
      (nodeData as unknown as Record<string, unknown>).addNode?.(eventData);

      const node = nodeData.getNode('new-node');
      expect(node).toBeDefined();
      expect(node?.nodeType).toBe('text');
      expect(node?.content).toBe('New node content');
      expect(node?.version).toBe(1);
    });

    it('should update node from LIVE SELECT event', () => {
      const nodeId = 'update-node';
      const originalNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Original content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, originalNode);

      const eventData: NodeEventData = {
        id: nodeId,
        nodeType: 'text',
        content: 'Updated content',
        version: 2,
        modifiedAt: '2025-01-01T10:05:00Z'
      };

      // Call private method through the API
      // @ts-expect-error - Accessing private method for testing
      (nodeData as unknown as Record<string, unknown>).updateNodeData?.(eventData);

      const node = nodeData.getNode(nodeId);
      expect(node?.content).toBe('Updated content');
      expect(node?.version).toBe(2);
      expect(node?.modifiedAt).toBe('2025-01-01T10:05:00Z');
    });

    it('should cancel pending content update when receiving server update', () => {
      const nodeId = 'sync-test';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Original',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // User starts typing (debounced update)
      nodeData.updateContent(nodeId, 'User typing...', 1);

      // Server sends update (higher version)
      const eventData: NodeEventData = {
        id: nodeId,
        nodeType: 'text',
        content: 'Server update',
        version: 3,
        modifiedAt: '2025-01-01T10:05:00Z'
      };
      // @ts-expect-error - Accessing private method for testing
      (nodeData as unknown as Record<string, unknown>).updateNodeData?.(eventData);

      // Advance past the debounce window
      vi.advanceTimersByTime(500);

      // Should have server's content and version
      const node = nodeData.getNode(nodeId);
      expect(node?.content).toBe('Server update');
      expect(node?.version).toBe(3);

      // Pending update should be cleared
      const pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.has(nodeId)).toBe(false);
    });

    it('should remove node from store', () => {
      const nodeId = 'delete-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'To be deleted',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // Call private method through the API
      // @ts-expect-error - Accessing private method for testing
      (nodeData as unknown as Record<string, unknown>).removeNode?.(nodeId);

      expect(nodeData.getNode(nodeId)).toBeUndefined();
    });

    it('should clear pending updates when removing node', () => {
      const nodeId = 'delete-pending';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Original',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // Schedule content update
      nodeData.updateContent(nodeId, 'Pending update', 2);

      // Verify pending exists
      let pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.has(nodeId)).toBe(true);

      // Remove node
      // @ts-expect-error - Accessing private method for testing
      (nodeData as unknown as Record<string, unknown>).removeNode?.(nodeId);

      // Pending should be cleared
      pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.has(nodeId)).toBe(false);
    });
  });

  describe('multi-client synchronization', () => {
    it('should handle multiple nodes independently', () => {
      const node1: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Node 1 content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };

      const node2: Node = {
        id: 'node-2',
        nodeType: 'text',
        content: 'Node 2 content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };

      nodeData.nodes.set('node-1', { ...node1 });
      nodeData.nodes.set('node-2', { ...node2 });

      // Update content of both nodes
      nodeData.updateContent('node-1', 'Updated node 1', 2);
      nodeData.updateContent('node-2', 'Updated node 2', 2);

      // Should have independent pending updates
      const pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.size).toBe(2);
      expect(pending.get('node-1')?.content).toBe('Updated node 1');
      expect(pending.get('node-2')?.content).toBe('Updated node 2');
    });

    it('should handle concurrent content and property updates', () => {
      const nodeId = 'concurrent-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'task',
        content: 'Original content',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: { completed: false }
      };
      nodeData.nodes.set(nodeId, testNode);

      // Schedule content update (debounced)
      nodeData.updateContent(nodeId, 'Updated content', 2);

      // Immediately update properties (immediate)
      nodeData.updateProperties(nodeId, { completed: true }, 2);

      const node = nodeData.getNode(nodeId);
      expect(node?.content).toBe('Updated content'); // From debounced update
      expect(node?.properties.completed).toBe(true); // From immediate update
    });
  });

  describe('optimistic rollback', () => {
    it('should create snapshot of all nodes', () => {
      const node1: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Content 1',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };

      const node2: Node = {
        id: 'node-2',
        nodeType: 'text',
        content: 'Content 2',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };

      nodeData.nodes.set('node-1', node1);
      nodeData.nodes.set('node-2', node2);

      const snapshot = nodeData.snapshot();
      expect(snapshot.size).toBe(2);
      expect(snapshot.get('node-1')?.content).toBe('Content 1');
      expect(snapshot.get('node-2')?.content).toBe('Content 2');
    });

    it('should restore all nodes from snapshot', () => {
      const node1: Node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Content 1',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };

      nodeData.nodes.set('node-1', { ...node1 });
      const snapshot = nodeData.snapshot();

      // Modify the store
      const node = nodeData.getNode('node-1');
      if (node) {
        nodeData.updateContent('node-1', 'Pending update', 2);
      }

      // Restore from snapshot
      nodeData.restore(snapshot);

      expect(nodeData.getNode('node-1')?.content).toBe('Content 1');
      expect(nodeData.getNode('node-1')?.version).toBe(1);

      // Pending updates should be cleared
      const pending = nodeData.__testOnly_getPendingUpdates();
      expect(pending.size).toBe(0);
    });
  });

  describe('performance - debounce optimization', () => {
    it('should reduce database writes through 400ms debounce', () => {
      const nodeId = 'perf-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Initial',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeData as unknown as Record<string, unknown>, 'persistContent');

      // Simulate 10 rapid typing updates
      for (let i = 0; i < 10; i++) {
        nodeData.updateContent(nodeId, `Content ${i}`, 1 + i);
        vi.advanceTimersByTime(50); // 50ms between keystrokes
      }

      // Should still have 0 persists (all within debounce window)
      expect(persistSpy).not.toHaveBeenCalled();

      // Advance to debounce timeout
      vi.advanceTimersByTime(400);

      // Should have exactly 1 persist (last update only)
      expect(persistSpy).toHaveBeenCalledTimes(1);
      expect(persistSpy).toHaveBeenCalledWith(nodeId, 'Content 9', 10);

      persistSpy.mockRestore();
    });
  });

  describe('edge cases', () => {
    it('should handle empty properties', () => {
      const nodeId = 'empty-props';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Test',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // @ts-expect-error - Accessing private method for testing
      const persistSpy = vi.spyOn(nodeData as unknown as Record<string, unknown>, 'persistProperties').mockResolvedValue(
        undefined
      );

      nodeData.updateProperties(nodeId, {}, 2);

      expect(persistSpy).toHaveBeenCalled();

      persistSpy.mockRestore();
    });

    it('should handle very long content', () => {
      const nodeId = 'long-content';
      const longContent = 'A'.repeat(10000);
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: '',
        version: 1,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      nodeData.updateContent(nodeId, longContent, 2);

      const node = nodeData.getNode(nodeId);
      expect(node?.content.length).toBe(10000);
    });

    it('should handle version overflow gracefully', () => {
      const nodeId = 'version-node';
      const testNode: Node = {
        id: nodeId,
        nodeType: 'text',
        content: 'Test',
        version: Number.MAX_SAFE_INTEGER,
        createdAt: '2025-01-01T00:00:00Z',
        modifiedAt: '2025-01-01T10:00:00Z',
        properties: {}
      };
      nodeData.nodes.set(nodeId, testNode);

      // Should not crash when updating with max version
      expect(() => {
        nodeData.updateContent(nodeId, 'New content', Number.MAX_SAFE_INTEGER);
      }).not.toThrow();
    });
  });
});
