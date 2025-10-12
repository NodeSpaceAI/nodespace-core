/**
 * Section 7: Database Persistence Tests (Phase 2 - Real Backend)
 *
 * Tests database persistence behavior with real HTTP backend.
 * Verifies when nodes persist, update operations, concurrency handling, and deletion.
 *
 * Part of comprehensive test coverage initiative (#208, Phase 2: #211)
 *
 * ## Test Organization
 *
 * - Section 6: Node Ordering (sibling relationships)
 * - Section 7: Database Persistence (this file - when/how nodes persist)
 * - Section 8: Event System (event emissions during operations)
 *
 * ## Why Separate Files?
 *
 * Each section tests a different architectural layer:
 * - Section 6: Linked list ordering logic
 * - Section 7: Backend persistence layer
 * - Section 8: Frontend event bus integration
 *
 * Separation enables:
 * - Independent test execution
 * - Isolated database per section
 * - Clear failure attribution
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import {
  createTestDatabase,
  cleanupTestDatabase,
  initializeTestDatabase,
  cleanDatabase,
  waitForDatabaseWrites
} from '../utils/test-database';
import { TestNodeBuilder } from '../utils/test-node-builder';
import { getBackendAdapter } from '$lib/services/backend-adapter';
import type { BackendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store';

/**
 * Helper: Create parent node with children
 *
 * Reduces duplication when creating parent-child hierarchies in tests.
 */
async function createNodeHierarchy(
  backend: BackendAdapter,
  parent: string,
  children: string[]
): Promise<{
  parentId: string;
  childIds: string[];
}> {
  const parentData = TestNodeBuilder.text(parent).build();
  const parentId = await backend.createNode(parentData);

  const childIds: string[] = [];
  let previousChildId: string | null = null;

  for (const childName of children) {
    const childData = TestNodeBuilder.text(childName)
      .withParent(parentId)
      .withBeforeSibling(previousChildId)
      .build();
    const childId = await backend.createNode(childData);
    childIds.push(childId);
    previousChildId = childId;
  }

  return { parentId, childIds };
}

describe.sequential('Section 7: Database Persistence Tests', () => {
  let dbPath: string;
  let backend: BackendAdapter;

  beforeAll(async () => {
    // Create isolated test database for this suite
    dbPath = createTestDatabase('database-persistence');
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
    const result = await cleanDatabase(backend);

    if (!result.success) {
      console.warn(
        `[Test] Database cleanup incomplete: ${result.deletedCount}/${result.totalCount} nodes deleted`
      );
      throw new Error('Database cleanup failed - test isolation compromised');
    }

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  describe('Placeholder Behavior', () => {
    it('should reject empty nodes - backend validation prevents placeholder creation', async () => {
      // Architecture Note: Empty placeholders are a frontend UX optimization.
      // Backend enforces data integrity by rejecting empty nodes.
      // This test validates the backend API contract, which is stable.
      // Frontend implementation may change (e.g., different placeholder strategy),
      // but backend validation remains constant.

      // NodeSpace backend validates that nodes must have content
      // Empty placeholder nodes exist ONLY in the frontend's memory (_nodes map)
      // They are never sent to the backend until content is added

      const emptyNodeData = TestNodeBuilder.text('').build();

      // Backend should reject empty content
      await expect(backend.createNode(emptyNodeData)).rejects.toThrow();

      // Verify node doesn't exist in database
      const fetchedNode = await backend.getNode(emptyNodeData.id);
      expect(fetchedNode).toBeNull();
    });

    it('should persist node after content added (first character triggers)', async () => {
      // Simulate: User presses Enter (creates empty placeholder), then types "H"
      // In real app: placeholder stays in memory until content added
      // In test: we directly create node with content to verify persistence

      const nodeData = TestNodeBuilder.text('H').build();
      const nodeId = await backend.createNode(nodeData);

      // Wait for database writes to complete
      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify node persisted in database
      const fetchedNode = await backend.getNode(nodeId);
      expect(fetchedNode).toBeTruthy();
      expect(fetchedNode?.content).toBe('H');
      expect(fetchedNode?.id).toBe(nodeId);
    });
  });

  describe('Update Operations', () => {
    it('should persist content updates correctly', async () => {
      // Create initial node with content
      const nodeData = TestNodeBuilder.text('Hello').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Update content
      await backend.updateNode(nodeId, { content: 'Hello World' });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify update persisted
      const fetchedNode = await backend.getNode(nodeId);
      expect(fetchedNode?.content).toBe('Hello World');
    });

    it('should persist sibling updates (beforeSiblingId changes saved)', async () => {
      // Visual Order Diagram:
      // Initial: A -> B -> C
      // After reordering: A -> C -> B

      // Create three sibling nodes: A -> B -> C
      const nodeAData = TestNodeBuilder.text('Node A').build();
      const nodeAId = await backend.createNode(nodeAData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const nodeBData = TestNodeBuilder.text('Node B').withBeforeSibling(nodeAId).build();
      const nodeBId = await backend.createNode(nodeBData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const nodeCData = TestNodeBuilder.text('Node C').withBeforeSibling(nodeBId).build();
      const nodeCId = await backend.createNode(nodeCData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Reorder: A -> C -> B (move C between A and B)
      // Step 1: Update C: beforeSiblingId = A  →  A -> C (B still points to old C.id)
      await backend.updateNode(nodeCId, { beforeSiblingId: nodeAId });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Step 2: Update B: beforeSiblingId = C  →  A -> C -> B (final order)
      await backend.updateNode(nodeBId, { beforeSiblingId: nodeCId });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify updates persisted
      const fetchedB = await backend.getNode(nodeBId);
      const fetchedC = await backend.getNode(nodeCId);

      expect(fetchedC?.beforeSiblingId).toBe(nodeAId);
      expect(fetchedB?.beforeSiblingId).toBe(nodeCId);
    });
  });

  describe('Concurrency', () => {
    it('should handle rapid creates without UNIQUE constraint violations', async () => {
      // Create 5 nodes rapidly to verify:
      // 1. UUID generator creates unique IDs (no collisions)
      // 2. Backend INSERT doesn't violate UNIQUE constraints
      // Note: 5 is sufficient because UUID collisions are deterministic bugs,
      // not probabilistic race conditions requiring stress testing.

      const nodeIds: string[] = [];
      for (let i = 0; i < 5; i++) {
        const nodeData = TestNodeBuilder.text(`Node ${i}`).build();
        const nodeId = await backend.createNode(nodeData);
        nodeIds.push(nodeId);
      }

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      expect(nodeIds).toHaveLength(5);
      expect(new Set(nodeIds).size).toBe(5); // All IDs unique

      // Verify all nodes persisted
      for (const id of nodeIds) {
        const node = await backend.getNode(id);
        expect(node).toBeTruthy();
      }
    });

    it('should handle rapid sequential updates without database errors', async () => {
      // IMPORTANT: This test validates sequential updates, not concurrent writes.
      // True concurrent writes are NOT tested because current backend architecture
      // does not support concurrent write operations (single SQLite connection).
      //
      // Concurrent write testing deferred until backend adds connection pooling.
      // Related: Issue #190 (Database locking race condition)

      // Create a node
      const nodeData = TestNodeBuilder.text('Initial').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Simulates rapid sequential updates (e.g., user typing fast)
      for (let i = 0; i < 10; i++) {
        await backend.updateNode(nodeId, { content: `Update ${i}` });
      }

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Final state should be last update
      const finalNode = await backend.getNode(nodeId);
      expect(finalNode?.content).toBe('Update 9');
    });
  });

  describe('Deletion', () => {
    it('should delete node and handle children via cascade/promotion', async () => {
      // ARCHITECTURE DECISION RECORD:
      //
      // When parent is deleted, backend MUST choose one strategy:
      //
      // Option A: CASCADE DELETE (SQLite DEFAULT)
      //   - Deletes parent and ALL descendants recursively
      //   - Simple, performant, matches most DB engines
      //   - Risk: Accidental data loss if user doesn't expect cascade
      //
      // Option B: PROMOTE (Application Logic)
      //   - Deletes parent, promotes children to root (parentId = null)
      //   - Preserves all data, matches some note-taking apps
      //   - Complex, requires additional queries
      //
      // Current Implementation: Determined by backend (see GitHub issue)
      //
      // This test validates operation completes without errors.
      // Specific assertions will be added once strategy is finalized.
      // Related: Issue #220 - Define CASCADE deletion strategy

      // Create parent with two children
      const { parentId, childIds } = await createNodeHierarchy(backend, 'Parent', [
        'Child 1',
        'Child 2'
      ]);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Delete parent
      await backend.deleteNode(parentId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify parent deleted
      const fetchedParent = await backend.getNode(parentId);
      expect(fetchedParent).toBeNull();

      // Check children status (behavior depends on backend strategy)
      const fetchedChild1 = await backend.getNode(childIds[0]);
      const fetchedChild2 = await backend.getNode(childIds[1]);

      // Determine which strategy backend implements
      if (fetchedChild1 === null && fetchedChild2 === null) {
        // CASCADE deletion strategy
        console.log('[Test] Backend uses CASCADE deletion (children deleted)');
      } else if (fetchedChild1 !== null && fetchedChild2 !== null) {
        // PROMOTE strategy
        console.log('[Test] Backend uses PROMOTE strategy (children promoted to root)');
        expect(fetchedChild1?.parentId).toBeNull();
        expect(fetchedChild2?.parentId).toBeNull();
      } else {
        // Inconsistent state - test should fail
        throw new Error('Inconsistent cascade behavior: partial child deletion detected');
      }
    }, 10000);

    it('should be idempotent when deleting non-existent node', async () => {
      // Backend DELETE is now idempotent (Issue #231).
      //
      // Current Behavior: DELETE non-existent node → HTTP 200/204 (success)
      //
      // Why Idempotence Matters:
      // - Network retries (e.g., timeout → retry) could double-delete
      // - Distributed systems (future) require idempotent operations
      // - REST/HTTP best practices mandate DELETE idempotence
      //
      // This test verifies idempotent delete behavior.

      // Create a node
      const nodeData = TestNodeBuilder.text('To Delete').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Delete once
      await backend.deleteNode(nodeId);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify deleted
      const fetchedAfterFirst = await backend.getNode(nodeId);
      expect(fetchedAfterFirst).toBeNull();

      // Delete again - backend should succeed (idempotent)
      // No error should be thrown
      await expect(backend.deleteNode(nodeId)).resolves.not.toThrow();

      // Still deleted (verify final state is consistent)
      const fetchedAfterSecond = await backend.getNode(nodeId);
      expect(fetchedAfterSecond).toBeNull();
    });
  });

  describe('Load Operations', () => {
    it('should load nodes without triggering writes (read-only operation)', async () => {
      // Create test nodes
      const node1Data = TestNodeBuilder.text('Node 1').build();
      const node1Id = await backend.createNode(node1Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      const node2Data = TestNodeBuilder.text('Node 2').withBeforeSibling(node1Id).build();
      await backend.createNode(node2Data);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Query nodes (simulates loading on app start)
      const rootNodes = await backend.queryNodes({ parentId: null });

      // Verify we got the nodes
      expect(rootNodes.length).toBe(2);

      // Query again - should return same results (no writes occurred)
      const rootNodesAgain = await backend.queryNodes({ parentId: null });
      expect(rootNodesAgain.length).toBe(2);

      // Verify node content unchanged (no write-back loop)
      const node1After = await backend.getNode(node1Id);
      expect(node1After?.content).toBe('Node 1');
      expect(node1After?.modifiedAt).toEqual(rootNodes[0].modifiedAt);
    });
  });
});
