/**
 * Section 6: Node Ordering Tests (Phase 2 - Real Backend)
 *
 * Tests node ordering behavior with real HTTP backend and database.
 * Verifies insertAtBeginning mode, nested operations, and visual order.
 *
 * Part of comprehensive test coverage initiative (#208, Phase 2: #211)
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import {
  createTestDatabase,
  cleanupTestDatabase,
  initializeTestDatabase,
  cleanDatabase,
  waitForDatabaseWrites
} from '../utils/test-database';
import { shouldUseDatabase } from '../utils/should-use-database';
import { TestNodeBuilder } from '../utils/test-node-builder';
import { getBackendAdapter } from '$lib/services/backend-adapter';
import type { BackendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types';
import { sharedNodeStore } from '$lib/services/shared-node-store';

describe.skipIf(!shouldUseDatabase()).sequential('Section 6: Node Ordering Tests', () => {
  let dbPath: string;
  let backend: BackendAdapter;

  beforeAll(async () => {
    // Create isolated test database for this suite
    dbPath = createTestDatabase('node-ordering');
    backend = getBackendAdapter();
    await initializeTestDatabase(dbPath);
    console.log(`[Test] Using database: ${dbPath}`);
  });

  afterAll(async () => {
    // Cleanup test database
    await cleanupTestDatabase(dbPath);
  });

  beforeEach(async () => {
    // Clean database between tests to ensure test isolation
    await cleanDatabase(backend);

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  /**
   * Get children of a parent node sorted by visual order (linked list traversal)
   *
   * NodeSpace uses a beforeSiblingId linked list for node ordering:
   * - Each node points to the node that comes BEFORE it visually
   * - The first node has beforeSiblingId = null (or points outside the sibling set)
   * - Visual order: A -> B -> C means B.beforeSiblingId = A.id, C.beforeSiblingId = B.id
   *
   * Algorithm:
   * 1. Find first child (beforeSiblingId is null or not in children set)
   * 2. Traverse: find next child where child.beforeSiblingId === current.id
   * 3. Guard against circular references with visited set
   * 4. Append orphaned nodes (broken links) at end
   *
   * @param parentId - Parent node ID (null for root nodes)
   * @returns Children sorted in visual order
   *
   * @example
   * // Visual order: node1 -> node2 -> node3
   * // node1.beforeSiblingId = null
   * // node2.beforeSiblingId = node1.id
   * // node3.beforeSiblingId = node2.id
   * const ordered = await getChildrenInOrder(parentId);
   * // Returns: [node1, node2, node3]
   */
  async function getChildrenInOrder(parentId: string | null): Promise<Node[]> {
    // Use Phase 2 queryNodes() method
    const children = await backend.queryNodes({ parentId });

    // Sort by beforeSiblingId linked list
    if (children.length === 0) return [];

    // Find first child (no beforeSiblingId or beforeSiblingId not in children)
    const childIds = new Set(children.map((c) => c.id));
    const first = children.find((c) => !c.beforeSiblingId || !childIds.has(c.beforeSiblingId));

    if (!first) return children; // Fallback: return unsorted

    // Traverse linked list
    const sorted: Node[] = [];
    const visited = new Set<string>();
    let current: Node | undefined = first;

    while (current && visited.size < children.length) {
      if (visited.has(current.id)) break; // Circular ref guard
      visited.add(current.id);
      sorted.push(current);

      // Find next node (node whose beforeSiblingId points to current)
      current = children.find((c) => c.beforeSiblingId === current!.id);
    }

    // Append any orphaned nodes
    for (const child of children) {
      if (!visited.has(child.id)) {
        sorted.push(child);
      }
    }

    return sorted;
  }

  describe('insertAtBeginning=true Visual Order', () => {
    it('should create new node BEFORE reference node (visual order)', async () => {
      // Create two sibling nodes: node1, then node2 after it
      const node1Data = TestNodeBuilder.text('First node').build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node2Data = TestNodeBuilder.text('Second node')
        .withBeforeSibling(node1Id) // node2 comes after node1
        .build();
      const node2Id = await backend.createNode(node2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Fetch to verify setup
      const node1 = await backend.getNode(node1Id);
      const node2 = await backend.getNode(node2Id);
      expect(node1).toBeTruthy();
      expect(node2).toBeTruthy();
      expect(node2?.beforeSiblingId).toBe(node1Id);

      // Create new node BEFORE node2 (simulating Enter at beginning of node2)
      // In visual order: node1 -> newNode -> node2
      const newNodeData = TestNodeBuilder.text('New node before node2')
        .withParent(null) // Same parent as node2 (null = root)
        .withBeforeSibling(node1Id) // Insert between node1 and node2
        .build();
      const newNodeId = await backend.createNode(newNodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Update node2 to point to newNode instead of node1
      await backend.updateNode(node2Id, 1, {
        beforeSiblingId: newNodeId
      });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify order: node1 -> newNode -> node2
      // Root nodes have parentId = null
      const rootChildren = await getChildrenInOrder(null);
      const ids = rootChildren.map((n) => n.id);

      const node1Index = ids.indexOf(node1Id);
      const newNodeIndex = ids.indexOf(newNodeId);
      const node2Index = ids.indexOf(node2Id);

      expect(newNodeIndex).toBeGreaterThan(node1Index);
      expect(node2Index).toBeGreaterThan(newNodeIndex);
    }, 10000);

    it('should maintain correct order with multiple insertAtBeginning operations', async () => {
      // Create root node
      const rootData = TestNodeBuilder.text('Root').build();
      const rootId = await backend.createNode(rootData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Create multiple nodes before previous new node (stack order)
      // Simulates: press Enter at beginning repeatedly
      // Expected order: node3 -> node2 -> node1 -> root

      const node1Data = TestNodeBuilder.text('Node 1').withBeforeSibling(rootId).build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node2Data = TestNodeBuilder.text('Node 2').withBeforeSibling(node1Id).build();
      const node2Id = await backend.createNode(node2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node3Data = TestNodeBuilder.text('Node 3').withBeforeSibling(node2Id).build();
      const node3Id = await backend.createNode(node3Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify order by traversing from root backwards
      // Each node points to the one before it in visual order
      const node3 = await backend.getNode(node3Id);
      const node2 = await backend.getNode(node2Id);
      const node1 = await backend.getNode(node1Id);

      expect(node3?.beforeSiblingId).toBe(node2Id);
      expect(node2?.beforeSiblingId).toBe(node1Id);
      expect(node1?.beforeSiblingId).toBe(rootId);
    }, 10000);
  });

  describe('insertAtBeginning=false (Normal Splitting)', () => {
    it('should create new node AFTER reference node (normal split)', async () => {
      // Create two nodes
      const node1Data = TestNodeBuilder.text('First').build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node2Data = TestNodeBuilder.text('Second').withBeforeSibling(node1Id).build();
      const node2Id = await backend.createNode(node2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Create new node AFTER node1 (normal split in middle of content)
      // Visual order: node1 -> newNode -> node2
      const newNodeData = TestNodeBuilder.text('New content after node1')
        .withBeforeSibling(node1Id) // Same beforeSiblingId as node1's next sibling would have
        .build();
      const newNodeId = await backend.createNode(newNodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Update node2 to point to newNode
      await backend.updateNode(node2Id, 1, {
        beforeSiblingId: newNodeId
      });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: node1 -> newNode -> node2
      const newNode = await backend.getNode(newNodeId);
      const node2 = await backend.getNode(node2Id);

      expect(newNode?.beforeSiblingId).toBe(node1Id);
      expect(node2?.beforeSiblingId).toBe(newNodeId);
    }, 10000);
  });

  describe('Nested Node Ordering', () => {
    it('should maintain correct order for child nodes', async () => {
      // Create parent node
      const parentData = TestNodeBuilder.text('Parent').build();
      const parentId = await backend.createNode(parentData);

      // Create two child nodes
      const child1Data = TestNodeBuilder.text('Child 1').withParent(parentId).build();
      const child1Id = await backend.createNode(child1Data);

      const child2Data = TestNodeBuilder.text('Child 2')
        .withParent(parentId)
        .withBeforeSibling(child1Id)
        .build();
      const child2Id = await backend.createNode(child2Data);

      // Verify children order
      const children = await getChildrenInOrder(parentId);
      expect(children.length).toBe(2);
      expect(children[0].id).toBe(child1Id);
      expect(children[1].id).toBe(child2Id);
    }, 10000);

    it('should handle insertAtBeginning for child nodes', async () => {
      // Create parent
      const parentData = TestNodeBuilder.text('Parent').build();
      const parentId = await backend.createNode(parentData);

      // Create first child
      const child1Data = TestNodeBuilder.text('Child 1').withParent(parentId).build();
      const child1Id = await backend.createNode(child1Data);

      // Create new child BEFORE child1
      const newChildData = TestNodeBuilder.text('New child before child1')
        .withParent(parentId)
        .withBeforeSibling(null) // First child has no beforeSibling
        .build();
      const newChildId = await backend.createNode(newChildData);

      // Update child1 to point to newChild
      await backend.updateNode(child1Id, 1, {
        beforeSiblingId: newChildId
      });

      // Verify order: newChild -> child1
      const children = await getChildrenInOrder(parentId);
      expect(children.length).toBe(2);
      expect(children[0].id).toBe(newChildId);
      expect(children[1].id).toBe(child1Id);
    }, 10000);

    it('should maintain deep hierarchy ordering correctly', async () => {
      // Create 3-level hierarchy
      const rootData = TestNodeBuilder.text('Root').build();
      const rootId = await backend.createNode(rootData);

      const childData = TestNodeBuilder.text('Child').withParent(rootId).build();
      const childId = await backend.createNode(childData);

      const gc1Data = TestNodeBuilder.text('Grandchild 1').withParent(childId).build();
      const gc1Id = await backend.createNode(gc1Data);

      const gc2Data = TestNodeBuilder.text('Grandchild 2')
        .withParent(childId)
        .withBeforeSibling(gc1Id)
        .build();
      const gc2Id = await backend.createNode(gc2Data);

      // Verify grandchildren order under child
      const grandchildren = await getChildrenInOrder(childId);
      expect(grandchildren.length).toBe(2);
      expect(grandchildren[0].id).toBe(gc1Id);
      expect(grandchildren[1].id).toBe(gc2Id);

      // Verify child is only child of root
      const children = await getChildrenInOrder(rootId);
      expect(children.length).toBe(1);
      expect(children[0].id).toBe(childId);
    }, 10000);
  });

  describe('Mixed Operations', () => {
    it('should handle mix of insertAtBeginning and normal splits', async () => {
      // Create initial node
      const node1Data = TestNodeBuilder.text('Node 1').build();
      const node1Id = await backend.createNode(node1Data);

      // Normal split (after node1)
      const node2Data = TestNodeBuilder.text('Node 2').withBeforeSibling(node1Id).build();
      const node2Id = await backend.createNode(node2Data);

      // Insert at beginning (before node1)
      const node0Data = TestNodeBuilder.text('Node 0')
        .withBeforeSibling(null) // First in list
        .build();
      const node0Id = await backend.createNode(node0Data);

      // Update node1 to point to node0
      await backend.updateNode(node1Id, 1, {
        beforeSiblingId: node0Id
      });

      // Normal split (after node2)
      const node3Data = TestNodeBuilder.text('Node 3').withBeforeSibling(node2Id).build();
      const node3Id = await backend.createNode(node3Data);

      // Verify order: node0 -> node1 -> node2 -> node3
      const node0 = await backend.getNode(node0Id);
      const node1 = await backend.getNode(node1Id);
      const node2 = await backend.getNode(node2Id);
      const node3 = await backend.getNode(node3Id);

      expect(node0?.beforeSiblingId).toBeNull();
      expect(node1?.beforeSiblingId).toBe(node0Id);
      expect(node2?.beforeSiblingId).toBe(node1Id);
      expect(node3?.beforeSiblingId).toBe(node2Id);
    }, 10000);
  });

  describe('Header Nodes with insertAtBeginning', () => {
    it('should create new node before header (Enter at |# Header)', async () => {
      // Create header node (without using properties - backend might not support it yet)
      const headerData = TestNodeBuilder.text('# My Header').build();
      const headerId = await backend.createNode(headerData);

      // Create new node BEFORE header (simulating Enter at beginning of header)
      // Note: Backend rejects empty nodes, so use actual content
      const newNodeData = TestNodeBuilder.text('New text before header')
        .withBeforeSibling(null) // First in list
        .build();
      const newNodeId = await backend.createNode(newNodeData);

      // Update header to point to new node
      await backend.updateNode(headerId, 1, {
        beforeSiblingId: newNodeId
      });

      // Verify order: newNode -> header
      const newNode = await backend.getNode(newNodeId);
      const header = await backend.getNode(headerId);

      expect(newNode?.beforeSiblingId).toBeNull();
      expect(header?.beforeSiblingId).toBe(newNodeId);
      expect(newNode?.content).toBe('New text before header');
      expect(header?.content).toBe('# My Header');
    }, 10000);
  });
});
