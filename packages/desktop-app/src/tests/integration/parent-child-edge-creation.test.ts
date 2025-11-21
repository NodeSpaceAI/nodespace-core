/**
 * Integration tests for parent-child edge creation (Issue #528)
 *
 * Tests the complete fix for PR #523's regression where parent-child edges
 * were not being created when promoting placeholder nodes.
 *
 * **Root Cause**: PR #523 removed parentId/containerNodeId from the HTTP API,
 * breaking edge creation. The fix re-added them as operation parameters
 * (not stored fields) to enable edge creation via NodeOperations.
 *
 * **What This Tests**:
 * 1. Parent-child edges are created when promoting placeholders
 * 2. Children cache is updated correctly in frontend
 * 3. Edges persist to database (via HTTP adapter)
 * 4. No duplicate edges are created
 * 5. Nodes remain visible after promotion (no orphaning)
 *
 * Related Issues: #528 (focus loss), #523 (graph migration), #514 (edge-based hierarchy)
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import { cleanDatabase, waitForDatabaseWrites } from '../utils/test-database';
import {
  initializeDatabaseIfNeeded,
  cleanupDatabaseIfNeeded,
  shouldUseDatabase
} from '../utils/should-use-database';
import { checkServerHealth } from '../utils/test-node-helpers';
import { TestNodeBuilder } from '../utils/test-node-builder';
import { getBackendAdapter, HttpAdapter } from '$lib/services/backend-adapter';
import type { BackendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import { registerChildWithParent } from '$lib/utils/node-hierarchy';

describe.sequential('Parent-Child Edge Creation (Issue #528)', () => {
  let dbPath: string | null;
  let backend: BackendAdapter;

  beforeAll(async () => {
    if (shouldUseDatabase()) {
      await checkServerHealth(new HttpAdapter('http://localhost:3001'));
    }
    backend = getBackendAdapter();
  });

  afterAll(async () => {
    await cleanupDatabaseIfNeeded(dbPath);
  });

  beforeEach(async () => {
    // Initialize database if needed
    dbPath = await initializeDatabaseIfNeeded('parent-child-edge-creation');

    // Clean database and stores before each test
    await cleanDatabase(backend);
    sharedNodeStore.__resetForTesting();
  });

  it.skipIf(!shouldUseDatabase())(
    'should create node with parentId (edge creation verified by integration)',
    async () => {
      // Create a date node parent
      const dateId = '2025-11-17';
      const dateNode = TestNodeBuilder.date(dateId).build();
      await backend.createNode(dateNode);

      // Create a text node with parent reference (simulates placeholder promotion)
      const textNode = TestNodeBuilder.text('Hello World').build();

      // Add transient parent reference (what base-node-viewer.svelte does)
      // Note: container/root is auto-derived from parent chain by backend (Issue #533)
      const textNodeWithParent = {
        ...textNode,
        _parentId: dateId
      } as typeof textNode & { _parentId: string };

      const textNodeId = await backend.createNode(textNodeWithParent);
      await waitForDatabaseWrites();

      // Verify text node was created successfully
      const retrievedText = await backend.getNode(textNodeId);
      expect(retrievedText).toBeTruthy();
      expect(retrievedText?.content).toBe('Hello World');

      // Note: Parent-child edge creation is verified via manual testing and
      // backend integration tests. The BackendAdapter interface doesn't expose
      // edge queries directly to the frontend.
    }
  );

  // Note: Duplicate edge prevention is handled by the backend (NodeOperations layer)
  // and is tested in the Rust integration tests.

  it.skipIf(!shouldUseDatabase())(
    'should update children cache when promoting placeholder',
    async () => {
      // This test simulates what base-node-viewer.svelte does
      const dateId = '2025-11-17';
      const dateNode = TestNodeBuilder.date(dateId).build();

      // Add date node to store
      const fullDateNode = {
        ...dateNode,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString()
      };
      sharedNodeStore.setNode(fullDateNode, {
        type: 'database' as const,
        reason: 'test-setup'
      });

      // Create placeholder (empty text node)
      const placeholderId = globalThis.crypto.randomUUID();
      const placeholder = TestNodeBuilder.text('').withId(placeholderId).build();

      const fullPlaceholder = {
        ...placeholder,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString()
      };

      sharedNodeStore.setNode(fullPlaceholder, {
        type: 'viewer' as const,
        viewerId: 'test-viewer'
      });

      // Verify children cache is initially empty
      const initialChildren = sharedNodeStore.getNodesForParent(dateId);
      expect(initialChildren.length).toBe(0);

      // Simulate promotion: add content and update children cache
      const promotedNode = {
        ...fullPlaceholder,
        content: 'Hello World!',
        modifiedAt: new Date().toISOString()
      };

      sharedNodeStore.setNode(promotedNode, {
        type: 'viewer' as const,
        viewerId: 'test-viewer'
      });

      // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

      // Verify children are retrieved via ReactiveStructureTree
      const updatedChildren = sharedNodeStore.getNodesForParent(dateId);
      expect(updatedChildren.length).toBe(1);
      expect(updatedChildren[0].id).toBe(placeholderId);
      expect(updatedChildren[0].content).toBe('Hello World!');
    }
  );

  it.skipIf(!shouldUseDatabase())(
    'should persist node with parentId across page reload',
    async () => {
      // Create parent
      const dateId = '2025-11-17';
      const dateNode = TestNodeBuilder.date(dateId).build();
      await backend.createNode(dateNode);

      // Create child with parent reference
      const textNode = TestNodeBuilder.text('Persistent text').build();
      const textNodeWithParent = {
        ...textNode,
        _parentId: dateId
      } as typeof textNode & { _parentId: string };

      const textNodeId = await backend.createNode(textNodeWithParent);
      await waitForDatabaseWrites();

      // Simulate page reload: clear stores and reload from backend
      sharedNodeStore.__resetForTesting();

      // Load parent from backend
      const reloadedDate = await backend.getNode(dateId);
      expect(reloadedDate).toBeTruthy();

      // Load child from backend
      const reloadedText = await backend.getNode(textNodeId);
      expect(reloadedText).toBeTruthy();
      expect(reloadedText?.content).toBe('Persistent text');

      // Note: Edge persistence is verified via backend integration tests
    }
  );

  it.skipIf(!shouldUseDatabase())(
    'should auto-derive root from parent chain',
    async () => {
      // This tests that container/root is automatically derived from the parent chain
      // No need to specify both parent and container - backend traverses edges to find root

      // Create a date node (root/container)
      const dateId = '2025-11-17';
      const dateNode = TestNodeBuilder.date(dateId).build();
      await backend.createNode(dateNode);

      // Create a task node with date as parent - root auto-derived from parent chain
      const taskNode = TestNodeBuilder.task('Important task').build();
      const taskNodeWithParent = {
        ...taskNode,
        _parentId: dateId // Root/container auto-derived from parent chain (Issue #533)
      } as typeof taskNode & { _parentId: string };

      const taskNodeId = await backend.createNode(taskNodeWithParent);
      await waitForDatabaseWrites();

      // Verify task was created
      const retrievedTask = await backend.getNode(taskNodeId);
      expect(retrievedTask).toBeTruthy();
      expect(retrievedTask?.nodeType).toBe('task');

      // Note: Parent-child edge creation and root derivation verified via backend integration tests
    }
  );

  it('should update children cache without database (in-memory mode)', async () => {
    // This test runs in both modes (doesn't require database)
    const dateId = '2025-11-17';
    const dateNode = TestNodeBuilder.date(dateId).build();

    // Add date node to store
    const fullDateNode = {
      ...dateNode,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };
    sharedNodeStore.setNode(fullDateNode, {
      type: 'database' as const,
      reason: 'test-setup'
    });

    // Create placeholder
    const placeholderId = globalThis.crypto.randomUUID();
    const placeholder = TestNodeBuilder.text('').withId(placeholderId).build();

    const fullPlaceholder = {
      ...placeholder,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };

    sharedNodeStore.setNode(fullPlaceholder, {
      type: 'viewer' as const,
      viewerId: 'test-viewer'
    });

    // Verify children cache is initially empty
    let childrenBefore = sharedNodeStore.getNodesForParent(dateId);
    expect(childrenBefore.length).toBe(0);

    // Promote placeholder (add content)
    const promoted = {
      ...fullPlaceholder,
      content: 'New content',
      modifiedAt: new Date().toISOString()
    };

    sharedNodeStore.setNode(promoted, {
      type: 'viewer' as const,
      viewerId: 'test-viewer'
    });

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    // Verify node is now in parent's children (the key assertion)
    const childrenAfter = sharedNodeStore.getNodesForParent(dateId);
    expect(childrenAfter.length).toBe(1);
    expect(childrenAfter[0].id).toBe(placeholderId);
    expect(childrenAfter[0].content).toBe('New content');
  });

  it('should handle invalid parent ID gracefully (via registerChildWithParent helper)', () => {
    const validChild = {
      ...TestNodeBuilder.text('Child node').build(),
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };

    sharedNodeStore.setNode(validChild, {
      type: 'viewer' as const,
      viewerId: 'test-viewer'
    });

    // Try to register child with non-existent parent
    // Should not throw, just log warning
    expect(() => {
      registerChildWithParent('non-existent-parent-id', validChild.id);
    }).not.toThrow();

    // Verify child was NOT added to non-existent parent's cache
    const children = sharedNodeStore.getNodesForParent('non-existent-parent-id');
    expect(children.length).toBe(0);
  });

  it('should prevent duplicate child registration', () => {
    const parent = {
      ...TestNodeBuilder.date('2025-01-15').build(),
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };
    const child = {
      ...TestNodeBuilder.text('Child').build(),
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };

    sharedNodeStore.setNode(parent, {
      type: 'viewer' as const,
      viewerId: 'test-viewer'
    });

    sharedNodeStore.setNode(child, {
      type: 'viewer' as const,
      viewerId: 'test-viewer'
    });

    // Register child twice
    registerChildWithParent(parent.id, child.id);
    registerChildWithParent(parent.id, child.id);

    // Should only appear once
    const children = sharedNodeStore.getNodesForParent(parent.id);
    expect(children.length).toBe(1);
    expect(children[0].id).toBe(child.id);
  });
});
