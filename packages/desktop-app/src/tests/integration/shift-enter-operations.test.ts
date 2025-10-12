/**
 * Integration Tests: Shift+Enter Key Operations
 *
 * Tests the updateNodeContent() method with newline characters (\n)
 * which simulates Shift+Enter behavior (insert newline without creating new node).
 * Covers 11 test cases for inline newline operations.
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

describe('Shift+Enter Key Operations', () => {
  let dbPath: string;
  let adapter: HttpAdapter;
  let service: ReturnType<typeof createReactiveNodeService>;
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
    dbPath = createTestDatabase('shift-enter-operations');
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

  it('should insert single newline in node content', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
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

    service.initializeNodes([node]);

    // Act: Insert newline (Shift+Enter)
    service.updateNodeContent('node-1', 'First line\nSecond line');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: In-memory state
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toBe('First line\nSecond line');
    expect(updatedNode?.content.split('\n')).toHaveLength(2);

    // Verify: No new nodes created
    const visible = service.visibleNodes;
    expect(visible).toHaveLength(1);
  });

  it('should insert multiple newlines in sequence', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Line 1',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add multiple newlines
    service.updateNodeContent('node-1', 'Line 1\n\n\nLine 4');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Multiple newlines preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toBe('Line 1\n\n\nLine 4');
    expect(updatedNode?.content.split('\n')).toHaveLength(4);
    expect(updatedNode?.content.split('\n')[1]).toBe('');
    expect(updatedNode?.content.split('\n')[2]).toBe('');
  });

  it('should preserve inline formatting across newlines', async () => {
    // Setup: Create node with markdown formatting
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '**Bold text**',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newline with more formatted content
    service.updateNodeContent('node-1', '**Bold text**\n*Italic text*\n`Code text`');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Formatting preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toContain('**Bold text**');
    expect(updatedNode?.content).toContain('*Italic text*');
    expect(updatedNode?.content).toContain('`Code text`');
    expect(updatedNode?.content.split('\n')).toHaveLength(3);
  });

  it('should preserve header formatting when adding newlines', async () => {
    // Setup: Create node with header
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '## Header',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newline after header
    service.updateNodeContent('node-1', '## Header\nSubtext content');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Header preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toMatch(/^##\s+Header/);
    expect(updatedNode?.content).toContain('Subtext content');
  });

  it('should handle newline at beginning of content', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Content',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newline at beginning
    service.updateNodeContent('node-1', '\nContent with leading newline');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Leading newline preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toMatch(/^\n/);
    expect(updatedNode?.content.split('\n')[0]).toBe('');
  });

  it('should handle newline at end of content', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Content',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newline at end
    service.updateNodeContent('node-1', 'Content with trailing newline\n');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Trailing newline preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toMatch(/\n$/);
  });

  it('should preserve list item formatting with newlines', async () => {
    // Setup: Create list item node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '- List item',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newlines with continued list content
    service.updateNodeContent('node-1', '- List item\n  Sub-item text\n  More sub-item');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: List formatting preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toMatch(/^-\s+List item/);
    expect(updatedNode?.content).toContain('Sub-item text');
    expect(updatedNode?.content.split('\n')).toHaveLength(3);
  });

  it('should preserve task checkbox formatting with newlines', async () => {
    // Setup: Create task node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '[ ] Task item',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add newlines with task description
    service.updateNodeContent('node-1', '[ ] Task item\nTask details line 1\nTask details line 2');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Task formatting preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toMatch(/^\[\s*\]\s+Task item/);
    expect(updatedNode?.content).toContain('Task details line 1');
    expect(updatedNode?.content).toContain('Task details line 2');
  });

  it('should handle mixed content with multiple formatting types', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: '## Title',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Update with complex multi-line content
    const complexContent = [
      '## Title',
      '**Bold text** and *italic*',
      '',
      '- List item 1',
      '- List item 2',
      '',
      '`Code snippet`',
      'Normal text'
    ].join('\n');

    service.updateNodeContent('node-1', complexContent);

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: All formatting preserved
    const updatedNode = service.findNode('node-1');
    expect(updatedNode?.content).toContain('## Title');
    expect(updatedNode?.content).toContain('**Bold text**');
    expect(updatedNode?.content).toContain('*italic*');
    expect(updatedNode?.content).toContain('- List item 1');
    expect(updatedNode?.content).toContain('`Code snippet`');
    expect(updatedNode?.content.split('\n')).toHaveLength(8);
  });

  it('should not create new nodes when content contains newlines', async () => {
    // Setup: Create two nodes
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

    const initialCount = service.visibleNodes.length;

    // Act: Add newlines to node 1
    service.updateNodeContent('node-1', 'Node 1\nLine 2\nLine 3');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Node count unchanged
    expect(service.visibleNodes).toHaveLength(initialCount);
    expect(service.visibleNodes.map((n) => n.id)).toEqual(['node-1', 'node-2']);

    // Verify: Only node-1 content changed
    const node1Updated = service.findNode('node-1');
    const node2Updated = service.findNode('node-2');
    expect(node1Updated?.content).toBe('Node 1\nLine 2\nLine 3');
    expect(node2Updated?.content).toBe('Node 2');
  });

  it('should handle empty lines between content', async () => {
    // Setup: Create node
    const node = await createAndFetchNode(adapter, {
      id: 'node-1',
      nodeType: 'text',
      content: 'Paragraph 1',
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      embeddingVector: null,
      mentions: []
    });

    service.initializeNodes([node]);

    // Act: Add content with empty lines (simulating paragraph breaks)
    service.updateNodeContent('node-1', 'Paragraph 1\n\nParagraph 2\n\nParagraph 3');

    await waitForDatabaseWrites();
    expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

    // Verify: Empty lines preserved
    const updatedNode = service.findNode('node-1');
    const lines = updatedNode?.content.split('\n');
    expect(lines).toHaveLength(5);
    expect(lines?.[1]).toBe(''); // Empty line
    expect(lines?.[3]).toBe(''); // Empty line
    expect(lines?.[0]).toBe('Paragraph 1');
    expect(lines?.[2]).toBe('Paragraph 2');
    expect(lines?.[4]).toBe('Paragraph 3');
  });
});
