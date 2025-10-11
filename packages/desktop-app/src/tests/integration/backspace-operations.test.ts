/**
 * Integration Tests: Backspace Operations
 *
 * Tests the combineNodes() method which simulates Backspace at beginning of node.
 * Covers 9 test cases for node combining and deletion operations.
 *
 * Each test verifies:
 * 1. In-memory state (service.nodes)
 * 2. Visual order (service.visibleNodes)
 * 3. Database persistence (via adapter.getNode())
 * 4. Event emissions (captured in beforeEach)
 * 5. No console errors
 *
 * Database Strategy: Per-test isolation (not per-suite)
 * We create a new database for each test (in beforeEach) rather than sharing one
 * database per suite. This trades minor performance cost (~50ms per test) for
 * stronger isolation guarantees against test interference.
 */

import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import {
  createTestDatabase,
  cleanupTestDatabase,
  initializeTestDatabase,
  waitForDatabaseWrites
} from '../utils/test-database';
import { createAndFetchNode, checkServerHealth } from '../utils/test-node-helpers';
import { HttpAdapter } from '$lib/services/backend-adapter';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';

describe('Backspace Operations', () => {
  let dbPath: string;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
  let deletedNodes: string[] = [];
  let hierarchyChangeCount: number;
  let focusRequests: Array<{ nodeId: string; position?: number }> = [];

  beforeAll(async () => {
    // Verify HTTP dev server is running before running any tests
    const healthCheckAdapter = new HttpAdapter('http://localhost:3001');
    await checkServerHealth(healthCheckAdapter);
  });

  beforeEach(async () => {
    // Note: We create a new database per test (not per suite) for better isolation,
    // trading minor performance cost for stronger guarantees against test interference.
    dbPath = createTestDatabase('backspace-operations');
    await initializeTestDatabase(dbPath);
    adapter = new HttpAdapter('http://localhost:3001');

    deletedNodes = [];
    hierarchyChangeCount = 0;
    focusRequests = [];

    service = createReactiveNodeService({
      focusRequested: (nodeId, position) => focusRequests.push({ nodeId, position }),
      hierarchyChanged: () => hierarchyChangeCount++,
      nodeCreated: vi.fn(),
      nodeDeleted: (nodeId) => deletedNodes.push(nodeId)
    });

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  afterEach(async () => {
    await cleanupTestDatabase(dbPath);
  });

  it('should combine two text nodes and preserve content', async () => {
    // Setup: Create two nodes
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Combine node-2 into node-1 (Backspace at beginning of node-2)
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: node-1 has combined content
    const combined = service.findNode('node-1');
    expect(combined?.content).toBe('FirstSecond');

    // Verify: node-2 deleted
    const deleted = service.findNode('node-2');
    expect(deleted).toBeNull();
    expect(deletedNodes).toContain('node-2');

    // Verify: Visual order
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1']);

    // Verify: Database state
    const dbNode1 = await adapter.getNode('node-1');
    expect(dbNode1?.content).toBe('FirstSecond');

    const dbNode2 = await adapter.getNode('node-2');
    expect(dbNode2).toBeNull();
  });

  it('should position cursor at junction point when combining', async () => {
    // Setup: Create two nodes
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First line',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second line',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Combine nodes
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Focus requested at correct position
    expect(focusRequests).toHaveLength(1);
    expect(focusRequests[0].nodeId).toBe('node-1');
    expect(focusRequests[0].position).toBe('First line'.length);
  });

  it('should strip formatting from current node when combining', async () => {
    // Setup: Create two nodes, one with header
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Plain text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: '## Header text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Combine nodes
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Header syntax stripped from combined content
    const combined = service.findNode('node-1');
    expect(combined?.content).toBe('Plain textHeader text');
    expect(combined?.content).not.toContain('##');
  });

  it('should promote children when combining nodes with children', async () => {
    // Setup: Create parent node with child, plus previous sibling
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child = await createAndFetchNode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child',
      parentId: 'node-2',
      containerNodeId: 'node-2',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, child]);

    // Act: Combine node-2 into node-1
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Child promoted to node-1's level
    const promotedChild = service.findNode('child-1');
    expect(promotedChild?.parentId).toBe(null); // Root level now

    // Verify: Visual order shows promotion
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', 'child-1']);
  });

  it('should repair sibling chain after combining', async () => {
    // Setup: Create three nodes
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createAndFetchNode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Third',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Act: Combine node-2 into node-1 (removes node-2)
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: node-3's sibling chain repaired
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1'); // Now points to node-1

    // Verify: Visual order correct
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', 'node-3']);
  });

  it('should handle combining nodes with nested children', async () => {
    // Setup: Create hierarchy with nested children
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Parent 1',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Parent 2',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child1 = await createAndFetchNode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child 1',
      parentId: 'node-2',
      containerNodeId: 'node-2',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const grandchild = await createAndFetchNode(adapter, {
      id: 'grandchild-1',
      nodeType: 'text',
      content: 'Grandchild',
      parentId: 'child-1',
      containerNodeId: 'node-2',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, child1, grandchild], { expanded: true });

    // Act: Combine node-2 into node-1
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Child hierarchy preserved
    const child = service.findNode('child-1');
    const grandchild1 = service.findNode('grandchild-1');

    expect(child).toBeDefined();
    expect(grandchild1).toBeDefined();
    expect(grandchild1?.parentId).toBe('child-1'); // Maintains parent relationship
  });

  it('should emit correct events when combining', async () => {
    // Setup: Create two nodes
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    const initialHierarchyCount = hierarchyChangeCount;

    // Act: Combine nodes
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Events emitted
    expect(deletedNodes).toContain('node-2');
    expect(hierarchyChangeCount).toBeGreaterThan(initialHierarchyCount);
    expect(focusRequests.length).toBeGreaterThan(0);
  });

  it('should handle combining when previous node is at different depth', async () => {
    // Setup: Create parent and child at different depths
    const parent = await createAndFetchNode(adapter, {
      id: 'parent',
      nodeType: 'text',
      content: 'Parent',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child = await createAndFetchNode(adapter, {
      id: 'child',
      nodeType: 'text',
      content: 'Child',
      parentId: 'parent',
      containerNodeId: 'parent',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([parent, child], { expanded: true });

    // Note: In real usage, you can't backspace from child to parent
    // (would need outdent first). But we test the service behavior.

    // Act: Attempt to combine child with parent
    service.combineNodes('child', 'parent');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Content combined
    const parentNode = service.findNode('parent');
    expect(parentNode?.content).toBe('ParentChild');

    // Verify: Child deleted
    expect(service.findNode('child')).toBeNull();
  });

  it('should strip task checkbox formatting when combining', async () => {
    // Setup: Create two nodes, one is a task
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Normal text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createAndFetchNode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: '[ ] Task item',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Combine task node into normal node
    service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Task checkbox stripped
    const combined = service.findNode('node-1');
    expect(combined?.content).toBe('Normal textTask item');
    expect(combined?.content).not.toContain('[ ]');
  });

  it('should no-op when combineNodes called with non-existent previous node (defensive check)', async () => {
    // Setup: Create single root-level node
    const firstNode = await createAndFetchNode(adapter, {
      id: 'first-node',
      nodeType: 'text',
      content: 'First Node',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([firstNode]);

    // Act: Call combineNodes with non-existent previousNodeId
    // This tests defensive programming - UI should prevent this, but service handles it gracefully
    // Real-world scenario: User presses backspace on first node (UI should block the call)
    service.combineNodes('first-node', 'non-existent-previous-node');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Operation was no-op (defensive null check succeeded)
    const unchanged = service.findNode('first-node');
    expect(unchanged?.content).toBe('First Node');
    expect(unchanged?.parentId).toBe(null);

    // Verify: Still in visual order
    const visible = service.visibleNodes;
    expect(visible).toHaveLength(1);
    expect(visible[0].id).toBe('first-node');

    // Verify: Database unchanged
    const dbNode = await adapter.getNode('first-node');
    expect(dbNode?.content).toBe('First Node');

    // Verify: No deletion event emitted (no nodes were combined)
    expect(deletedNodes).toHaveLength(0);
  });
});
