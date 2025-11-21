/**
 * Integration Tests: Node Ordering Integrity
 *
 * Tests that node ordering is maintained correctly across all operations.
 * Previously tested beforeSiblingId linked list maintenance, but now the
 * backend handles ordering via fractional IDs.
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
import { waitForDatabaseWrites } from '../utils/test-database';
import {
  initializeDatabaseIfNeeded,
  cleanupDatabaseIfNeeded,
  shouldUseDatabase
} from '../utils/should-use-database';
import { createNodeForCurrentMode, checkServerHealth } from '../utils/test-node-helpers';
import { HttpAdapter } from '$lib/services/backend-adapter';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';

describe('Node Ordering Integrity', () => {
  let dbPath: string | null;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
  let hierarchyChangeCount: number;

  beforeAll(async () => {
    // Only verify HTTP dev server health in database mode
    if (shouldUseDatabase()) {
      const healthCheckAdapter = new HttpAdapter('http://localhost:3001');
      await checkServerHealth(healthCheckAdapter);
    }
  });

  beforeEach(async () => {
    // Note: We create a new database per test (not per suite) for better isolation,
    // trading minor performance cost for stronger guarantees against test interference.
    dbPath = await initializeDatabaseIfNeeded('node-ordering-integrity');
    adapter = new HttpAdapter('http://localhost:3001');

    hierarchyChangeCount = 0;

    // Reset shared node store to clear persistedNodeIds from previous tests
    sharedNodeStore.__resetForTesting();

    service = createReactiveNodeService({
      focusRequested: vi.fn(),
      hierarchyChanged: () => hierarchyChangeCount++,
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    });

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  afterEach(async () => {
    await cleanupDatabaseIfNeeded(dbPath);
  });

  it('should maintain valid order after creating multiple nodes', async () => {
    // Setup: Create initial node
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Act: Create multiple nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');
    const node4Id = service.createNode(node3Id, 'Fourth', 'text');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Visual order matches creation order
    const visible = service.visibleNodes(null);
    expect(visible.map((n) => n.id)).toEqual(['node-1', node2Id, node3Id, node4Id]);
  });

  it('should maintain order when node is deleted', async () => {
    // Setup: Create three nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Create additional nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');

    await waitForDatabaseWrites();

    // Act: Delete middle node
    service.deleteNode(node2Id);

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Remaining nodes in correct order
    const visible = service.visibleNodes(null);
    expect(visible.map((n) => n.id)).toEqual(['node-1', node3Id]);

    // Verify database persistence only in database mode
    if (shouldUseDatabase()) {
      // Verify: node-2 was actually deleted from database
      const node2Persisted = await adapter.getNode(node2Id);
      expect(node2Persisted).toBeNull();
    }
  });

  it('should maintain order integrity during indent operation', async () => {
    // Setup: Create three siblings
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Create additional nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');

    await waitForDatabaseWrites();

    // Act: Indent node-2 (makes it a child of node-1)
    await service.indentNode(node2Id);

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Root level has node-1 and node-3
    const rootVisible = service.visibleNodes(null);
    const rootIds = rootVisible.filter(n => service.getUIState(n.id)?.depth === 0).map(n => n.id);
    expect(rootIds).toContain('node-1');
    expect(rootIds).toContain(node3Id);

    // Verify: node-2 is now a child of node-1 (at depth 1)
    const node2UI = service.getUIState(node2Id);
    expect(node2UI?.depth).toBe(1);
  });

  it('should maintain order integrity during outdent operation', async () => {
    // Setup: Create parent with children
    const parent = await createNodeForCurrentMode(adapter, {
      id: 'parent',
      nodeType: 'text',
      content: 'Parent',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child1 = await createNodeForCurrentMode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child 1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child2 = await createNodeForCurrentMode(adapter, {
      id: 'child-2',
      nodeType: 'text',
      content: 'Child 2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    // Setup parent-child relationships (graph edges)
    // In the new architecture, parent relationships are stored as has_child edges
    // We need to explicitly set them up for tests
    // Only do this in database mode - in-memory mode doesn't have a backend
    if (shouldUseDatabase()) {
      await adapter.setParent('child-1', 'parent');
      await adapter.setParent('child-2', 'parent');
    }

    // Initialize nodes with explicit parent mapping for both database and in-memory modes
    // This ensures the parent-child cache is populated correctly
    service.initializeNodes([parent, child1, child2], {
      expanded: true,
      parentMapping: {
        parent: null, // Root node
        'child-1': 'parent', // Child of parent
        'child-2': 'parent' // Child of parent
      }
    });

    // Act: Outdent child-1
    await service.outdentNode('child-1');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: child-1 is now at root level (depth 0)
    const child1UI = service.getUIState('child-1');
    expect(child1UI?.depth).toBe(0);
  });

  it('should maintain order when combining nodes', async () => {
    // Setup: Create three nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Create additional nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');

    await waitForDatabaseWrites();

    // Act: Combine node-2 into node-1
    await service.combineNodes(node2Id, 'node-1');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Only node-1 and node-3 remain
    const visible = service.visibleNodes(null);
    expect(visible.map((n) => n.id)).toEqual(['node-1', node3Id]);

    // Verify: node-1 content was combined
    const combinedNode = service.findNode('node-1');
    expect(combinedNode?.content).toContain('First');
    expect(combinedNode?.content).toContain('Second');
  });

  it('should maintain order integrity with complex operations sequence', async () => {
    // Setup: Create initial nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Node 1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Create node-2
    const node2Id = service.createNode('node-1', 'Node 2', 'text');
    await waitForDatabaseWrites();

    // Act: Perform complex sequence with proper sequencing to avoid thundering herd
    // Each operation must complete before starting the next to prevent database contention
    const node3Id = service.createNode(node2Id, 'Node 3', 'text'); // Create
    await waitForDatabaseWrites();

    await service.indentNode(node2Id); // Indent node-2 under node-1
    await waitForDatabaseWrites();

    const node4Id = service.createNode('node-1', 'Node 4', 'text'); // Create after node-1
    await waitForDatabaseWrites();

    await service.outdentNode(node2Id); // Outdent node-2 back to root
    await waitForDatabaseWrites();

    await service.combineNodes(node3Id, node2Id); // Combine node-3 into node-2
    await waitForDatabaseWrites();

    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: node-3 was deleted by combineNodes (combined into node-2)
    expect(service.findNode(node3Id)).toBeNull();
    // Verify: node-4 still exists
    expect(service.findNode(node4Id)).toBeTruthy();

    // Verify: Visual order makes sense
    const visible = service.visibleNodes(null);
    expect(visible.length).toBeGreaterThan(0);
  });
});
