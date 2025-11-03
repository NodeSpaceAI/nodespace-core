/**
 * Section 8: Event System Tests (Phase 2 - Real Backend)
 *
 * Tests event emission behavior during node operations with real HTTP backend.
 * Verifies that events fire correctly, once per operation, and in proper sequence.
 *
 * Part of comprehensive test coverage initiative (#208, Phase 2: #211)
 *
 * ## Test Organization
 *
 * - Section 6: Node Ordering (sibling relationships)
 * - Section 7: Database Persistence (when/how nodes persist)
 * - Section 8: Event System (this file - event emissions during operations)
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

import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach } from 'vitest';
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
import { eventBus } from '$lib/services/event-bus';
import type {
  NodeCreatedEvent,
  NodeUpdatedEvent,
  NodeDeletedEvent,
  HierarchyChangedEvent
} from '$lib/services/event-types';
import { sharedNodeStore } from '$lib/services/shared-node-store';

describe.sequential('Section 8: Event System Tests', () => {
  let dbPath: string;
  let backend: BackendAdapter;

  beforeAll(async () => {
    // Create isolated test database for this suite
    dbPath = createTestDatabase('event-emission');
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

    // Note: Due to backend's non-idempotent DELETE behavior (Issue #219),
    // cleanup may report failures for nodes that were already deleted.
    // We accept this as long as SOME cleanup occurred.
    if (!result.success && result.deletedCount === 0 && result.totalCount > 0) {
      // Only fail if we couldn't delete ANY nodes despite nodes existing
      console.warn(
        `[Test] Database cleanup completely failed: ${result.deletedCount}/${result.totalCount} nodes deleted`
      );
      throw new Error('Database cleanup failed - test isolation compromised');
    }

    // Reset event bus to clear all event history and subscribers
    eventBus.reset();

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  afterEach(() => {
    // Reset event bus after each test to prevent memory leaks
    eventBus.reset();
  });

  describe('Event Emission Integrity', () => {
    it('should fire events once per operation (no duplicates)', async () => {
      // Track all events received
      const eventsReceived: string[] = [];

      // Subscribe to all node lifecycle events
      eventBus.subscribe('node:created', () => {
        eventsReceived.push('node:created');
      });
      eventBus.subscribe('node:updated', () => {
        eventsReceived.push('node:updated');
      });
      eventBus.subscribe('node:deleted', () => {
        eventsReceived.push('node:deleted');
      });

      // Perform operations
      const nodeData = TestNodeBuilder.text('Test Node').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.updateNode(nodeId, 1, { content: 'Updated Content' });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.deleteNode(nodeId, 1);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: Each operation should emit exactly one event
      expect(eventsReceived).toEqual(['node:created', 'node:updated', 'node:deleted']);

      // Verify: No duplicate events
      const uniqueEvents = new Set(eventsReceived);
      expect(eventsReceived.length).toBe(uniqueEvents.size);
    });

    it('should fire node:created event with correct nodeId', async () => {
      let capturedEvent: NodeCreatedEvent | null = null;

      // Subscribe to node:created events
      eventBus.subscribe('node:created', (event) => {
        capturedEvent = event as NodeCreatedEvent;
      });

      // Create node
      const nodeData = TestNodeBuilder.text('New Node').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: Event was emitted
      expect(capturedEvent).not.toBeNull();

      // Use non-null assertion since we verified it's not null above
      const event = capturedEvent!;

      // Verify: Event contains correct data
      expect(event.type).toBe('node:created');
      expect(event.nodeId).toBe(nodeId);
      expect(event.nodeType).toBe('text');
      expect(event.namespace).toBe('lifecycle');
      expect(event.source).toBe('HttpAdapter');
      expect(event.timestamp).toBeTypeOf('number');
    });

    it('should fire node:updated event on content change', async () => {
      let capturedEvent: NodeUpdatedEvent | null = null;

      // Create node first
      const nodeData = TestNodeBuilder.text('Initial Content').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Subscribe to node:updated events AFTER creation
      eventBus.subscribe('node:updated', (event) => {
        capturedEvent = event as NodeUpdatedEvent;
      });

      // Update node content
      await backend.updateNode(nodeId, 1, { content: 'Updated Content' });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: Event was emitted
      expect(capturedEvent).not.toBeNull();

      // Use non-null assertion since we verified it's not null above
      const event = capturedEvent!;

      // Verify: Event contains correct data
      expect(event.type).toBe('node:updated');
      expect(event.nodeId).toBe(nodeId);
      expect(event.updateType).toBe('content');
      expect(event.namespace).toBe('lifecycle');
      expect(event.source).toBe('HttpAdapter');
      expect(event.timestamp).toBeTypeOf('number');
    });

    it('should fire node:deleted event on deletion', async () => {
      let capturedEvent: NodeDeletedEvent | null = null;

      // Create node first
      const nodeData = TestNodeBuilder.text('Node to Delete').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Subscribe to node:deleted events AFTER creation
      eventBus.subscribe('node:deleted', (event) => {
        capturedEvent = event as NodeDeletedEvent;
      });

      // Delete node
      await backend.deleteNode(nodeId, 1);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: Event was emitted
      expect(capturedEvent).not.toBeNull();

      // Use non-null assertion since we verified it's not null above
      const event = capturedEvent!;

      // Verify: Event contains correct data
      expect(event.type).toBe('node:deleted');
      expect(event.nodeId).toBe(nodeId);
      expect(event.namespace).toBe('lifecycle');
      expect(event.source).toBe('HttpAdapter');
      expect(event.timestamp).toBeTypeOf('number');
    });

    it('should fire hierarchy:changed event on structural changes', async () => {
      const hierarchyEvents: HierarchyChangedEvent[] = [];

      // Subscribe to hierarchy:changed events
      eventBus.subscribe('hierarchy:changed', (event) => {
        hierarchyEvents.push(event as HierarchyChangedEvent);
      });

      // Create parent node
      const parentData = TestNodeBuilder.text('Parent Node').build();
      const parentId = await backend.createNode(parentData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Create child node (hierarchy change)
      const childData = TestNodeBuilder.text('Child Node').withParent(parentId).build();
      const childId = await backend.createNode(childData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Move child node (hierarchy change via beforeSiblingId update)
      const siblingData = TestNodeBuilder.text('Sibling Node').withParent(parentId).build();
      const siblingId = await backend.createNode(siblingData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.updateNode(childId, 1, { beforeSiblingId: siblingId });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Delete child node (hierarchy change)
      await backend.deleteNode(childId, 1);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: hierarchy:changed events were emitted
      expect(hierarchyEvents.length).toBeGreaterThan(0);

      // Verify: Events have correct structure
      for (const event of hierarchyEvents) {
        expect(event.type).toBe('hierarchy:changed');
        expect(event.namespace).toBe('lifecycle');
        expect(event.source).toBe('HttpAdapter');
        expect(event.affectedNodes).toBeDefined();
        expect(Array.isArray(event.affectedNodes)).toBe(true);
        expect(event.changeType).toMatch(/create|move|delete/);
        expect(event.timestamp).toBeTypeOf('number');
      }
    });

    it('should emit events in correct sequence (logical order)', async () => {
      const eventSequence: string[] = [];

      // Subscribe to all lifecycle events
      eventBus.subscribe('node:created', () => {
        eventSequence.push('created');
      });
      eventBus.subscribe('node:updated', () => {
        eventSequence.push('updated');
      });
      eventBus.subscribe('node:deleted', () => {
        eventSequence.push('deleted');
      });

      // Perform operations in sequence
      const nodeData = TestNodeBuilder.text('Test Node').build();
      const nodeId = await backend.createNode(nodeData);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.updateNode(nodeId, 1, { content: 'Updated 1' });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.updateNode(nodeId, 2, { content: 'Updated 2' });

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      await backend.deleteNode(nodeId, 1);

      await waitForDatabaseWrites();
      expect(sharedNodeStore.getTestErrors()).toHaveLength(0);

      // Verify: Events fired in correct order
      expect(eventSequence).toEqual(['created', 'updated', 'updated', 'deleted']);

      // Verify: Created event comes before updates
      const createdIndex = eventSequence.indexOf('created');
      const firstUpdateIndex = eventSequence.indexOf('updated');
      expect(createdIndex).toBeLessThan(firstUpdateIndex);

      // Verify: Deleted event comes last
      const deletedIndex = eventSequence.indexOf('deleted');
      expect(deletedIndex).toBe(eventSequence.length - 1);
    });
  });
});
