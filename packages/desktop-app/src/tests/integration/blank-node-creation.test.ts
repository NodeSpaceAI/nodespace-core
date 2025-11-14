/**
 * Integration Tests: Blank Text Node Creation (Issue #484)
 *
 * Verifies that blank text nodes can be created, updated, and persisted
 * through the complete request flow (frontend → Tauri → database).
 *
 * Related:
 * - Issue #484: Backend: Allow blank text nodes in validation
 * - Issue #479: Phase 1 - Ephemeral node elimination
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

describe.skipIf(!shouldUseDatabase()).sequential('Blank Text Node Creation (Issue #484)', () => {
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
    dbPath = await initializeDatabaseIfNeeded('blank-node-creation');

    // Clean database between tests to ensure test isolation
    await cleanDatabase(backend);

    // Reset shared node store to clear persistedNodeIds from previous tests
    sharedNodeStore.__resetForTesting();

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  describe('Acceptance Criteria from Issue #484', () => {
    it('should accept blank text nodes via createNode (criterion 1)', async () => {
      // Acceptance Criterion: Backend accepts blank text nodes (content: "")
      const blankNodeData = TestNodeBuilder.text('') // Blank content
        .withType('text')
        .build();

      const nodeId = await backend.createNode(blankNodeData);

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // Verify node was created successfully
      expect(nodeId).toBeTruthy();
      expect(typeof nodeId).toBe('string');
      expect(nodeId.length).toBeGreaterThan(0);

      // Verify node can be retrieved
      const retrievedNode = await backend.getNode(nodeId);
      expect(retrievedNode).toBeTruthy();
      expect(retrievedNode?.content).toBe('');
      expect(retrievedNode?.nodeType).toBe('text');
    });

    it('should return success when creating blank text node (criterion 2)', async () => {
      // Acceptance Criterion: POST /api/nodes with blank text node returns 200
      // (In our architecture, this means no errors from backend.createNode)
      const blankNodeData = TestNodeBuilder.text('')
        .withType('text')
        .build();

      // Should not throw any errors
      let nodeId: string | undefined;
      let error: Error | null = null;

      try {
        nodeId = await backend.createNode(blankNodeData);
      } catch (err) {
        error = err as Error;
      }

      await waitForDatabaseWrites();

      // Verify no error occurred
      expect(error).toBeNull();
      expect(nodeId).toBeTruthy();

      if (shouldUseDatabase()) {
        // No validation errors should be reported
        const errors = sharedNodeStore.getTestErrors();
        expect(errors).toHaveLength(0);
      }
    });

    it('should allow blank nodes to be created and persisted (criterion 3)', async () => {
      // Acceptance Criterion: Blank nodes can be created, updated, and persisted
      // Part 1: Creation and persistence
      const blankNodeData = TestNodeBuilder.text('')
        .withType('text')
        .build();

      const nodeId = await backend.createNode(blankNodeData);

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // Verify persistence by retrieving from database
      const persistedNode = await backend.getNode(nodeId);
      expect(persistedNode).toBeTruthy();
      expect(persistedNode?.content).toBe('');
      expect(persistedNode?.id).toBe(nodeId);
    });

    it('should allow updating existing node to blank content (criterion 3)', async () => {
      // Acceptance Criterion: Blank nodes can be created, updated, and persisted
      // Part 2: Updating to blank content

      // Create node with initial content
      const initialNodeData = TestNodeBuilder.text('Initial content')
        .withType('text')
        .build();

      const nodeId = await backend.createNode(initialNodeData);

      await waitForDatabaseWrites();

      // Update node to blank content
      await backend.updateNode(nodeId, 1, { content: '' });

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // Verify node was updated to blank content
      const retrievedNode = await backend.getNode(nodeId);
      expect(retrievedNode).toBeTruthy();
      expect(retrievedNode?.content).toBe('');
      expect(retrievedNode?.id).toBe(nodeId);
    });

    it('should produce no validation errors for blank content (criterion 4)', async () => {
      // Acceptance Criterion: No validation errors for blank content
      const blankNodeData = TestNodeBuilder.text('')
        .withType('text')
        .build();

      // Should complete without throwing validation errors
      const nodeId = await backend.createNode(blankNodeData);

      await waitForDatabaseWrites();

      // Verify success
      expect(nodeId).toBeTruthy();

      if (shouldUseDatabase()) {
        // Explicitly check for validation errors
        const errors = sharedNodeStore.getTestErrors();
        const validationErrors = errors.filter(
          (e) =>
            e.message.includes('validation') ||
            e.message.includes('MissingField') ||
            e.message.includes('content')
        );
        expect(validationErrors).toHaveLength(0);
      }
    });
  });

  describe('User Workflow Integration', () => {
    it('should support Enter key workflow: create blank node immediately', async () => {
      // Simulates the workflow from Issue #479 Phase 1:
      // User presses Enter → Frontend creates blank node → Backend accepts it

      // Simulate parent node
      const parentNodeData = TestNodeBuilder.text('Parent node content')
        .withType('text')
        .build();
      const parentId = await backend.createNode(parentNodeData);

      await waitForDatabaseWrites();

      // User presses Enter → Create blank child node (immediate persistence)
      const blankChildData = TestNodeBuilder.text('')
        .withType('text')
        .withParent(parentId)
        .build();

      const childId = await backend.createNode(blankChildData);

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // Verify both nodes exist
      const parent = await backend.getNode(parentId);
      const child = await backend.getNode(childId);

      expect(parent).toBeTruthy();
      expect(parent?.content).toBe('Parent node content');

      expect(child).toBeTruthy();
      expect(child?.content).toBe('');
      expect(child?.parentId).toBe(parentId);
    });

    it('should allow user to fill in blank node after creation', async () => {
      // Workflow: Create blank → User types content → Update node

      // Step 1: Create blank node (Enter key)
      const blankNodeData = TestNodeBuilder.text('')
        .withType('text')
        .build();
      const nodeId = await backend.createNode(blankNodeData);

      await waitForDatabaseWrites();

      // Step 2: User types content
      await backend.updateNode(nodeId, 1, { content: 'User typed this content' });

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // Step 3: Verify content was updated
      const finalNode = await backend.getNode(nodeId);
      expect(finalNode).toBeTruthy();
      expect(finalNode?.content).toBe('User typed this content');
    });
  });

  describe('Edge Cases', () => {
    it('should handle multiple blank nodes in sequence', async () => {
      // User presses Enter multiple times, creating several blank nodes
      const blankNode1Data = TestNodeBuilder.text('').withType('text').build();
      const blankNode2Data = TestNodeBuilder.text('').withType('text').build();
      const blankNode3Data = TestNodeBuilder.text('').withType('text').build();

      const id1 = await backend.createNode(blankNode1Data);
      const id2 = await backend.createNode(blankNode2Data);
      const id3 = await backend.createNode(blankNode3Data);

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      // All should be created successfully
      expect(id1).toBeTruthy();
      expect(id2).toBeTruthy();
      expect(id3).toBeTruthy();

      // All should be retrievable
      const node1 = await backend.getNode(id1);
      const node2 = await backend.getNode(id2);
      const node3 = await backend.getNode(id3);

      expect(node1?.content).toBe('');
      expect(node2?.content).toBe('');
      expect(node3?.content).toBe('');
    });

    it('should handle blank node with properties', async () => {
      // Blank content but with metadata properties
      const blankNodeData = TestNodeBuilder.text('')
        .withType('text')
        .withProperty('created_via', 'enter_key')
        .withProperty('timestamp', Date.now())
        .build();

      const nodeId = await backend.createNode(blankNodeData);

      await waitForDatabaseWrites();
      if (shouldUseDatabase()) {
        expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
      }

      const retrievedNode = await backend.getNode(nodeId);
      expect(retrievedNode).toBeTruthy();
      expect(retrievedNode?.content).toBe('');
      expect(retrievedNode?.properties).toHaveProperty('created_via', 'enter_key');
      expect(retrievedNode?.properties).toHaveProperty('timestamp');
    });
  });
});
