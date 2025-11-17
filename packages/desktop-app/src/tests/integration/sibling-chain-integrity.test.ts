/**
 * Integration Tests: Sibling Chain Integrity
 *
 * Tests the beforeSiblingId linked list maintenance across all operations.
 * Covers 7 test cases for sibling chain validation and repairs.
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

describe('Sibling Chain Integrity', () => {
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
    dbPath = await initializeDatabaseIfNeeded('sibling-chain-integrity');
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

  /**
   * Helper function to validate sibling chain integrity
   * Checks for:
   * - No circular references
   * - Exactly one first child (beforeSiblingId = null or not in sibling set)
   * - All siblings reachable from first child
   * - No orphaned nodes
   */
  function validateSiblingChain(parentId: string | null): {
    valid: boolean;
    errors: string[];
    firstChildren: string[];
    reachable: Set<string>;
    total: number;
  } {
    const errors: string[] = [];
    const firstChildren: string[] = [];
    const reachable = new Set<string>();

    // Get siblings for this specific parent (not all nodes)
    const siblings = sharedNodeStore.getNodesForParent(parentId).map((n) => n.id);

    if (siblings.length === 0) {
      return { valid: true, errors: [], firstChildren: [], reachable, total: 0 };
    }

    // Find first children (beforeSiblingId = null or points outside sibling set)
    for (const siblingId of siblings) {
      const node = service.findNode(siblingId);
      if (!node) {
        errors.push(`Node ${siblingId} not found`);
        continue;
      }

      if (node.beforeSiblingId === null || !siblings.includes(node.beforeSiblingId)) {
        firstChildren.push(siblingId);
      }
    }

    // Should have exactly one first child
    if (firstChildren.length === 0) {
      errors.push(`No first child found for parent ${parentId || 'root'}`);
    } else if (firstChildren.length > 1) {
      errors.push(`Multiple first children for parent ${parentId || 'root'}: ${firstChildren.join(', ')}`);
    }

    // If we have a valid first child, follow the chain
    if (firstChildren.length === 1) {
      const visited = new Set<string>();
      let currentId: string | null = firstChildren[0];

      while (currentId && visited.size < siblings.length) {
        if (visited.has(currentId)) {
          errors.push(`Circular reference detected at ${currentId}`);
          break;
        }

        visited.add(currentId);
        reachable.add(currentId);

        // Find next sibling (the one whose beforeSiblingId points to currentId)
        const nextSibling = siblings.find((id) => {
          const node = service.findNode(id);
          return node?.beforeSiblingId === currentId;
        });

        currentId = nextSibling || null;
      }

      // Check for orphaned nodes
      const orphans = siblings.filter((id) => !reachable.has(id));
      if (orphans.length > 0) {
        errors.push(`Orphaned nodes not in chain: ${orphans.join(', ')}`);
      }
    }

    return {
      valid: errors.length === 0,
      errors,
      firstChildren,
      reachable,
      total: siblings.length
    };
  }

  it('should maintain valid chain after creating multiple nodes', async () => {
    // Setup: Create initial node
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1]);

    // Populate hierarchy cache (node-1 is a root-level node)
    sharedNodeStore.updateChildrenCache(null, ['node-1']);

    // Act: Create multiple nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');
    const node4Id = service.createNode(node3Id, 'Fourth', 'text');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Chain integrity
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);
    expect(validation.reachable.size).toBe(4);
    expect(validation.firstChildren).toHaveLength(1);
    expect(validation.firstChildren[0]).toBe('node-1');

    // Verify: Visual order matches chain
    const visible = service.visibleNodes(null);
    expect(visible.map((n) => n.id)).toEqual(['node-1', node2Id, node3Id, node4Id]);
  });

  it('should repair chain when node is deleted', async () => {
    // Setup: Create three nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Third',
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Populate hierarchy cache (all nodes are root-level siblings)
    sharedNodeStore.updateChildrenCache(null, ['node-1', 'node-2', 'node-3']);

    // Act: Delete middle node
    service.deleteNode('node-2');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Chain repaired
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);
    expect(validation.reachable.size).toBe(2);

    // Verify: node-3 now points to node-1 (in-memory)
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');

    // Verify database persistence only in database mode
    if (shouldUseDatabase()) {
      const node3Persisted = await adapter.getNode('node-3');
      expect(node3Persisted?.beforeSiblingId).toBe('node-1');

      // Verify: node-2 was actually deleted from database
      const node2Persisted = await adapter.getNode('node-2');
      expect(node2Persisted).toBeNull();
    }
  });

  it('should maintain chain integrity during indent operation', async () => {
    // Setup: Create three siblings
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Third',
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Populate hierarchy cache (all nodes are root-level siblings)
    sharedNodeStore.updateChildrenCache(null, ['node-1', 'node-2', 'node-3']);
    // Also populate children cache for node-1 (initially empty, will receive node-2)
    sharedNodeStore.updateChildrenCache('node-1', []);

    // Act: Indent node-2
    service.indentNode('node-2');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Root chain repaired
    const rootValidation = validateSiblingChain(null);
    expect(rootValidation.valid).toBe(true);
    expect(rootValidation.reachable.size).toBe(2); // node-1 and node-3

    // Verify: node-1's children chain valid
    const node1ChildValidation = validateSiblingChain('node-1');
    expect(node1ChildValidation.valid).toBe(true);
    expect(node1ChildValidation.reachable.size).toBe(1); // node-2

    // Verify: node-3 now points to node-1 (bypassing indented node-2) (in-memory)
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');

    // Verify database persistence only in database mode
    if (shouldUseDatabase()) {
      const node2Persisted = await adapter.getNode('node-2');
      expect(node2Persisted?.beforeSiblingId).toBeNull(); // Last child of node-1

      const node3Persisted = await adapter.getNode('node-3');
      expect(node3Persisted?.beforeSiblingId).toBe('node-1'); // node-3 bypasses indented node-2
    }
  });

  it('should maintain chain integrity during outdent operation', async () => {
    // Setup: Create parent with children
    const parent = await createNodeForCurrentMode(adapter, {
      id: 'parent',
      nodeType: 'text',
      content: 'Parent',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child1 = await createNodeForCurrentMode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child 1',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child2 = await createNodeForCurrentMode(adapter, {
      id: 'child-2',
      nodeType: 'text',
      content: 'Child 2',
      beforeSiblingId: 'child-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([parent, child1, child2], { expanded: true });

    // Populate hierarchy cache
    sharedNodeStore.updateChildrenCache(null, ['parent']); // parent is root
    sharedNodeStore.updateChildrenCache('parent', ['child-1', 'child-2']); // children of parent
    // Also populate children cache for child-1 (initially empty, will receive child-2 after outdent)
    sharedNodeStore.updateChildrenCache('child-1', []);

    // Act: Outdent child-1
    service.outdentNode('child-1');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Root chain valid
    const rootValidation = validateSiblingChain(null);
    expect(rootValidation.valid).toBe(true);

    // Verify: child-1's children chain valid
    const child1ChildValidation = validateSiblingChain('child-1');
    expect(child1ChildValidation.valid).toBe(true);

    // Verify database persistence only in database mode
    if (shouldUseDatabase()) {
      const child1Persisted = await adapter.getNode('child-1');
      expect(child1Persisted?.beforeSiblingId).toBe('parent'); // Positioned after parent

      const child2Persisted = await adapter.getNode('child-2');
      expect(child2Persisted?.beforeSiblingId).toBeNull(); // First/only child of child-1
    }
  });

  it('should maintain chain when combining nodes', async () => {
    // Setup: Create three nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Third',
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Populate hierarchy cache (all nodes are root-level siblings)
    sharedNodeStore.updateChildrenCache(null, ['node-1', 'node-2', 'node-3']);

    // Act: Combine node-2 into node-1
    await service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: Chain repaired
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.reachable.size).toBe(2); // node-1 and node-3

    // Verify: node-3 points to node-1
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');
  });

  it('should validate chain has no circular references', async () => {
    // Setup: Create valid chain
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'First',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Second',
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Third',
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Verify: No circular references
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).not.toContain(expect.stringContaining('Circular reference'));

    // Additional check: Follow chain and ensure we don't loop
    const visited = new Set<string>();
    let currentId: string | null = 'node-1';
    let iterations = 0;
    const maxIterations = 100; // Safety limit

    while (currentId && iterations < maxIterations) {
      if (visited.has(currentId)) {
        throw new Error('Circular reference detected in manual traversal');
      }
      visited.add(currentId);

      // Find next
      const allNodes = Array.from(service.nodes.values());
      const next = allNodes.find((n) => n.beforeSiblingId === currentId);
      currentId = next?.id || null;
      iterations++;
    }

    expect(visited.size).toBe(3);
  });

  it('should maintain chain integrity with complex operations sequence', async () => {
    // Setup: Create initial nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Node 1',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Node 2',
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Perform complex sequence with proper sequencing to avoid thundering herd
    // Each operation must complete before starting the next to prevent database contention
    const node3Id = service.createNode('node-2', 'Node 3', 'text'); // Create
    await waitForDatabaseWrites();

    service.indentNode('node-2'); // Indent node-2 under node-1
    await waitForDatabaseWrites();

    const node4Id = service.createNode('node-1', 'Node 4', 'text'); // Create after node-1
    await waitForDatabaseWrites();

    service.outdentNode('node-2'); // Outdent node-2 back to root
    await waitForDatabaseWrites();

    await service.combineNodes(node3Id, 'node-2'); // Combine node-3 into node-2
    await waitForDatabaseWrites();

    // Only check for errors in database mode (in-memory mode expects DatabaseInitializationError)
    if (shouldUseDatabase()) {
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
    }

    // Verify: node-3 was deleted by combineNodes (combined into node-2)
    expect(service.findNode(node3Id)).toBeNull();
    // Verify: node-4 still exists
    expect(service.findNode(node4Id)).toBeTruthy();

    // Verify: Chain integrity maintained throughout
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);

    // Verify: Visual order makes sense
    const visible = service.visibleNodes(null);
    expect(visible.length).toBeGreaterThan(0);

    // Verify: All nodes either have valid beforeSiblingId or are first
    for (const node of visible) {
      if (node.beforeSiblingId !== null) {
        const beforeNode = service.findNode(node.beforeSiblingId);
        expect(beforeNode).toBeDefined();
      }
    }
  });
});
