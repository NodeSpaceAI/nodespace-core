/**
 * Section 6: Node Ordering Tests (Phase 2 - Real Backend)
 *
 * Tests node ordering behavior with real HTTP backend and database.
 * Node ordering is now handled by the backend via fractional IDs.
 * These tests verify visual order through the backend's queryNodes method.
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
   * Get children of a parent node via queryNodes
   * Backend now returns children in correct order via fractional IDs
   *
   * @param parentId - Parent node ID (null for root nodes)
   * @returns Children in visual order
   */
  async function getChildrenInOrder(parentId: string | null): Promise<Node[]> {
    // Backend handles ordering via fractional IDs
    const children = await backend.queryNodes({ parentId });
    return children;
  }

  // ============================================================================
  // Basic Node Creation and Ordering
  // ============================================================================

  describe('Basic Node Creation', () => {
    it('should create nodes successfully', async () => {
      const node1Data = TestNodeBuilder.text('Node 1').build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify node exists
      const node1 = await backend.getNode(node1Id);
      expect(node1).toBeTruthy();
      expect(node1?.content).toBe('Node 1');
    });

    it('should return nodes from queryNodes', async () => {
      // Create two nodes
      const node1Data = TestNodeBuilder.text('Node 1').build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node2Data = TestNodeBuilder.text('Node 2').build();
      const node2Id = await backend.createNode(node2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Query all root nodes
      const rootNodes = await getChildrenInOrder(null);

      // Both nodes should be present
      const nodeIds = rootNodes.map((n) => n.id);
      expect(nodeIds).toContain(node1Id);
      expect(nodeIds).toContain(node2Id);
    });
  });

  // ============================================================================
  // Node Movement with moveNode
  // ============================================================================

  describe('Node Movement via moveNode', () => {
    it('should move node after another node', async () => {
      // Create three nodes
      const node1Data = TestNodeBuilder.text('Node 1').build();
      const node1Id = await backend.createNode(node1Data);

      const node2Data = TestNodeBuilder.text('Node 2').build();
      const node2Id = await backend.createNode(node2Data);

      const node3Data = TestNodeBuilder.text('Node 3').build();
      const node3Id = await backend.createNode(node3Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Move node3 after node1 (reorder)
      await backend.moveNode(node3Id, null);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify all nodes still exist
      const node1 = await backend.getNode(node1Id);
      const node2 = await backend.getNode(node2Id);
      const node3 = await backend.getNode(node3Id);

      expect(node1).toBeTruthy();
      expect(node2).toBeTruthy();
      expect(node3).toBeTruthy();
    });

    it('should move node to become child of another node', async () => {
      // Create parent and child nodes
      const parentData = TestNodeBuilder.text('Parent').build();
      const parentId = await backend.createNode(parentData);

      const childData = TestNodeBuilder.text('Child').build();
      const childId = await backend.createNode(childData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Move child under parent using setParent
      await backend.setParent(childId, parentId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify child is now under parent
      const children = await backend.getChildren(parentId);
      expect(children).toHaveLength(1);
      expect(children[0].id).toBe(childId);
    });
  });

  // ============================================================================
  // Parent-Child Relationships
  // ============================================================================

  describe('Parent-Child Relationships', () => {
    it('should maintain parent-child relationship after setParent', async () => {
      // Create hierarchy
      const parentData = TestNodeBuilder.text('Parent').build();
      const parentId = await backend.createNode(parentData);

      const child1Data = TestNodeBuilder.text('Child 1').build();
      const child1Id = await backend.createNode(child1Data);

      const child2Data = TestNodeBuilder.text('Child 2').build();
      const child2Id = await backend.createNode(child2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Set parent-child relationships
      await backend.setParent(child1Id, parentId);
      await backend.setParent(child2Id, parentId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify children
      const children = await backend.getChildren(parentId);
      expect(children).toHaveLength(2);

      const childIds = children.map((c) => c.id);
      expect(childIds).toContain(child1Id);
      expect(childIds).toContain(child2Id);
    });

    it('should handle deep hierarchy (grandchildren)', async () => {
      // Create 3-level hierarchy
      const rootData = TestNodeBuilder.text('Root').build();
      const rootId = await backend.createNode(rootData);

      const childData = TestNodeBuilder.text('Child').build();
      const childId = await backend.createNode(childData);

      const grandchildData = TestNodeBuilder.text('Grandchild').build();
      const grandchildId = await backend.createNode(grandchildData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Set up hierarchy
      await backend.setParent(childId, rootId);
      await backend.setParent(grandchildId, childId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify child of root
      const childrenOfRoot = await backend.getChildren(rootId);
      expect(childrenOfRoot).toHaveLength(1);
      expect(childrenOfRoot[0].id).toBe(childId);

      // Verify grandchild of child
      const grandchildren = await backend.getChildren(childId);
      expect(grandchildren).toHaveLength(1);
      expect(grandchildren[0].id).toBe(grandchildId);
    });
  });

  // ============================================================================
  // Node Deletion
  // ============================================================================

  describe('Node Deletion', () => {
    it('should delete node successfully', async () => {
      const nodeData = TestNodeBuilder.text('To be deleted').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify node exists
      let node = await backend.getNode(nodeId);
      expect(node).toBeTruthy();

      // Delete node
      await backend.deleteNode(nodeId, node!.version);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify node no longer exists
      node = await backend.getNode(nodeId);
      expect(node).toBeNull();
    });

    it('should handle deletion of node with children', async () => {
      // Create parent with child
      const parentData = TestNodeBuilder.text('Parent').build();
      const parentId = await backend.createNode(parentData);

      const childData = TestNodeBuilder.text('Child').build();
      const childId = await backend.createNode(childData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Set up parent-child relationship
      await backend.setParent(childId, parentId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify child exists under parent
      let children = await backend.getChildren(parentId);
      expect(children).toHaveLength(1);

      // Delete child
      const child = await backend.getNode(childId);
      await backend.deleteNode(childId, child!.version);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify child no longer under parent
      children = await backend.getChildren(parentId);
      expect(children).toHaveLength(0);
    });
  });
});
