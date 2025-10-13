/**
 * Integration Tests: Enter Key Operations
 *
 * Tests the createNode() method which simulates Enter key behavior.
 * Covers 10 test cases for node creation operations.
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
import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';

describe('Enter Key Operations', () => {
  let dbPath: string;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
  let createdNodes: string[] = [];
  let deletedNodes: string[] = [];
  let hierarchyChangeCount: number;

  beforeAll(async () => {
    // Disable test mode for integration tests - we want real database operations
    PersistenceCoordinator.getInstance().disableTestMode();

    // Verify HTTP dev server is running before running any tests
    const healthCheckAdapter = new HttpAdapter('http://localhost:3001');
    await checkServerHealth(healthCheckAdapter);
  });

  afterAll(() => {
    // Reset to default test mode state to prevent test ordering dependencies
    PersistenceCoordinator.getInstance().enableTestMode();
  });

  beforeEach(async () => {
    // Note: We create a new database per test (not per suite) for better isolation,
    // trading minor performance cost for stronger guarantees against test interference.
    dbPath = createTestDatabase('enter-key-operations');
    await initializeTestDatabase(dbPath);
    adapter = new HttpAdapter('http://localhost:3001');

    createdNodes = [];
    deletedNodes = [];
    hierarchyChangeCount = 0;

    // Reset shared node store to clear persistedNodeIds from previous tests
    sharedNodeStore.__resetForTesting();

    service = createReactiveNodeService({
      focusRequested: vi.fn(),
      hierarchyChanged: () => hierarchyChangeCount++,
      nodeCreated: (nodeId) => createdNodes.push(nodeId),
      nodeDeleted: (nodeId) => deletedNodes.push(nodeId)
    });

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  afterEach(async () => {
    await cleanupTestDatabase(dbPath);
  });

  it('should create new node after current (basic enter)', async () => {
    // Setup: Create first node via backend
    const firstNode = await createAndFetchNode(adapter, {
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

    // Initialize service with node
    service.initializeNodes([firstNode]);

    // Act: Simulate Enter key by calling createNode
    const newNodeId = service.createNode('node-1', '', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: In-memory state
    expect(newNodeId).toBeTruthy();
    const newNode = service.findNode(newNodeId);
    expect(newNode?.content).toBe('');
    expect(newNode?.beforeSiblingId).toBe('node-1');
    expect(newNode?.parentId).toBe(null);

    // Verify: Visual order
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', newNodeId]);

    // Verify: Database persistence
    const dbNode = await adapter.getNode(newNodeId);
    expect(dbNode).toBeDefined();
    expect(dbNode?.beforeSiblingId).toBe('node-1');

    // Verify: Events
    expect(createdNodes).toContain(newNodeId);
    expect(hierarchyChangeCount).toBeGreaterThan(0);
  });

  it('should preserve header formatting when creating node with empty content', async () => {
    // Setup: Create node with header
    const headerNode = await createAndFetchNode(adapter, {
      id: 'header-node',
      nodeType: 'text',
      content: '## Header Text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([headerNode]);

    // Act: Create new node with empty content (should inherit header)
    const newNodeId = service.createNode('header-node', '', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Header preserved
    const newNode = service.findNode(newNodeId);
    expect(newNode?.content).toMatch(/^##\s+/);

    // Verify: Database persistence
    const dbNode = await adapter.getNode(newNodeId);
    expect(dbNode?.content).toMatch(/^##\s+/);
  });

  it('should NOT persist beforeSiblingId when creating placeholder node above (FOREIGN KEY regression)', async () => {
    // Regression test for Issue #208: FOREIGN KEY constraint violation
    // When creating a node above (Enter at beginning), the new node is a placeholder
    // that won't be persisted until it gets content. We must NOT update the original
    // node's beforeSiblingId to point to the unpersisted placeholder.

    // Setup: Create root-level existing node (simpler test without parent dependencies)
    const existingNode = await createAndFetchNode(adapter, {
      id: 'existing-node',
      nodeType: 'text',
      content: '# Hello',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });
    expect(existingNode).toBeDefined();

    // Initialize service with the existing node
    service.initializeNodes([existingNode]);
    await waitForDatabaseWrites();

    // Create a placeholder node above (insertAtBeginning=true)
    // This simulates pressing Enter at the beginning of the line
    const newNodeId = service.createPlaceholderNode(
      'existing-node',
      'text',
      1, // Header level
      true, // insertAtBeginning - creates node ABOVE
      '# Hello' // Original content
    );

    // Wait for any persistence attempts
    await waitForDatabaseWrites();

    // CRITICAL ASSERTION: The new placeholder should NOT exist in database
    const dbNewNode = await adapter.getNode(newNodeId);
    expect(dbNewNode).toBeNull(); // Placeholder not persisted

    // CRITICAL ASSERTION: The existing node should NOT have beforeSiblingId updated in database
    // because that would reference a non-existent node (FOREIGN KEY violation)
    const dbExistingNode = await adapter.getNode('existing-node');
    expect(dbExistingNode).toBeDefined();
    // beforeSiblingId should still be null (or whatever it was), NOT pointing to newNodeId
    expect(dbExistingNode?.beforeSiblingId).not.toBe(newNodeId);

    // Verify: In-memory state DOES have the relationship (for UI rendering)
    const memoryExistingNode = service.nodes.get('existing-node');
    expect(memoryExistingNode?.beforeSiblingId).toBe(newNodeId); // UI state is correct

    // Verify no errors occurred during the operation
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // PART 2: Verify deferred update happens when placeholder gets content
    // This is the critical part - the sibling relationship must be established in DB
    // when the placeholder becomes a real node

    // Add content to the placeholder node
    service.updateNodeContent(newNodeId, 'New content above');
    await waitForDatabaseWrites();

    // CRITICAL ASSERTION: New node should NOW exist in database
    const dbNewNodeAfterContent = await adapter.getNode(newNodeId);
    expect(dbNewNodeAfterContent).toBeDefined();
    expect(dbNewNodeAfterContent?.content).toBe('New content above');

    // CRITICAL ASSERTION: Original node's beforeSiblingId should NOW be updated in database
    // The deferred update should have been triggered when the placeholder was persisted
    const dbExistingNodeAfterContent = await adapter.getNode('existing-node');
    expect(dbExistingNodeAfterContent).toBeDefined();
    expect(dbExistingNodeAfterContent?.beforeSiblingId).toBe(newNodeId);

    // Verify the complete sibling chain is correct
    // - New node above: beforeSiblingId = null (first in list)
    // - Existing node below: beforeSiblingId = newNodeId (points to node above)
    expect(dbNewNodeAfterContent?.beforeSiblingId).toBeNull();
    expect(dbExistingNodeAfterContent?.beforeSiblingId).toBe(newNodeId);

    // Verify no errors during the deferred update
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
  });

  it('should use insertAtBeginning flag to insert node before current', async () => {
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

    // Act: Insert at beginning of node-2
    const newNodeId = service.createNode('node-2', 'New', 'text', undefined, true);

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Node inserted before node-2
    const newNode = service.findNode(newNodeId);
    expect(newNode?.beforeSiblingId).toBe('node-1'); // Takes node-2's position

    // Verify: node-2 now points to newNode
    const updatedNode2 = service.findNode('node-2');
    expect(updatedNode2?.beforeSiblingId).toBe(newNodeId);

    // Verify: Visual order
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', newNodeId, 'node-2']);
  });

  it('should transfer children when node is expanded', async () => {
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

    // Act: Create new node after parent (should transfer children)
    const newNodeId = service.createNode('parent', '', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Children transferred to new node
    const newNode = service.findNode(newNodeId);
    expect(newNode).toBeDefined();

    const updatedChild1 = service.findNode('child-1');
    const updatedChild2 = service.findNode('child-2');

    expect(updatedChild1?.parentId).toBe(newNodeId);
    expect(updatedChild2?.parentId).toBe(newNodeId);
  });

  it('should not transfer children when insertAtBeginning is true', async () => {
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

    service.initializeNodes([parent, child1], { expanded: true });

    // Act: Insert at beginning (should NOT transfer children)
    const newNodeId = service.createNode('parent', '', 'text', undefined, true);

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: New node created
    expect(newNodeId).toBeTruthy();

    // Verify: Children remain with parent
    const child = service.findNode('child-1');
    expect(child?.parentId).toBe('parent');
  });

  it('should update sibling chain when creating between nodes', async () => {
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

    service.initializeNodes([node1, node2]);

    // Act: Create node between node1 and node2
    const newNodeId = service.createNode('node-1', 'Middle', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Sibling chain updated
    const newNode = service.findNode(newNodeId);
    expect(newNode?.beforeSiblingId).toBe('node-1');

    const node2Updated = service.findNode('node-2');
    expect(node2Updated?.beforeSiblingId).toBe(newNodeId);

    // Verify: Visual order
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', newNodeId, 'node-2']);
  });

  it('should set containerNodeId correctly for root and nested nodes', async () => {
    // Setup: Create root node
    const root = await createAndFetchNode(adapter, {
      id: 'root',
      nodeType: 'text',
      content: 'Root',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([root]);

    // Act: Create child (should get root as containerNodeId)
    const childId = service.createNode('root', 'Child', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: containerNodeId set to root
    const child = service.findNode(childId);
    expect(child?.containerNodeId).toBe('root');
    expect(child?.parentId).toBe(null); // No parent change in basic enter

    // Note: In real usage, indent operation would set parentId
    // This test verifies containerNodeId inheritance logic
  });

  it('should handle originalNodeContent parameter for header preservation', async () => {
    // Setup: Create node with header
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '### Modified Header',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Update content to remove header
    service.updateNodeContent('node-1', 'No header now');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Act: Create node with originalNodeContent to preserve original header
    const newNodeId = service.createNode(
      'node-1',
      '',
      'text',
      undefined,
      false,
      '### Modified Header' // Original content with header
    );

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Header preserved from original content
    const newNode = service.findNode(newNodeId);
    expect(newNode?.content).toMatch(/^###\s+/);
  });

  it('should handle focusNewNode parameter', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
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

    service.initializeNodes([node]);

    // Act: Create node with focusNewNode = false
    const newNodeId = service.createNode('node-1', '', 'text', undefined, false, undefined, false);

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: New node does NOT have autoFocus
    const newNodeUI = service.getUIState(newNodeId);
    expect(newNodeUI?.autoFocus).toBe(false);

    // Verify: Original node should have autoFocus
    const originalNodeUI = service.getUIState('node-1');
    expect(originalNodeUI?.autoFocus).toBe(true);
  });

  it('should create multiple nodes in sequence maintaining order', async () => {
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

    // Act: Create multiple nodes in sequence
    const node2Id = service.createNode('node-1', 'Second', 'text');
    const node3Id = service.createNode(node2Id, 'Third', 'text');
    const node4Id = service.createNode(node3Id, 'Fourth', 'text');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: All nodes created
    expect(createdNodes).toHaveLength(3);
    expect(createdNodes).toEqual([node2Id, node3Id, node4Id]);

    // Verify: Visual order maintained
    const visible = service.visibleNodes;
    expect(visible.map((n) => n.id)).toEqual(['node-1', node2Id, node3Id, node4Id]);

    // Verify: Sibling chain correct
    const node2 = service.findNode(node2Id);
    const node3 = service.findNode(node3Id);
    const node4 = service.findNode(node4Id);

    expect(node2?.beforeSiblingId).toBe('node-1');
    expect(node3?.beforeSiblingId).toBe(node2Id);
    expect(node4?.beforeSiblingId).toBe(node3Id);

    // Verify: All persisted to database
    const dbNode2 = await adapter.getNode(node2Id);
    const dbNode3 = await adapter.getNode(node3Id);
    const dbNode4 = await adapter.getNode(node4Id);

    expect(dbNode2).toBeDefined();
    expect(dbNode3).toBeDefined();
    expect(dbNode4).toBeDefined();
  });

  it('should convert containerNodeId "root" to null in database', async () => {
    // This test verifies the ROOT_CONTAINER_ID constant behavior:
    // Frontend uses "root" as a sentinel value for root-level nodes.
    // Backend must convert "root" to NULL in the database.

    // Setup: Create a node with containerNodeId="root"
    const rootNode = await createAndFetchNode(adapter, {
      id: 'root-node',
      nodeType: 'text',
      content: 'Root level node',
      parentId: null,
      containerNodeId: 'root', // Frontend uses "root" string
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    // Initialize service with the root node
    service.initializeNodes([rootNode]);
    await waitForDatabaseWrites();

    // Verify: Database stores NULL, not "root" string
    const dbNode = await adapter.getNode('root-node');
    expect(dbNode).toBeDefined();
    expect(dbNode?.containerNodeId).toBeNull(); // Backend converts to null

    // Verify: Frontend receives the node correctly
    const serviceNode = service.nodes.get('root-node');
    expect(serviceNode).toBeDefined();

    // The node should work correctly with root containerNodeId
    expect(serviceNode?.id).toBe('root-node');
    expect(serviceNode?.content).toBe('Root level node');

    // Verify: No errors occurred
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
  });
});
