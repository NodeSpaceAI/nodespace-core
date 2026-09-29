import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
import { SharedNodeStore, sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types';
import type { SchemaNode } from '$lib/types/schema-node';
import * as backendAdapterModule from '$lib/services/backend-adapter';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import { schemasStore } from '$lib/stores/schemas.svelte';
import { savedQueriesData } from '$lib/stores/saved-queries.svelte';
import {
  clearAiChatRefreshTimer,
  clearSavedQueryRefreshTimer,
  clearSchemaRefreshTimer
} from '$lib/utils/collection-refresh';

/**
 * Tests for Tauri Domain Event Listener
 *
 * Verifies that TauriSyncListener correctly handles domain events from the Rust backend
 * via Tauri's event system, ensuring real-time sync works correctly in desktop mode.
 *
 * ## Event Flow
 *
 * 1. Backend emits domain events via DomainEventForwarder
 * 2. Tauri event system forwards events to frontend
 * 3. TauriSyncListener handles events and updates stores
 *
 * ## ID-Only Events
 *
 * Events now send only node_id (not full payload). Tests mock backendAdapter.getNode
 * to return test data when frontend fetches node details.
 *
 * ## Test Coverage
 *
 * - Node events (created, updated, deleted)
 * - Edge events (hierarchy created, updated, deleted)
 * - Conditional fetching (nodeUpdated only fetches if node in store)
 * - Error handling for failed fetches
 * - Tauri environment detection
 */

/**
 * Helper to create test nodes with proper schema
 */
function createTestNode(id: string, content = 'Test node'): Node {
  return {
    id,
    nodeType: 'text',
    content,
    properties: {},
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
}

/**
 * Mock node storage for backendAdapter.getNode
 */
const mockNodes = new Map<string, Node>();

/**
 * Mock Tauri event type
 */
interface MockTauriEvent<T = unknown> {
  payload: T;
}

/**
 * Mock Tauri event listeners storage
 * Maps event name to handler function
 */
const mockEventListeners = new Map<string, (event: MockTauriEvent) => void>();

/**
 * Setup mock for backendAdapter.getNode
 */
function setupMockGetNode() {
  vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockImplementation(
    async (id: string) => {
      return mockNodes.get(id) || null;
    }
  );
}

/**
 * Register a node to be returned by mocked getNode
 */
function registerMockNode(node: Node) {
  mockNodes.set(node.id, node);
}

/**
 * Mock Tauri's listen function to capture event listeners
 */
function setupMockTauriListen() {
  // Mock @tauri-apps/api/event module
  vi.mock('@tauri-apps/api/event', () => ({
    listen: vi.fn(async (eventName: string, handler: (event: MockTauriEvent) => void) => {
      mockEventListeners.set(eventName, handler);
      // Return unsubscribe function (not used in tests)
      return () => {
        mockEventListeners.delete(eventName);
      };
    })
  }));
}

/**
 * Simulate emitting a Tauri event
 */
function emitTauriEvent(eventName: string, payload: unknown) {
  const handler = mockEventListeners.get(eventName);
  if (handler) {
    handler({ payload });
  } else {
    throw new Error(`No listener registered for event: ${eventName}`);
  }
}

/**
 * Mock Tauri environment detection
 */
function mockTauriEnvironment(isTauri: boolean) {
  interface WindowWithTauri extends Window {
    __TAURI__?: Record<string, unknown>;
  }

  if (isTauri) {
    (global.window as WindowWithTauri).__TAURI__ = {};
  } else {
    delete (global.window as WindowWithTauri).__TAURI__;
  }
}

/** Let the burst-coalescing window (one frame) fire and its flushes complete. */
const settleCoalescer = () => new Promise<void>((resolve) => setTimeout(resolve, 40));

describe('TauriSyncListener', () => {
  beforeEach(() => {
    // Reset stores
    SharedNodeStore.resetInstance();
    structureTree.clear();
    mockNodes.clear();
    mockEventListeners.clear();
    aiChatsData.reset();

    // Setup mocks
    setupMockGetNode();
    setupMockTauriListen();
    mockTauriEnvironment(true);
  });

  afterEach(() => {
    // Cleanup
    sharedNodeStore.clearAll();
    structureTree.clear();
    SharedNodeStore.resetInstance();
    mockNodes.clear();
    mockEventListeners.clear();
    vi.restoreAllMocks();
    aiChatsData.reset();
    clearAiChatRefreshTimer();
    savedQueriesData.reset();
    clearSavedQueryRefreshTimer();
    // This file installs the Tauri bridge markers; clear them rather than relying on a
    // later file's setup to do it, which only works by accident of ordering.
    Reflect.deleteProperty(window, '__TAURI__');
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    // schema-plugin-loader.ts's registry is a real, unmocked singleton in this
    // file (only getNode/getSchema are spied per-test) — clear it so a plugin
    // registered by one test can't leak into the next.
    pluginRegistry.clear();
  });

  describe('Environment Detection', () => {
    it('should skip initialization when not in Tauri environment', async () => {
      mockTauriEnvironment(false);

      await initializeTauriSyncListeners();

      // No listeners should be registered
      expect(mockEventListeners.size).toBe(0);
    });

    it('should initialize listeners in Tauri environment', async () => {
      mockTauriEnvironment(true);

      await initializeTauriSyncListeners();

      // Verify all expected listeners are registered
      expect(mockEventListeners.has('node:created')).toBe(true);
      expect(mockEventListeners.has('node:updated')).toBe(true);
      expect(mockEventListeners.has('node:deleted')).toBe(true);
      // Unified relationship events replace old edge:* events
      expect(mockEventListeners.has('relationship:created')).toBe(true);
      expect(mockEventListeners.has('relationship:updated')).toBe(true);
      expect(mockEventListeners.has('relationship:deleted')).toBe(true);
      expect(mockEventListeners.has('sync:error')).toBe(true);
      expect(mockEventListeners.has('sync:status')).toBe(true);
    });
  });

  describe('Node Events - ID-Only Optimization', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('should fetch and store node on node:created event', async () => {
      const testNode = createTestNode('node1', 'New node');
      registerMockNode(testNode);

      emitTauriEvent('node:created', { id: 'node1' });

      // Wait for async fetch to complete
      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('node1')).toBe(true);
      });

      const storedNode = sharedNodeStore.getNode('node1');
      expect(storedNode).toBeDefined();
      expect(storedNode?.content).toBe('New node');
    });

    it('should always fetch node data on node:created (unconditional)', async () => {
      const testNode = createTestNode('node1');
      registerMockNode(testNode);

      // Node is NOT in store yet
      expect(sharedNodeStore.hasNode('node1')).toBe(false);

      emitTauriEvent('node:created', { id: 'node1' });

      // Should fetch even though node is not in store
      await vi.waitFor(() => {
        expect(backendAdapterModule.backendAdapter.getNode).toHaveBeenCalledWith('node1');
      });
    });

    it('should only fetch node:updated if node already in store', async () => {
      const testNode = createTestNode('node1', 'Original content');
      registerMockNode(testNode);

      // Pre-populate store with node
      sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);

      // Update node in mock backend
      const updatedNode = { ...testNode, content: 'Updated content' };
      registerMockNode(updatedNode);

      emitTauriEvent('node:updated', { id: 'node1' });

      // Should fetch since node is in store
      await vi.waitFor(() => {
        const storedNode = sharedNodeStore.getNode('node1');
        expect(storedNode?.content).toBe('Updated content');
      });
    });

    it('should fetch node:updated even if node not yet in store', async () => {
      const testNode = createTestNode('node1');
      registerMockNode(testNode);

      // Node is NOT in store
      expect(sharedNodeStore.hasNode('node1')).toBe(false);

      emitTauriEvent('node:updated', { id: 'node1' });

      // Should fetch and add to store (daemon may update ai-chat nodes not yet loaded)
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(sharedNodeStore.hasNode('node1')).toBe(true);
    });

    it('should delete node on node:deleted event', async () => {
      const testNode = createTestNode('node1');
      sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);

      expect(sharedNodeStore.hasNode('node1')).toBe(true);

      emitTauriEvent('node:deleted', { id: 'node1' });

      expect(sharedNodeStore.hasNode('node1')).toBe(false);
    });
  });

  // A schema's plugin registration (hasTitleTemplate/titleTemplate)
  // was only ever refreshed on node:created — node:updated (e.g. update_schema
  // adding a title_template to an existing custom type mid-session) never
  // touched it, so resolveDisplayTitle kept using the stale flag until a full
  // app restart.
  describe('Schema plugin refresh on node:updated', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    function mockSchema(id: string, titleTemplate?: string): SchemaNode {
      return {
        id,
        content: 'Test Schema',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 2,
        isCore: false,
        schemaVersion: 2,
        description: '',
        fields: [],
        titleTemplate
      };
    }

    it('refreshes an existing schema plugin`s title template', async () => {
      const schemaId = 'sync-listener-test-schema';

      // node:updated's payload carries only an id (no nodeType — see
      // NodeEventData) — the *fetched* node's type is what must gate the
      // refresh, so give it nodeType: 'schema'.
      registerMockNode({
        id: schemaId,
        nodeType: 'schema',
        content: 'Test Schema',
        properties: {},
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 2
      });
      vi.spyOn(backendAdapterModule.backendAdapter, 'getSchema').mockResolvedValue(
        mockSchema(schemaId, '{first_name} {last_name}')
      );

      expect(pluginRegistry.hasTitleTemplate(schemaId)).toBe(false);

      emitTauriEvent('node:updated', { id: schemaId });

      await vi.waitFor(() => {
        expect(pluginRegistry.hasTitleTemplate(schemaId)).toBe(true);
      });
      expect(pluginRegistry.getTitleTemplate(schemaId)).toBe('{first_name} {last_name}');
    });

    it('does not touch the plugin registry for a non-schema node:updated', async () => {
      const getSchemaSpy = vi.spyOn(backendAdapterModule.backendAdapter, 'getSchema');
      registerMockNode(createTestNode('node1', 'Just a text node'));

      emitTauriEvent('node:updated', { id: 'node1' });

      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('node1')).toBe(true);
      });
      expect(getSchemaSpy).not.toHaveBeenCalled();
    });
  });

  // The AI Chats sidebar list had no node:created/node:updated
  // wiring at all — unlike collections and schemas (scheduleCollectionRefresh/
  // scheduleSchemaRefresh), so an externally-created chat never appeared and
  // background titling's node:updated (which writes the real title into
  // content) never refreshed the list's "Untitled chat" placeholder.
  describe('AI chats sidebar refresh on node:created / node:updated', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    function mockAiChatNode(id: string, content: string): Node {
      return {
        id,
        nodeType: 'ai-chat',
        content,
        properties: {},
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1
      };
    }

    it('refreshes the AI chats list when an ai-chat node is created externally', async () => {
      const created = mockAiChatNode('chat-1', '');
      registerMockNode(created);
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([created]);

      emitTauriEvent('node:created', { id: 'chat-1', nodeType: 'ai-chat' });

      await vi.waitFor(
        () => {
          expect(aiChatsData.state.chats.map((c) => c.id)).toContain('chat-1');
        },
        { timeout: 1000 }
      );
    });

    it('refreshes the AI chats list when background titling updates a chat via node:updated', async () => {
      // node:updated's payload never carries nodeType (see NodeEventData) — the
      // refresh must be gated on the *fetched* node's type, not the event
      // payload, or this case (the actual #2221 failure scenario) can't work
      // at all.
      const titled = mockAiChatNode('chat-2', 'Quarterly Planning');
      registerMockNode(titled);
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([titled]);

      emitTauriEvent('node:updated', { id: 'chat-2' });

      await vi.waitFor(
        () => {
          expect(aiChatsData.state.chats.find((c) => c.id === 'chat-2')?.content).toBe(
            'Quarterly Planning'
          );
        },
        { timeout: 1000 }
      );
    });

    it('does not schedule an AI chats refresh for a non-ai-chat node update', async () => {
      registerMockNode(createTestNode('node1', 'Just a text node'));
      const queryNodesSpy = vi
        .spyOn(backendAdapterModule.backendAdapter, 'queryNodes')
        .mockResolvedValue([]);

      emitTauriEvent('node:updated', { id: 'node1' });

      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('node1')).toBe(true);
      });
      // Give the (would-be) debounce window ample time to fire if a refresh
      // had incorrectly been scheduled.
      await new Promise((resolve) => setTimeout(resolve, 350));
      expect(queryNodesSpy).not.toHaveBeenCalled();
    });

    it('coalesced path also refreshes the AI chats list', async () => {
      const created = mockAiChatNode('chat-pro', 'From coalesced burst');
      registerMockNode(created);
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([created]);

      emitTauriEvent('node:created', { id: 'chat-pro', nodeType: 'ai-chat' });

      await vi.waitFor(
        () => {
          expect(aiChatsData.state.chats.map((c) => c.id)).toContain('chat-pro');
        },
        { timeout: 1000 }
      );
    });
  });

  // Saved queries are listed under their node type in the sidebar; a query
  // created, renamed or deleted out-of-band (e.g. a playbook install) must
  // reach that list without a reload.
  describe('saved queries sidebar refresh', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    function mockQueryNode(id: string, name: string, targetType: string): Node {
      return {
        id,
        nodeType: 'query',
        content: name,
        targetType,
        properties: {},
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1
      } as unknown as Node;
    }

    it('lists a query created externally under its target type', async () => {
      const created = mockQueryNode('q1', 'Specs by Status', 'spec');
      registerMockNode(created);
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([created]);

      emitTauriEvent('node:created', { id: 'q1', nodeType: 'query' });

      await vi.waitFor(
        () => {
          expect(savedQueriesData.forType('spec').map((q) => q.name)).toEqual(['Specs by Status']);
        },
        { timeout: 1000 }
      );
    });

    it('reflects a rename delivered as node:updated (payload has no nodeType)', async () => {
      const renamed = mockQueryNode('q1', 'Renamed', 'spec');
      registerMockNode(renamed);
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([renamed]);

      emitTauriEvent('node:updated', { id: 'q1' });

      await vi.waitFor(
        () => {
          expect(savedQueriesData.forType('spec').map((q) => q.name)).toEqual(['Renamed']);
        },
        { timeout: 1000 }
      );
    });

    it('drops a listed query when it is deleted', async () => {
      savedQueriesData.queries = [{ id: 'q1', name: 'Gone', targetType: 'spec' }];
      vi.spyOn(backendAdapterModule.backendAdapter, 'queryNodes').mockResolvedValue([]);

      emitTauriEvent('node:deleted', { id: 'q1' });

      await vi.waitFor(() => expect(savedQueriesData.queries).toEqual([]), { timeout: 1000 });
    });

    it('does not reload saved queries for a non-query node update', async () => {
      registerMockNode(createTestNode('node1', 'Just a text node'));
      const queryNodesSpy = vi
        .spyOn(backendAdapterModule.backendAdapter, 'queryNodes')
        .mockResolvedValue([]);

      emitTauriEvent('node:updated', { id: 'node1' });

      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('node1')).toBe(true);
      });
      await new Promise((resolve) => setTimeout(resolve, 350));
      expect(queryNodesSpy).not.toHaveBeenCalled();
    });
  });

  // When sync is active, a reconnect-replay burst of node events is
  // coalesced — collected over a short window, then applied in one synchronous
  // pass so the caught-up set renders once instead of once per node.
  describe('Node burst render coalescing', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('defers a burst then applies every node in one pass', async () => {
      for (const id of ['n1', 'n2', 'n3']) {
        registerMockNode(createTestNode(id, `content ${id}`));
        emitTauriEvent('node:created', { id });
      }

      // Synchronously after the burst the applies are still deferred (the
      // coalescing window hasn't fired) — proves we don't render per node.
      expect(sharedNodeStore.hasNode('n1')).toBe(false);
      expect(sharedNodeStore.hasNode('n2')).toBe(false);
      expect(sharedNodeStore.hasNode('n3')).toBe(false);

      // After the window flushes, the whole burst has landed.
      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('n1')).toBe(true);
        expect(sharedNodeStore.hasNode('n2')).toBe(true);
        expect(sharedNodeStore.hasNode('n3')).toBe(true);
      });
      expect(sharedNodeStore.getNode('n2')?.content).toBe('content n2');
    });

    it('a delete during the window wins — the queued upsert does not resurrect the node', async () => {
      // Node is updated (queued for coalesced re-fetch) then deleted before the
      // window flushes. Even though getNode would still return it, the delete
      // must win — the coalescer evicts the pending re-fetch.
      registerMockNode(createTestNode('zombie', 'should not come back'));

      emitTauriEvent('node:updated', { id: 'zombie' }); // queued
      emitTauriEvent('node:deleted', { id: 'zombie' }); // evicts the queued re-fetch

      // Give the coalescing window time to fire.
      await new Promise((resolve) => setTimeout(resolve, 40));

      expect(sharedNodeStore.hasNode('zombie')).toBe(false);
    });
  });

  // A cloud-sync pull floods relationship events (tens of thousands for a
  // populated tenant). Applied one-by-one each event costs a full reactive
  // invalidation of the structure tree on the main thread. When sync is
  // active the has_child ops are buffered over the coalescing window and the
  // whole burst is applied inside one structureTree batch.
  describe('Relationship-event coalescing', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    function emitHasChildCreated(parentId: string, childId: string, order: number): void {
      emitTauriEvent('relationship:created', {
        id: `relationship:${parentId}:${childId}`,
        fromId: `node:${parentId}`,
        toId: `node:${childId}`,
        relationshipType: 'has_child',
        properties: { order }
      });
    }

    function emitHasChildDeleted(parentId: string, childId: string): void {
      emitTauriEvent('relationship:deleted', {
        id: `relationship:${parentId}:${childId}`,
        fromId: `node:${parentId}`,
        toId: `node:${childId}`,
        relationshipType: 'has_child'
      });
    }

    it('applies a burst of relationship:created events in a single batch', async () => {
      const runBatchSpy = vi.spyOn(structureTree, 'runBatch');

      const burst = 25;
      for (let i = 0; i < burst; i++) {
        emitHasChildCreated('parent1', `child${i}`, i + 1);
      }

      // Synchronously after the burst nothing has been applied — the ops are
      // buffered, not applied per event.
      expect(structureTree.getChildren('parent1')).toHaveLength(0);
      expect(runBatchSpy).not.toHaveBeenCalled();

      // After the window flushes, the whole burst has landed ...
      await vi.waitFor(() => {
        expect(structureTree.getChildren('parent1')).toHaveLength(burst);
      });
      // ... through exactly one batch (one reactive notification), not N.
      expect(runBatchSpy).toHaveBeenCalledTimes(1);
      // Per-event order values were honored (children sorted by order).
      expect(structureTree.getChildren('parent1')[0]).toBe('child0');
      expect(structureTree.getChildren('parent1')[burst - 1]).toBe(`child${burst - 1}`);
    });

    it('create followed by delete of the same edge in one window resolves to absent', async () => {
      emitHasChildCreated('parent1', 'child1', 1);
      emitHasChildDeleted('parent1', 'child1');

      // Let the coalescing window flush.
      await new Promise((resolve) => setTimeout(resolve, 40));

      expect(structureTree.getChildren('parent1')).toHaveLength(0);
      expect(structureTree.getParent('child1')).toBeNull();
    });

    it('delete followed by re-create of the same edge in one window resolves to present', async () => {
      // Edge exists before the window opens.
      structureTree.addChild({ parentId: 'parent1', childId: 'child1', order: 1 });

      emitHasChildDeleted('parent1', 'child1');
      emitHasChildCreated('parent1', 'child1', 2);

      await new Promise((resolve) => setTimeout(resolve, 40));

      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
      expect(structureTree.getChildrenWithOrder('parent1')[0].order).toBe(2);
    });

    it('drops relationship ops buffered before a database switch', async () => {
      emitHasChildCreated('parent1', 'child1', 1); // buffered

      // The active database switches (epoch bump) before the window flushes.
      sharedNodeStore.clearAll();

      await new Promise((resolve) => setTimeout(resolve, 40));

      // The buffered edge belonged to the previous database — dropped.
      expect(structureTree.getChildren('parent1')).toHaveLength(0);
    });
  });

  describe('Large import burst', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('handles a 32k node + has_child burst without per-event work, in bounded fetch chunks', async () => {
      const total = 32000;
      for (let i = 0; i < total; i++) registerMockNode(createTestNode(`b${i}`, `c${i}`));
      const getNodeSpy = vi.mocked(backendAdapterModule.backendAdapter.getNode);
      const runBatchSpy = vi.spyOn(structureTree, 'runBatch');
      let inFlight = 0;
      let maxInFlight = 0;
      getNodeSpy.mockImplementation(async (id: string) => {
        inFlight++;
        maxInFlight = Math.max(maxInFlight, inFlight);
        await Promise.resolve();
        inFlight--;
        return mockNodes.get(id) || null;
      });

      for (let i = 0; i < total; i++) {
        emitTauriEvent('node:created', { id: `b${i}` });
        emitTauriEvent('relationship:created', {
          id: `relationship:r${i}`,
          fromId: `node:bp${Math.floor(i / 10)}`,
          toId: `node:b${i}`,
          relationshipType: 'has_child',
          properties: { order: (i % 10) + 1 }
        });
      }

      // Nothing was fetched or applied inside the event handlers: the whole
      // burst is deferred, so the main thread is free right after the last event.
      expect(getNodeSpy).not.toHaveBeenCalled();
      expect(runBatchSpy).not.toHaveBeenCalled();
      expect(sharedNodeStore.hasNode('b0')).toBe(false);

      await vi.waitFor(() => expect(sharedNodeStore.hasNode(`b${total - 1}`)).toBe(true), {
        timeout: 20000,
        interval: 50
      });
      expect(getNodeSpy).toHaveBeenCalledTimes(total);
      expect(maxInFlight).toBeLessThanOrEqual(200);
      // One batch for all relationship ops, not one per edge.
      expect(runBatchSpy).toHaveBeenCalledTimes(1);
      expect(structureTree.getChildren('bp0')).toHaveLength(10);
      expect(structureTree.getChildren(`bp${total / 10 - 1}`)).toHaveLength(10);
    }, 60000);
  });

  describe('Flush error isolation', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('a node that throws while being applied does not drop the rest of the burst', async () => {
      for (const id of ['e1', 'e2', 'e3']) registerMockNode(createTestNode(id));
      const realSetNode = sharedNodeStore.setNode.bind(sharedNodeStore);
      vi.spyOn(sharedNodeStore, 'setNode').mockImplementation((node, source, skip) => {
        if (node.id === 'e2') throw new Error('boom');
        return realSetNode(node, source, skip);
      });

      for (const id of ['e1', 'e2', 'e3']) emitTauriEvent('node:created', { id });
      await settleCoalescer();

      expect(sharedNodeStore.hasNode('e1')).toBe(true);
      expect(sharedNodeStore.hasNode('e2')).toBe(false);
      expect(sharedNodeStore.hasNode('e3')).toBe(true);
    });
  });

  // A coalesced node flush processes ids in chunks with a macrotask yield
  // between them, so a huge burst (initial pull) cannot monopolize the main
  // thread — and the database-switch guard is re-checked per chunk.
  describe('Node-fetch flush chunking', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    /** Mock getNode with per-id deferred promises so chunk boundaries are
     *  observable and controllable. */
    function setupDeferredGetNode(): Map<string, (node: Node | null) => void> {
      const resolvers = new Map<string, (node: Node | null) => void>();
      vi.mocked(backendAdapterModule.backendAdapter.getNode).mockImplementation(
        (id: string) =>
          new Promise<Node | null>((resolve) => {
            resolvers.set(id, resolve);
          })
      );
      return resolvers;
    }

    function getNodeCalls(): string[] {
      return vi
        .mocked(backendAdapterModule.backendAdapter.getNode)
        .mock.calls.map((call) => call[0]);
    }

    it('fetches a large burst in chunks of 200, yielding between chunks', async () => {
      const resolvers = setupDeferredGetNode();

      const total = 250; // chunk size is 200 → two chunks
      for (let i = 0; i < total; i++) {
        emitTauriEvent('node:created', { id: `bulk${i}` });
      }

      // Only the first chunk's fetches are dispatched — the flush does not
      // fan out all 250 at once.
      await vi.waitFor(() => {
        expect(getNodeCalls()).toHaveLength(200);
      });
      expect(getNodeCalls()).not.toContain('bulk200');

      // Completing chunk 1 lets the flush apply it, yield, then dispatch
      // chunk 2.
      for (let i = 0; i < 200; i++) {
        resolvers.get(`bulk${i}`)!(createTestNode(`bulk${i}`));
      }
      await vi.waitFor(() => {
        expect(getNodeCalls()).toHaveLength(250);
      });
      for (let i = 200; i < total; i++) {
        resolvers.get(`bulk${i}`)!(createTestNode(`bulk${i}`));
      }

      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('bulk0')).toBe(true);
        expect(sharedNodeStore.hasNode('bulk199')).toBe(true);
        expect(sharedNodeStore.hasNode('bulk249')).toBe(true);
      });
    });

    it('re-checks the database-switch guard per chunk — a switch between chunks drops the remainder', async () => {
      const resolvers = setupDeferredGetNode();

      for (let i = 0; i < 250; i++) {
        emitTauriEvent('node:created', { id: `bulk${i}` });
      }
      await vi.waitFor(() => {
        expect(getNodeCalls()).toHaveLength(200);
      });

      // Resolve chunk 1, and queue the database switch as a macrotask: it
      // runs after chunk 1 is applied (a microtask) but before the flush's
      // between-chunk yield timer, which is scheduled later than this one.
      for (let i = 0; i < 200; i++) {
        resolvers.get(`bulk${i}`)!(createTestNode(`bulk${i}`));
      }
      setTimeout(() => sharedNodeStore.clearAll(), 0);

      // Give the flush ample time to (incorrectly) dispatch chunk 2.
      await new Promise((resolve) => setTimeout(resolve, 60));

      // The per-chunk guard saw the epoch change at the chunk boundary and
      // dropped the remainder — chunk 2 was never even fetched.
      expect(getNodeCalls()).toHaveLength(200);
      expect(getNodeCalls()).not.toContain('bulk200');
      expect(sharedNodeStore.hasNode('bulk200')).toBe(false);
    });
  });

  // ADR-053: switching the active database bumps the store's database epoch.
  // A domain-event hydration whose read was already in flight across that
  // switch resolves with the previous database's row — it must be dropped, not
  // written into the now-active store, or the switch leaks orphan nodes.
  describe('ADR-053 in-flight read drop across a database switch', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('coalescer drops a queued burst that resolves after a switch', async () => {
            let resolveGet!: (node: Node | null) => void;
      const pending = new Promise<Node | null>((resolve) => {
        resolveGet = resolve;
      });
      vi.mocked(backendAdapterModule.backendAdapter.getNode).mockImplementation(() => pending);

      // Queue the event; let the coalescing window fire so the flush reaches
      // its (now-pending) read.
      emitTauriEvent('node:created', { id: 'node1' });
      await new Promise((resolve) => setTimeout(resolve, 40));

      // Switch databases while the coalesced read is outstanding.
      sharedNodeStore.clearAll();

      resolveGet(createTestNode('node1'));
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(sharedNodeStore.hasNode('node1')).toBe(false);
    });
  });

  describe('Unified Relationship Events - has_child', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('should add hierarchy edge on relationship:created with has_child type', async () => {
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child',
        properties: { order: 100 }
      });

      await settleCoalescer();
      const children = structureTree.getChildrenWithOrder('parent1');
      expect(children).toHaveLength(1);
      expect(children[0].nodeId).toBe('child1');
      expect(children[0].order).toBe(100);
    });

    it('should not add duplicate edges (idempotent)', async () => {
      // Add edge first time
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child',
        properties: { order: 100 }
      });

      // Try to add same edge again
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child',
        properties: { order: 100 }
      });

      await settleCoalescer();
      // Should still only have one child
      const children = structureTree.getChildrenWithOrder('parent1');
      expect(children).toHaveLength(1);
    });

    it('should remove hierarchy edge on relationship:deleted with has_child type', async () => {
      // Add edge first
      structureTree.addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 100
      });

      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(1);

      // Delete edge
      emitTauriEvent('relationship:deleted', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child'
      });

      await settleCoalescer();
      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(0);
    });

    it('should update child order on relationship:updated for has_child', async () => {
      // Add edge first at order 100
      structureTree.addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 100
      });

      // Emit relationship:updated with new order
      emitTauriEvent('relationship:updated', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child',
        properties: { order: 200 }
      });

      await settleCoalescer();
      const children = structureTree.getChildrenWithOrder('parent1');
      expect(children[0].order).toBe(200);
    });
  });

  describe('Relationship Events — node: prefix normalization', () => {
    // Backend's `RelationshipEvent` serialization contract emits
    // `from_id` / `to_id` already prefixed with `node:`. The
    // listener's `stripNodePrefix`
    // helper normalizes at the boundary so the structureTree's
    // bare-id keyspace stays consistent with the local-action path.
    // Without these tests, deleting `stripNodePrefix` from any of
    // the call sites would silently regress (existing tests pass
    // bare ids only).
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('strips node: prefix on relationship:created → addChild', async () => {
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent1:child1',
        fromId: 'node:parent1',
        toId: 'node:child1',
        relationshipType: 'has_child',
        properties: { order: 150 }
      });

      await settleCoalescer();
      // structureTree must see bare ids. If stripNodePrefix were
      // removed, the key would be "node:parent1" and this lookup
      // against the bare-id key "parent1" would return empty.
      const children = structureTree.getChildrenWithOrder('parent1');
      expect(children).toHaveLength(1);
      expect(children[0].nodeId).toBe('child1');
      expect(children[0].order).toBe(150);

      // And the prefixed key must NOT have been used.
      expect(structureTree.getChildrenWithOrder('node:parent1')).toHaveLength(0);
    });

    it('strips node: prefix on relationship:updated → updateChildOrder', async () => {
      // Seed with bare ids (matches what the local-action path does).
      structureTree.addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 100
      });

      emitTauriEvent('relationship:updated', {
        id: 'relationship:parent1:child1',
        fromId: 'node:parent1',
        toId: 'node:child1',
        relationshipType: 'has_child',
        properties: { order: 250 }
      });

      await settleCoalescer();
      const children = structureTree.getChildrenWithOrder('parent1');
      expect(children).toHaveLength(1);
      expect(children[0].order).toBe(250);
    });

    it('strips node: prefix on relationship:deleted → removeChild', async () => {
      structureTree.addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 100
      });
      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(1);

      emitTauriEvent('relationship:deleted', {
        id: 'relationship:parent1:child1',
        fromId: 'node:parent1',
        toId: 'node:child1',
        relationshipType: 'has_child'
      });

      await settleCoalescer();
      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(0);
    });

    it('handles a mix of prefixed-and-bare ids identically (defensive)', async () => {
      // In practice the contract is "always prefixed" — but the
      // helper is a pass-through for already-bare ids, and the
      // listener shouldn't behave differently if a future
      // backend (or replay tool) emits the bare form.
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent2:child2',
        fromId: 'parent2', // bare
        toId: 'node:child2', // prefixed
        relationshipType: 'has_child',
        properties: { order: 100 }
      });

      await settleCoalescer();
      const children = structureTree.getChildrenWithOrder('parent2');
      expect(children).toHaveLength(1);
      expect(children[0].nodeId).toBe('child2');
    });
  });

  describe('Unified Relationship Events - Mentions (backlinks refresh)', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('refreshes mentionedIn for the target node on relationship:created', async () => {
      const targetNode = createTestNode('node2', 'Target node');
      sharedNodeStore.setNode(targetNode, { type: 'database', reason: 'test' }, false);

      vi.spyOn(backendAdapterModule.backendAdapter, 'getMentioningContainers').mockResolvedValue([
        { id: 'node1', title: 'Source node', nodeType: 'text' }
      ]);

      emitTauriEvent('relationship:created', {
        id: 'relationship:mention:node1:node2',
        fromId: 'node1',
        toId: 'node2',
        relationshipType: 'mentions',
        properties: {}
      });

      await vi.waitFor(() => {
        expect(backendAdapterModule.backendAdapter.getMentioningContainers).toHaveBeenCalledWith(
          'node2'
        );
      });

      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('node2')?.mentionedIn).toEqual([
          { id: 'node1', title: 'Source node', nodeType: 'text' }
        ]);
      });
    });

    it('does not throw on relationship:updated with mentions type (no dedicated handling)', async () => {
      expect(() => {
        emitTauriEvent('relationship:updated', {
          id: 'relationship:mention:node1:node2',
          fromId: 'node1',
          toId: 'node2',
          relationshipType: 'mentions',
          properties: {}
        });
      }).not.toThrow();
    });

    it('refreshes mentionedIn for the target node on relationship:deleted', async () => {
      const targetNode = createTestNode('node2', 'Target node');
      sharedNodeStore.setNode(
        { ...targetNode, mentionedIn: [{ id: 'node1', title: 'Source node', nodeType: 'text' }] },
        { type: 'database', reason: 'test' },
        false
      );

      vi.spyOn(backendAdapterModule.backendAdapter, 'getMentioningContainers').mockResolvedValue([]);

      emitTauriEvent('relationship:deleted', {
        id: 'relationship:mention:node1:node2',
        fromId: 'node1',
        toId: 'node2',
        relationshipType: 'mentions'
      });

      await vi.waitFor(() => {
        expect(backendAdapterModule.backendAdapter.getMentioningContainers).toHaveBeenCalledWith(
          'node2'
        );
      });

      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('node2')?.mentionedIn).toEqual([]);
      });
    });

    it('does not fetch backlinks for a target node that is not currently cached', async () => {
      const getMentioningContainersSpy = vi
        .spyOn(backendAdapterModule.backendAdapter, 'getMentioningContainers')
        .mockResolvedValue([{ id: 'node1', title: 'Source', nodeType: 'text' }]);

      expect(sharedNodeStore.hasNode('uncached-target')).toBe(false);

      emitTauriEvent('relationship:created', {
        id: 'relationship:mention:node1:uncached-target',
        fromId: 'node1',
        toId: 'uncached-target',
        relationshipType: 'mentions',
        properties: {}
      });

      // Give any (incorrectly-fired) async refresh a tick to resolve, then
      // confirm it never happened — the node isn't displayed, so nothing to
      // refresh (see `refreshMentionedIn`'s cache guard).
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(getMentioningContainersSpy).not.toHaveBeenCalled();
    });
  });

  // A write that reaches the database without going through sharedNodeStore
  // (onboarding's identity step, a playbook install, the CLI, an agent) is
  // visible to the UI only through these events. The store may already hold
  // a stale copy of the node from before the write.
  describe('Out-of-band backend writes', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    afterEach(() => {
      clearSchemaRefreshTimer();
      schemasStore.schemas = [];
    });

    it('node:updated replaces a stale cached copy with the backend version', async () => {
      const stale = { ...createTestNode('person-1', ''), nodeType: 'person' };
      sharedNodeStore.setNode(stale, { type: 'database', reason: 'test' }, true);

      registerMockNode({
        ...stale,
        title: 'Ada Lovelace',
        properties: { first_name: 'Ada', last_name: 'Lovelace', email: 'ada@example.com' },
        version: 2
      } as Node);

      emitTauriEvent('node:updated', { id: 'person-1' });

      await vi.waitFor(() => {
        const node = sharedNodeStore.getNode('person-1');
        expect(node?.version).toBe(2);
        expect(node?.title).toBe('Ada Lovelace');
      });
    });

    it('node:created for a schema adds it to the sidebar type list', async () => {
      const spec: SchemaNode = {
        id: 'spec',
        content: 'Spec',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        isCore: false,
        schemaVersion: 1,
        description: '',
        fields: []
      };
      vi.spyOn(backendAdapterModule.backendAdapter, 'getAllSchemas').mockResolvedValue([spec]);
      vi.spyOn(backendAdapterModule.backendAdapter, 'getSchema').mockResolvedValue(spec);
      expect(schemasStore.customSchemas).toEqual([]);

      emitTauriEvent('node:created', { id: 'spec', nodeType: 'schema' });

      await vi.waitFor(
        () => {
          expect(schemasStore.customSchemas.map((s) => s.id)).toEqual(['spec']);
        },
        { timeout: 1000 }
      );
    });
  });

  describe('Error Handling', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('should handle failed node fetch gracefully', async () => {
      // Mock getNode to throw error
      vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockRejectedValue(
        new Error('Network error')
      );

      // Should not throw
      expect(() => {
        emitTauriEvent('node:created', { id: 'node1' });
      }).not.toThrow();

      // Node should not be in store
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(sharedNodeStore.hasNode('node1')).toBe(false);
    });

    it('should handle node not found (returns null)', async () => {
      // Mock getNode to return null
      vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockResolvedValue(null);

      emitTauriEvent('node:created', { id: 'nonexistent' });

      // Should not crash, node should not be in store
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(sharedNodeStore.hasNode('nonexistent')).toBe(false);
    });

    it('should handle sync:error events', async () => {
      // Should not crash
      expect(() => {
        emitTauriEvent('sync:error', {
          message: 'Database connection lost',
          errorType: 'connection'
        });
      }).not.toThrow();
    });

    it('should handle sync:status events', async () => {
      // Should not crash
      expect(() => {
        emitTauriEvent('sync:status', {
          status: 'connected',
          reason: 'Initial connection'
        });
      }).not.toThrow();
    });
  });

  describe('Event Ordering Scenarios', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('should handle relationship created before node exists', async () => {
      // Relationship event arrives first
      emitTauriEvent('relationship:created', {
        id: 'relationship:parent1:child1',
        fromId: 'parent1',
        toId: 'child1',
        relationshipType: 'has_child',
        properties: { order: 100 }
      });

      await settleCoalescer();
      // Edge should be in structure tree
      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(1);

      // Node event arrives later
      const testNode = createTestNode('child1');
      registerMockNode(testNode);
      emitTauriEvent('node:created', { id: 'child1' });

      // Node should be in store
      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('child1')).toBe(true);
      });
    });

    it('should handle node deleted before relationship deleted', async () => {
      // Setup: node and edge exist
      const testNode = createTestNode('child1');
      sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);
      structureTree.addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 100
      });

      // Node deleted first
      emitTauriEvent('node:deleted', { id: 'child1' });
      expect(sharedNodeStore.hasNode('child1')).toBe(false);

      // Relationship deleted second (should not crash)
      expect(() => {
        emitTauriEvent('relationship:deleted', {
          id: 'relationship:parent1:child1',
          fromId: 'parent1',
          toId: 'child1',
          relationshipType: 'has_child'
        });
      }).not.toThrow();

      await settleCoalescer();
      expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(0);
    });

    it('should handle multiple concurrent node creations', async () => {
      const node1 = createTestNode('node1');
      const node2 = createTestNode('node2');
      const node3 = createTestNode('node3');

      registerMockNode(node1);
      registerMockNode(node2);
      registerMockNode(node3);

      // Emit events rapidly
      emitTauriEvent('node:created', { id: 'node1' });
      emitTauriEvent('node:created', { id: 'node2' });
      emitTauriEvent('node:created', { id: 'node3' });

      // All nodes should be fetched and stored
      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('node1')).toBe(true);
        expect(sharedNodeStore.hasNode('node2')).toBe(true);
        expect(sharedNodeStore.hasNode('node3')).toBe(true);
      });
    });
  });

  describe('Task Node Normalization', () => {
    beforeEach(async () => {
      await initializeTauriSyncListeners();
    });

    it('should normalize task nodes with flat status field', async () => {
      const taskNode: Node = {
        id: 'task1',
        nodeType: 'task',
        content: 'Test task',
        properties: {
          status: 'open',
          priority: 'high'
        },
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1
      };

      registerMockNode(taskNode);
      emitTauriEvent('node:created', { id: 'task1' });

      await vi.waitFor(() => {
        const storedNode = sharedNodeStore.getNode('task1');
        expect(storedNode).toBeDefined();
        // After normalization, task nodes have flat status field
        interface TaskNode extends Node {
          status?: string;
        }
        expect((storedNode as TaskNode).status).toBeDefined();
      });
    });
  });
});
