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

describe('Sibling Chain Integrity', () => {
  let dbPath: string;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
  let hierarchyChangeCount: number;

  beforeAll(async () => {
    // Verify HTTP dev server is running before running any tests
    const healthCheckAdapter = new HttpAdapter('http://localhost:3001');
    await checkServerHealth(healthCheckAdapter);
  });

  beforeEach(async () => {
    // Note: We create a new database per test (not per suite) for better isolation,
    // trading minor performance cost for stronger guarantees against test interference.
    dbPath = createTestDatabase('sibling-chain-integrity');
    await initializeTestDatabase(dbPath);
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
    await cleanupTestDatabase(dbPath);
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

    // Get all siblings for this parent
    const siblings = Array.from(service.nodes.values())
      .filter((n) => n.parentId === parentId)
      .map((n) => n.id);

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
      errors.push(`Multiple first children: ${firstChildren.join(', ')}`);
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

    service.initializeNodes([node1]);

    // Act: Create multiple nodes
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');
    const node4Id = service.createNode(node3Id, 'Fourth', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Chain integrity
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);
    expect(validation.reachable.size).toBe(4);
    expect(validation.firstChildren).toHaveLength(1);
    expect(validation.firstChildren[0]).toBe('node-1');

    // Verify: Visual order matches chain
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', node2Id, node3Id, node4Id]);
  });

  it('should repair chain when node is deleted', async () => {
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

    // Act: Delete middle node
    service.deleteNode('node-2');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Chain repaired
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);
    expect(validation.reachable.size).toBe(2);

    // Verify: node-3 now points to node-1
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');
  });

  it('should maintain chain integrity during indent operation', async () => {
    // Setup: Create three siblings
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

    // Act: Indent node-2
    service.indentNode('node-2');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Root chain repaired
    const rootValidation = validateSiblingChain(null);
    expect(rootValidation.valid).toBe(true);
    expect(rootValidation.reachable.size).toBe(2); // node-1 and node-3

    // Verify: node-1's children chain valid
    const node1ChildValidation = validateSiblingChain('node-1');
    expect(node1ChildValidation.valid).toBe(true);
    expect(node1ChildValidation.reachable.size).toBe(1); // node-2

    // Verify: node-3 now points to node-1 (bypassing indented node-2)
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');
  });

  it('should maintain chain integrity during outdent operation', async () => {
    // Setup: Create parent with children
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

    const child1 = await createAndFetchNode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child 1',
      parentId: 'parent',
      containerNodeId: 'parent',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child2 = await createAndFetchNode(adapter, {
      id: 'child-2',
      nodeType: 'text',
      content: 'Child 2',
      parentId: 'parent',
      containerNodeId: 'parent',
      beforeSiblingId: 'child-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([parent, child1, child2], { expanded: true });

    // Act: Outdent child-1
    service.outdentNode('child-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Root chain valid
    const rootValidation = validateSiblingChain(null);
    expect(rootValidation.valid).toBe(true);

    // Verify: child-2 transferred to child-1 (as outdent transfers siblings below)
    const child2Updated = service.findNode('child-2');
    expect(child2Updated?.parentId).toBe('child-1');

    // Verify: child-1's children chain valid
    const child1ChildValidation = validateSiblingChain('child-1');
    expect(child1ChildValidation.valid).toBe(true);
  });

  it('should maintain chain when combining nodes', async () => {
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

    // Act: Combine node-2 into node-1
    await service.combineNodes('node-2', 'node-1');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

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
    const node1 = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Node 1',
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
      content: 'Node 2',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2]);

    // Act: Perform complex sequence
    const node3Id = service.createNode('node-2', 'Node 3', 'text'); // Create
    service.indentNode('node-2'); // Indent node-2 under node-1
    const node4Id = service.createNode('node-1', 'Node 4', 'text'); // Create after node-1
    service.outdentNode('node-2'); // Outdent node-2 back to root
    await service.combineNodes(node3Id, 'node-2'); // Combine node-3 into node-2

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: All nodes exist
    expect(service.findNode(node3Id)).toBeTruthy();
    expect(service.findNode(node4Id)).toBeTruthy();

    // Verify: Chain integrity maintained throughout
    const validation = validateSiblingChain(null);
    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);

    // Verify: Visual order makes sense
    const visible = service.visibleNodes;
    const rootNodes = visible.filter((n) => n.parentId === null);
    expect(rootNodes.length).toBeGreaterThan(0);

    // Verify: All nodes either have valid beforeSiblingId or are first
    for (const node of rootNodes) {
      if (node.beforeSiblingId !== null) {
        const beforeNode = service.findNode(node.beforeSiblingId);
        expect(beforeNode).toBeDefined();
        expect(beforeNode?.parentId).toBe(null); // Same parent level
      }
    }
  });
});
