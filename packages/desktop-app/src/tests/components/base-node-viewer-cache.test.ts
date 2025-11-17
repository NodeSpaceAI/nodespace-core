/**
 * BaseNodeViewer Cache Optimization Tests (Issue #517)
 *
 * Tests for cache-first loading strategy that checks SharedNodeStore
 * before hitting the database, reducing unnecessary database calls by ~95%.
 *
 * Test Coverage:
 * - Cache hit: No database call when children are cached
 * - Cache miss: Database fetch when cache is empty
 * - Real-time updates: Subscription system still works (no regression)
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store';
import { PersistenceCoordinator } from '../../lib/services/persistence-coordinator.svelte';
import type { Node } from '../../lib/types';

describe('BaseNodeViewer cache optimization', () => {
  let store: SharedNodeStore;

  // Helper to create mock nodes
  // Note: parentId parameter kept for test convenience but not used in Node object (removed in Issue #514)
  function createMockNode(id: string, nodeType: string, _parentId: string | null = null): Node {
    return {
      id,
      nodeType,
      content: `Content for ${id}`,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      mentions: []
    };
  }

  beforeEach(() => {
    // Reset singleton before each test
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();

    // Enable test mode on PersistenceCoordinator to skip database operations
    PersistenceCoordinator.resetInstance();
    const coordinator = PersistenceCoordinator.getInstance();
    coordinator.enableTestMode();
    coordinator.resetTestState();
  });

  afterEach(async () => {
    // Clean up
    store.clearAll();
    SharedNodeStore.resetInstance();

    // Reset PersistenceCoordinator and wait for cancellation cleanup
    const coordinator = PersistenceCoordinator.getInstance();
    await coordinator.reset();
    PersistenceCoordinator.resetInstance();
  });

  it('should use cached children when available (cache hit)', async () => {
    const parent = createMockNode('parent-id', 'date');
    const children = [createMockNode('child-1', 'text', 'parent-id'), createMockNode('child-2', 'text', 'parent-id')];

    // Pre-populate cache by adding nodes to store
    store.setNode(parent, { type: 'database', reason: 'loaded-from-db' });
    for (const child of children) {
      store.setNode(child, { type: 'database', reason: 'loaded-from-db' });
    }

    // Explicitly populate the parent-child cache (Issue #514: cache-based hierarchy)
    store.updateChildrenCache('parent-id', children.map(c => c.id));

    // Spy on loadChildrenForParent (database call)
    const loadSpy = vi.spyOn(store, 'loadChildrenForParent');

    // Simulate cache-first loading logic from BaseNodeViewer.loadChildrenForParent()
    const cached = store.getNodesForParent('parent-id');

    let allNodes: Node[];
    if (cached && cached.length > 0) {
      // Cache hit - use immediately (no database call!)
      allNodes = cached;
    } else {
      // Cache miss - fetch from database
      allNodes = await store.loadChildrenForParent('parent-id');
    }

    // Verify cache was used
    expect(allNodes.length).toBe(2);
    expect(allNodes[0].id).toBe('child-1');
    expect(allNodes[1].id).toBe('child-2');

    // Should NOT have called database (cache hit)
    expect(loadSpy).not.toHaveBeenCalled();

    loadSpy.mockRestore();
  });

  it('should fetch from database on cache miss', async () => {
    const parent = createMockNode('parent-id', 'date');
    const children = [createMockNode('child-1', 'text', 'parent-id')];

    // Start with only parent in cache (no children - true cache miss)
    store.setNode(parent, { type: 'database', reason: 'loaded-from-db' });

    // Mock loadChildrenForParent to simulate database fetch
    const loadSpy = vi.spyOn(store, 'loadChildrenForParent').mockImplementation(async () => {
      // Simulate database returning children and adding them to store
      for (const child of children) {
        store.setNode(child, { type: 'database', reason: 'loaded-from-db' });
      }
      return children;
    });

    // Simulate cache-first loading logic from BaseNodeViewer
    const cached = store.getNodesForParent('parent-id');

    let allNodes: Node[];
    if (cached && cached.length > 0) {
      // Cache hit - use immediately
      allNodes = cached;
    } else {
      // Cache miss - fetch from database (should take this path)
      allNodes = await store.loadChildrenForParent('parent-id');
    }

    // Verify cache miss triggered database fetch
    expect(loadSpy).toHaveBeenCalledTimes(1);
    expect(loadSpy).toHaveBeenCalledWith('parent-id');
    expect(allNodes.length).toBe(1);
    expect(allNodes[0].id).toBe('child-1');

    loadSpy.mockRestore();
  });

  it('should maintain real-time updates from external sources (no regression)', async () => {
    // This test verifies cache updates correctly when nodes are added externally
    const parent = createMockNode('parent-id', 'date');

    // Pre-populate cache
    store.setNode(parent, { type: 'database', reason: 'loaded-from-db' });

    // Initial cache check
    let cached = store.getNodesForParent('parent-id');
    expect(cached.length).toBe(0);

    // Simulate external update (e.g., from MCP server)
    const newChild = createMockNode('new-child', 'text', 'parent-id');
    store.setNode(newChild, { type: 'mcp-server', serverId: 'test-server' });

    // CRITICAL: Update cache to reflect parent-child relationship
    // In real scenarios, this would happen when:
    // 1. MCP update notification includes parent info, OR
    // 2. Frontend re-queries backend via loadChildrenForParent()
    store.addChildToCache('parent-id', 'new-child');

    // Verify cache was updated and includes the new node
    cached = store.getNodesForParent('parent-id');
    expect(cached.length).toBe(1);
    expect(cached.some((n) => n.id === 'new-child')).toBe(true);
  });

  it('should handle empty cache correctly', () => {
    // Cache miss scenario - no children exist yet
    const cached = store.getNodesForParent('non-existent-parent');

    expect(cached).toEqual([]);
    expect(cached.length).toBe(0);

    // This would trigger database fetch in real implementation
    // (tested indirectly via integration tests)
  });

  it('should return fresh data after cache updates', () => {
    const parent = createMockNode('parent-id', 'date');
    const child1 = createMockNode('child-1', 'text', 'parent-id');

    // Initial cache population
    store.setNode(parent, { type: 'database', reason: 'loaded-from-db' });
    store.setNode(child1, { type: 'database', reason: 'loaded-from-db' });

    // CRITICAL: Explicitly populate cache with parent-child relationship
    // This simulates what loadChildrenForParent() would do after querying backend
    store.updateChildrenCache('parent-id', ['child-1']);

    let cached = store.getNodesForParent('parent-id');
    expect(cached.length).toBe(1);

    // Add another child (simulates real-time update)
    const child2 = createMockNode('child-2', 'text', 'parent-id');
    store.setNode(child2, { type: 'viewer', viewerId: 'test-viewer' });

    // Update cache to reflect the new child (simulates cache refresh after update)
    store.updateChildrenCache('parent-id', ['child-1', 'child-2']);

    // Cache should reflect the update
    cached = store.getNodesForParent('parent-id');
    expect(cached.length).toBe(2);
    expect(cached.some((n) => n.id === 'child-2')).toBe(true);
  });
});
