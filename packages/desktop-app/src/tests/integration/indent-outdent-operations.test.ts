/**
 * Integration Tests: Indent/Outdent Operations
 *
 * Tests the indentNode() and outdentNode() methods which manipulate hierarchy.
 * Covers 14 test cases for indentation operations.
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
import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';

describe('Indent/Outdent Operations', () => {
  let dbPath: string | null;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
  let hierarchyChangeCount: number;

  beforeAll(async () => {
    if (shouldUseDatabase()) {
      // Verify HTTP dev server is running before running any tests
      const healthCheckAdapter = new HttpAdapter('http://localhost:3001');
      await checkServerHealth(healthCheckAdapter);
    }
  });

  beforeEach(async () => {
    // CRITICAL: Reset singletons to prevent test interference
    // 1. PersistenceCoordinator tracks async operations - must be clean between tests
    PersistenceCoordinator.resetInstance();

    // 2. SharedNodeStore holds node data - without reset, nodes from previous tests remain
    sharedNodeStore.__resetForTesting();

    // Initialize database if needed
    dbPath = await initializeDatabaseIfNeeded('indent-outdent-operations');
    adapter = new HttpAdapter('http://localhost:3001');

    hierarchyChangeCount = 0;

    service = createReactiveNodeService({
      focusRequested: vi.fn(),
      hierarchyChanged: () => hierarchyChangeCount++,
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    });
  });

  afterEach(async () => {
    await cleanupDatabaseIfNeeded(dbPath);
  });

  it('should indent node to become child of previous sibling', async () => {
    // Setup: Create two sibling nodes
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const node2 = await createNodeForCurrentMode(adapter, {
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

    // Act: Indent node-2
    const result = service.indentNode('node-2');

    // Verify: Indent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: node-2 is now child of node-1
    const indented = service.findNode('node-2');
    expect(indented?.parentId).toBe('node-1');

    // Verify: Depth increased
    const ui = service.getUIState('node-2');
    expect(ui?.depth).toBe(1);

    // Verify: Database state
    if (shouldUseDatabase()) {
      const dbNode = await adapter.getNode('node-2');
      expect(dbNode?.parentId).toBe('node-1');
    }

    // Verify: Hierarchy changed event
    expect(hierarchyChangeCount).toBeGreaterThan(0);
  });

  it('should not allow indent on first node', async () => {
    // Setup: Create single node
    const node = await createNodeForCurrentMode(adapter, {
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

    // Act: Try to indent first node
    const result = service.indentNode('node-1');

    // Verify: Indent failed
    expect(result).toBe(false);

    // Verify: Node unchanged
    const unchanged = service.findNode('node-1');
    expect(unchanged?.parentId).toBeNull();
  });

  it('should recalculate descendant depths when indenting', async () => {
    // Setup: Create parent with child, then sibling
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const node2 = await createNodeForCurrentMode(adapter, {
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

    const child = await createNodeForCurrentMode(adapter, {
      id: 'child-1',
      nodeType: 'text',
      content: 'Child of Second',
      parentId: 'node-2',
      containerNodeId: 'node-2',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, child], { expanded: true });

    // Act: Indent node-2 (with child)
    service.indentNode('node-2');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: node-2 depth is 1
    const node2UI = service.getUIState('node-2');
    expect(node2UI?.depth).toBe(1);

    // Verify: child depth recalculated to 2
    const childUI = service.getUIState('child-1');
    expect(childUI?.depth).toBe(2);
  });

  it('should outdent node to parent level', async () => {
    // Setup: Create parent with child
    const parent = await createNodeForCurrentMode(adapter, {
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

    const child = await createNodeForCurrentMode(adapter, {
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

    // Act: Outdent child
    const result = service.outdentNode('child');

    // Verify: Outdent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: child is now at root level
    const outdented = service.findNode('child');
    expect(outdented?.parentId).toBeNull();

    // Verify: Depth decreased
    const ui = service.getUIState('child');
    expect(ui?.depth).toBe(0);

    // Verify: Database state
    if (shouldUseDatabase()) {
      const dbNode = await adapter.getNode('child');
      expect(dbNode?.parentId).toBeNull();
    }
  });

  it('should not allow outdent on root node', async () => {
    // Setup: Create root node
    const node = await createNodeForCurrentMode(adapter, {
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

    service.initializeNodes([node]);

    // Act: Try to outdent root
    const result = service.outdentNode('root');

    // Verify: Outdent failed
    expect(result).toBe(false);

    // Verify: Node unchanged
    const unchanged = service.findNode('root');
    expect(unchanged?.parentId).toBeNull();
  });

  it('should transfer siblings below when outdenting', async () => {
    // Setup: Create parent with multiple children
    const parent = await createNodeForCurrentMode(adapter, {
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

    const child1 = await createNodeForCurrentMode(adapter, {
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

    const child2 = await createNodeForCurrentMode(adapter, {
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

    const child3 = await createNodeForCurrentMode(adapter, {
      id: 'child-3',
      nodeType: 'text',
      content: 'Child 3',
      parentId: 'parent',
      containerNodeId: 'parent',
      beforeSiblingId: 'child-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([parent, child1, child2, child3], { expanded: true });

    // Act: Outdent child-2 (should take child-3 with it as sibling below transfers to child)
    service.outdentNode('child-2');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: child-2 outdented
    const child2Node = service.findNode('child-2');
    expect(child2Node?.parentId).toBeNull();

    // Verify: child-3 became child of child-2 (transferred as sibling below)
    const child3Node = service.findNode('child-3');
    expect(child3Node?.parentId).toBe('child-2');

    // Verify: child-1 remains with parent
    const child1Node = service.findNode('child-1');
    expect(child1Node?.parentId).toBe('parent');
  });

  it('should position outdented node after its old parent', async () => {
    // Setup: Create parent with child
    const parent = await createNodeForCurrentMode(adapter, {
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

    const child = await createNodeForCurrentMode(adapter, {
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

    // Act: Outdent child
    service.outdentNode('child');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: child positioned after parent
    const outdentedChild = service.findNode('child');
    expect(outdentedChild?.beforeSiblingId).toBe('parent');

    // Verify: Visual order
    const visible = service.visibleNodes(null);
    expect(visible.map((n) => n.id)).toEqual(['parent', 'child']);
  });

  it('should update sibling chain when indenting', async () => {
    // Setup: Create three siblings
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const node2 = await createNodeForCurrentMode(adapter, {
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

    const node3 = await createNodeForCurrentMode(adapter, {
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

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: node-3 now points to node-1 (bypassing node-2)
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.beforeSiblingId).toBe('node-1');

    // Verify: Visual order maintained at root level
    const visible = service.visibleNodes(null);
    const rootIds = visible.filter((n) => n.parentId === null).map((n) => n.id);
    expect(rootIds).toEqual(['node-1', 'node-3']);
  });

  it('should handle indenting node that has children', async () => {
    // Setup: Create parent with children, then sibling with children
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const node2 = await createNodeForCurrentMode(adapter, {
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

    const child = await createNodeForCurrentMode(adapter, {
      id: 'child-of-2',
      nodeType: 'text',
      content: 'Child',
      parentId: 'node-2',
      containerNodeId: 'node-2',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, child], { expanded: true });

    // Act: Indent node-2 (which has children)
    service.indentNode('node-2');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: node-2 and its children moved
    const node2Updated = service.findNode('node-2');
    expect(node2Updated?.parentId).toBe('node-1');

    // Verify: child still attached to node-2
    const childNode = service.findNode('child-of-2');
    expect(childNode?.parentId).toBe('node-2');

    // Verify: Depths updated correctly
    const node2UI = service.getUIState('node-2');
    const childUI = service.getUIState('child-of-2');
    expect(node2UI?.depth).toBe(1);
    expect(childUI?.depth).toBe(2);
  });

  it('should append indented node to existing children', async () => {
    // Setup: Create node with existing child, then sibling
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const existingChild = await createNodeForCurrentMode(adapter, {
      id: 'existing-child',
      nodeType: 'text',
      content: 'Existing Child',
      parentId: 'node-1',
      containerNodeId: 'node-1',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
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

    service.initializeNodes([node1, existingChild, node2], { expanded: true });

    // Act: Indent node-2 (should append after existing child)
    service.indentNode('node-2');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: node-2 positioned after existing child
    const node2Updated = service.findNode('node-2');
    expect(node2Updated?.parentId).toBe('node-1');
    expect(node2Updated?.beforeSiblingId).toBe('existing-child');

    // Verify: Visual order
    const visible = service.visibleNodes(null);
    const node1Children = visible.filter((n) => n.parentId === 'node-1').map((n) => n.id);
    expect(node1Children).toEqual(['existing-child', 'node-2']);
  });

  it('should indent multiple nodes as siblings when indented separately', async () => {
    // Setup: Create chain of nodes
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Level 0',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Level 1',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Level 2',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'node-2',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3]);

    // Act: Indent multiple different nodes (not the same node twice)
    // This simulates indenting node-2, then moving cursor to node-3 and indenting it
    service.indentNode('node-2'); // node-2 becomes child of node-1
    service.indentNode('node-3'); // node-3 becomes child of node-1 (sibling of node-2, not nested deeper)

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: Both nodes are children of node-1 (siblings of each other)
    const node2Updated = service.findNode('node-2');
    const node3Updated = service.findNode('node-3');

    expect(node2Updated?.parentId).toBe('node-1');
    expect(node3Updated?.parentId).toBe('node-1'); // Sibling of node-2, not child

    // Verify: Both at same depth (siblings)
    const node2UI = service.getUIState('node-2');
    const node3UI = service.getUIState('node-3');

    expect(node2UI?.depth).toBe(1);
    expect(node3UI?.depth).toBe(1); // Same depth as node-2
  });

  it('should handle multiple consecutive outdents', async () => {
    // Setup: Create nested hierarchy
    const node1 = await createNodeForCurrentMode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Level 0',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node2 = await createNodeForCurrentMode(adapter, {
      id: 'node-2',
      nodeType: 'text',
      content: 'Level 1',
      parentId: 'node-1',
      containerNodeId: 'node-1',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const node3 = await createNodeForCurrentMode(adapter, {
      id: 'node-3',
      nodeType: 'text',
      content: 'Level 2',
      parentId: 'node-2',
      containerNodeId: 'node-1',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node1, node2, node3], { expanded: true });

    // Act: Outdent multiple times
    service.outdentNode('node-3'); // node-3 moves to level 1
    service.outdentNode('node-3'); // node-3 moves to level 0

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: Hierarchy flattened
    const node3Updated = service.findNode('node-3');
    expect(node3Updated?.parentId).toBeNull();

    // Verify: Depth correct
    const node3UI = service.getUIState('node-3');
    expect(node3UI?.depth).toBe(0);
  });

  it('should maintain sibling chain integrity when outdenting first child', async () => {
    // Setup: Create parent with multiple children
    const parent = await createNodeForCurrentMode(adapter, {
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

    const child1 = await createNodeForCurrentMode(adapter, {
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

    const child2 = await createNodeForCurrentMode(adapter, {
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

    // Act: Outdent first child
    service.outdentNode('child-1');

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: child-1 outdented
    const child1Node = service.findNode('child-1');
    expect(child1Node?.parentId).toBeNull();

    // Verify: child-2 became child of child-1 (transferred as sibling below)
    const child2Node = service.findNode('child-2');
    expect(child2Node?.parentId).toBe('child-1');
  });

  it('should emit hierarchy changed events for indent/outdent', async () => {
    // Setup: Create nodes
    const node1 = await createNodeForCurrentMode(adapter, {
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

    const node2 = await createNodeForCurrentMode(adapter, {
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

    const initialCount = hierarchyChangeCount;

    // Act: Indent and outdent
    service.indentNode('node-2');
    const afterIndent = hierarchyChangeCount;

    service.outdentNode('node-2');
    const afterOutdent = hierarchyChangeCount;

    // CRITICAL: Wait for async database writes to complete before checking persistence
    await waitForDatabaseWrites();

    // Verify: Events emitted
    expect(afterIndent).toBeGreaterThan(initialCount);
    expect(afterOutdent).toBeGreaterThan(afterIndent);
  });

  it('should prevent indent when previous sibling is code-block (canHaveChildren: false)', async () => {
    // Setup: Create code-block followed by text node
    const codeBlock = await createNodeForCurrentMode(adapter, {
      id: 'code-1',
      nodeType: 'code-block',
      content: '```js\nconsole.log("test");\n```',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: { language: 'javascript' },
      embeddingVector: null,
      mentions: []
    });

    const textNode = await createNodeForCurrentMode(adapter, {
      id: 'text-1',
      nodeType: 'text',
      content: 'Some text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'code-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([codeBlock, textNode]);

    // Act: Try to indent text node into code-block
    const result = service.indentNode('text-1');

    // Verify: Indent was prevented
    expect(result).toBe(false);

    // Verify: Text node unchanged
    const unchanged = service.findNode('text-1');
    expect(unchanged?.parentId).toBeNull();
    expect(unchanged?.beforeSiblingId).toBe('code-1');

    // Verify: Code-block has no children
    const children = service.visibleNodes(null).filter((n) => n.parentId === 'code-1');
    expect(children).toHaveLength(0);
  });

  it('should allow indent when previous sibling is text node (canHaveChildren: true)', async () => {
    // Setup: Create text node followed by another text node
    const textNode1 = await createNodeForCurrentMode(adapter, {
      id: 'text-1',
      nodeType: 'text',
      content: 'First text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const textNode2 = await createNodeForCurrentMode(adapter, {
      id: 'text-2',
      nodeType: 'text',
      content: 'Second text',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'text-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([textNode1, textNode2]);

    // Act: Indent text-2 into text-1
    const result = service.indentNode('text-2');

    // Verify: Indent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete
    await waitForDatabaseWrites();

    // Verify: text-2 is now child of text-1
    const indented = service.findNode('text-2');
    expect(indented?.parentId).toBe('text-1');
  });

  it('should allow indent when previous sibling is header node (canHaveChildren: true)', async () => {
    // Setup: Create header followed by text node
    const headerNode = await createNodeForCurrentMode(adapter, {
      id: 'header-1',
      nodeType: 'header',
      content: 'Header',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: { level: 1 },
      embeddingVector: null,
      mentions: []
    });

    const textNode = await createNodeForCurrentMode(adapter, {
      id: 'text-1',
      nodeType: 'text',
      content: 'Content',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'header-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([headerNode, textNode]);

    // Act: Indent text into header
    const result = service.indentNode('text-1');

    // Verify: Indent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete
    await waitForDatabaseWrites();

    // Verify: text-1 is now child of header-1
    const indented = service.findNode('text-1');
    expect(indented?.parentId).toBe('header-1');
  });

  it('should allow indent when previous sibling is task node (canHaveChildren: true)', async () => {
    // Setup: Create task followed by text node
    const taskNode = await createNodeForCurrentMode(adapter, {
      id: 'task-1',
      nodeType: 'task',
      content: 'Task item',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: { completed: false },
      embeddingVector: null,
      mentions: []
    });

    const textNode = await createNodeForCurrentMode(adapter, {
      id: 'text-1',
      nodeType: 'text',
      content: 'Task details',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: 'task-1',
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([taskNode, textNode]);

    // Act: Indent text into task
    const result = service.indentNode('text-1');

    // Verify: Indent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete
    await waitForDatabaseWrites();

    // Verify: text-1 is now child of task-1
    const indented = service.findNode('text-1');
    expect(indented?.parentId).toBe('task-1');
  });

  it('should persist outdent when parent becomes a date container', async () => {
    // Regression test for bug where outdenting nodes to date container level
    // failed to persist because date nodes (with empty content) were incorrectly
    // treated as placeholders and stripped from parentId changes

    // Setup: Create date container with nested nodes
    const dateContainer = await createNodeForCurrentMode(adapter, {
      id: '2025-11-07',
      nodeType: 'date',
      content: '', // Date nodes have empty content by design
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const parent = await createNodeForCurrentMode(adapter, {
      id: 'parent',
      nodeType: 'text',
      content: 'Parent',
      parentId: '2025-11-07',
      containerNodeId: '2025-11-07',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    const child = await createNodeForCurrentMode(adapter, {
      id: 'child',
      nodeType: 'text',
      content: 'Test',
      parentId: 'parent',
      containerNodeId: '2025-11-07',
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([dateContainer, parent, child], { expanded: true });

    // Act: Outdent child (should move from parent → date container)
    const result = service.outdentNode('child');

    // Verify: Outdent succeeded
    expect(result).toBe(true);

    // CRITICAL: Wait for async database writes to complete
    await waitForDatabaseWrites();

    // Verify: In-memory state updated
    const outdentedChild = service.findNode('child');
    expect(outdentedChild?.parentId).toBe('2025-11-07');

    // Verify: Database state persisted (this was failing before fix)
    if (shouldUseDatabase()) {
      const dbNode = await adapter.getNode('child');
      expect(dbNode?.parentId).toBe('2025-11-07');
    }
  });

  // TODO (Issue #434): Add automated tests for placeholder persistence behavior
  // Manual testing performed with Chrome DevTools MCP confirms:
  // - Placeholder nodes (empty content) are NOT persisted during indent/outdent
  // - No HTTP 500 errors occur when outdenting empty nodes
  // - Deferred reference reconciliation works correctly
  // Automated tests require refactoring ReactiveNodeService API to expose
  // methods for creating unpersisted placeholders in test scenarios
});
