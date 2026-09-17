/**
 * Regression coverage for the `createSchemaInstance()` / `SharedNodeStore`
 * duplicate-node-id collision.
 *
 * `createSchemaInstance()` (`schema-authoring.ts`) creates its node directly
 * via `backendAdapter.createNode()`, bypassing `SharedNodeStore` entirely —
 * it never lands in the store's `persistedNodeIds` bookkeeping set. If a
 * caller then feeds that same id into `SharedNodeStore.setNode()` (e.g. the
 * AI Chat viewer's first write once its tab opens) without first registering
 * it as persisted, `setNode()`'s `isNewNode = !this.persistedNodeIds.has(node.id)`
 * wrongly evaluates `true`, so the store treats the already-existing node as
 * brand new and re-issues a CREATE for an id that already has a row —
 * colliding with it (`Failed to insert node`).
 *
 * The fix: callers of `createSchemaInstance()` must register the new id as
 * persisted immediately — the cheapest way being a `setNode(created, {type:
 * 'database', reason: '...'})` call right after creation, which hydrates the
 * store AND marks it persisted in one step (see `ai-chats.svelte.ts`'s
 * `createChat()`, and `query-node-viewer.svelte`'s `handleCreateInstance`,
 * which already did this).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore, SimplePersistenceCoordinator } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { createSchemaInstance } from '$lib/services/schema-authoring';
import type { Node } from '$lib/types';

function aiChatNode(id: string, overrides: Partial<Node> = {}): Node {
  return {
    id,
    nodeType: 'ai-chat',
    content: 'Untitled',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    ...overrides
  } as Node;
}

describe('createSchemaInstance + SharedNodeStore.setNode() — persisted-id regression', () => {
  let store: SharedNodeStore;

  function mockBackend() {
    const createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async (input) => (input as Node).id);
    const getNodeSpy = vi
      .spyOn(backendAdapter, 'getNode')
      .mockImplementation(async (id: string) => aiChatNode(id));
    return { createNodeSpy, getNodeSpy };
  }

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  it('does not re-create an id createSchemaInstance already persisted, once the caller registers it', async () => {
    const { createNodeSpy, getNodeSpy } = mockBackend();

    // 1. Mint the node exactly as `createSchemaInstance` does — a direct
    //    backend create, entirely bypassing the store.
    const created = await createSchemaInstance('ai-chat');
    expect(createNodeSpy).toHaveBeenCalledTimes(1);
    // `createSchemaInstance` hydrates its own return value with a second
    // round-trip GET, entirely separate from the store.
    expect(getNodeSpy).toHaveBeenCalledWith(created.id);

    // 2. The fix: the caller registers the new id as persisted immediately,
    //    mirroring `ai-chats.svelte.ts`'s `createChat()`.
    store.setNode(created, { type: 'database', reason: 'ai-chat-created' });
    expect(store.isNodePersisted(created.id)).toBe(true);

    // 3. Later, a write reaches `setNode()` for the SAME id — the AI Chat
    //    viewer's first write once the tab opens (model selection / first
    //    message hydration echo). This must not attempt a second CREATE for
    //    an id the daemon already has a row for.
    store.setNode(
      aiChatNode(created.id, { content: 'updated by viewer' }),
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
    );

    // Drain any queued persistence microtasks.
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(createNodeSpy).toHaveBeenCalledTimes(1);
  });

  it('[proves the guard above is not vacuous] without registration, the same sequence DOES re-create the id', async () => {
    const { createNodeSpy } = mockBackend();

    // Same creation step as above, but the caller forgets (or never had) the
    // registration step — reproducing the pre-fix `ai-chats.svelte.ts`
    // behavior, which called `createSchemaInstance` and nothing else.
    const created = await createSchemaInstance('ai-chat');
    expect(createNodeSpy).toHaveBeenCalledTimes(1);
    expect(store.isNodePersisted(created.id)).toBe(false);

    store.setNode(
      aiChatNode(created.id, { content: 'updated by viewer' }),
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
    );

    // `isNewNode` wrongly reads `true` for an id the backend already has a
    // row for, so a second CREATE is issued for the same id — the exact
    // collision that produced "Failed to insert node" in production.
    await vi.waitFor(() => {
      expect(createNodeSpy).toHaveBeenCalledTimes(2);
    });
    expect(createNodeSpy.mock.calls[1][0]).toEqual(
      expect.objectContaining({ id: created.id })
    );
  });
});
