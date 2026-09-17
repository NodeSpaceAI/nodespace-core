/**
 * AI Chats store — sidebar list load + "+ New chat" create.
 *
 * `loadAiChats` fetches ai-chat nodes and sorts them most-recently-modified
 * first (the backend query has no order-by). `createChat` mints a new chat
 * immediately (no name prompt) and prepends it to the list so it shows up
 * without a reload; `createBusy`/`createError` mirror how Collections
 * surfaces create-in-flight state.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import type { Node } from '$lib/types';

const mockQueryNodes = vi.fn<(query: unknown) => Promise<Node[]>>();
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    queryNodes: (query: unknown) => mockQueryNodes(query)
  }
}));

const mockCreateSchemaInstance = vi.fn<(typeId: string) => Promise<Node>>();
vi.mock('$lib/services/schema-authoring', () => ({
  createSchemaInstance: (typeId: string) => mockCreateSchemaInstance(typeId)
}));

import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

function makeChat(id: string, content: string, modifiedAt: string): Node {
  return {
    id,
    nodeType: 'ai-chat',
    content,
    createdAt: modifiedAt,
    modifiedAt,
    version: 1,
    properties: {}
  } as Node;
}

describe('aiChatsData', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    aiChatsData.reset();
  });

  describe('loadAiChats', () => {
    it('queries ai-chat nodes with no fetch-side limit', async () => {
      mockQueryNodes.mockResolvedValue([]);

      await aiChatsData.loadAiChats();

      // No `limit` on the query: the backend has no order-by, so a fetch-side
      // limit would ask SQLite for an arbitrary (effectively insertion-order)
      // subset with no ORDER BY, and sorting that subset afterward could not
      // recover chats the LIMIT had already excluded. The display cap is
      // applied client-side, after sorting (see the "more than DISPLAY_LIMIT"
      // test below).
      expect(mockQueryNodes).toHaveBeenCalledWith({ nodeType: 'ai-chat' });
    });

    it('caps the displayed list to DISPLAY_LIMIT after sorting, not before', async () => {
      // 52 chats returned in an arbitrary (non-recency) order — id order —
      // with only the LAST one actually being the most recent. A limit-then-
      // sort implementation would have already dropped it from the fetch;
      // this proves the real implementation sorts the full set first.
      const nodes: Node[] = [];
      for (let i = 0; i < 52; i++) {
        nodes.push(makeChat(`chat-${i}`, `Chat ${i}`, `2026-01-01T00:00:${String(i).padStart(2, '0')}.000Z`));
      }
      mockQueryNodes.mockResolvedValue(nodes);

      await aiChatsData.loadAiChats();

      expect(aiChatsData.state.chats).toHaveLength(50);
      // The most-recently-modified chat (chat-51) must survive the cap.
      expect(aiChatsData.state.chats[0].id).toBe('chat-51');
    });

    it('sorts results most-recently-modified first, regardless of fetch order', async () => {
      mockQueryNodes.mockResolvedValue([
        makeChat('old', 'Oldest', '2026-01-01T00:00:00.000Z'),
        makeChat('newest', 'Newest', '2026-03-01T00:00:00.000Z'),
        makeChat('mid', 'Middle', '2026-02-01T00:00:00.000Z')
      ]);

      await aiChatsData.loadAiChats();

      expect(aiChatsData.state.chats.map((c) => c.id)).toEqual(['newest', 'mid', 'old']);
    });

    it('carries raw content through, including empty strings', async () => {
      mockQueryNodes.mockResolvedValue([makeChat('empty', '', '2026-01-01T00:00:00.000Z')]);

      await aiChatsData.loadAiChats();

      expect(aiChatsData.state.chats[0].content).toBe('');
    });

    it('sets loading state during the fetch and clears it after', async () => {
      let resolveQuery: (nodes: Node[]) => void = () => {};
      mockQueryNodes.mockReturnValue(
        new Promise<Node[]>((resolve) => {
          resolveQuery = resolve;
        })
      );

      const loadPromise = aiChatsData.loadAiChats();
      expect(aiChatsData.state.loading).toBe(true);

      resolveQuery([]);
      await loadPromise;

      expect(aiChatsData.state.loading).toBe(false);
    });

    it('surfaces a failure without throwing, and leaves the prior list untouched', async () => {
      mockQueryNodes.mockResolvedValue([makeChat('a', 'A', '2026-01-01T00:00:00.000Z')]);
      await aiChatsData.loadAiChats();

      mockQueryNodes.mockRejectedValue(new Error('daemon unavailable'));
      await aiChatsData.loadAiChats();

      expect(aiChatsData.state.error).toContain('daemon unavailable');
      expect(aiChatsData.state.chats.map((c) => c.id)).toEqual(['a']);
    });

    // Unlike every other cross-switch read in the codebase
    // (loadChildrenForParent, doLoadChildrenTree, refreshDatabaseSettings,
    // createChat, createCollection), loadAiChats committed its fetched array
    // into state without re-checking the store generation after the await — a
    // response issued against the previous database could land after a switch
    // and get committed as if it belonged to the new one.
    describe('stale-response guard across a database switch', () => {
      it('discards a load that resolves after invalidateForDatabaseSwitch, without writing into the list', async () => {
        let resolveLoad: (nodes: Node[]) => void = () => {};
        mockQueryNodes.mockReturnValue(
          new Promise<Node[]>((resolve) => {
            resolveLoad = resolve;
          })
        );

        const loadPromise = aiChatsData.loadAiChats();

        // A database switch lands while the load is still in flight.
        aiChatsData.invalidateForDatabaseSwitch();

        resolveLoad([makeChat('stale-db-chat', 'Stale', '2026-01-01T00:00:00.000Z')]);
        await loadPromise;

        expect(aiChatsData.state.chats).toEqual([]);
        expect(aiChatsData.state.error).toBeNull();
      });

      it('a late-resolving load from the previous database does not clobber a fresh load for the new one', async () => {
        // Reproduces the exact failure scenario: DB A's loadAiChats is still in
        // flight when the user switches to DB B. B's fresh load resolves first;
        // A's late response must not then overwrite it.
        let resolveFirst: (nodes: Node[]) => void = () => {};
        mockQueryNodes.mockReturnValueOnce(
          new Promise<Node[]>((resolve) => {
            resolveFirst = resolve;
          })
        );

        const firstLoad = aiChatsData.loadAiChats(); // issued against DB A

        aiChatsData.invalidateForDatabaseSwitch(); // switch to DB B
        mockQueryNodes.mockResolvedValueOnce([
          makeChat('b-chat', 'From DB B', '2026-01-01T00:00:00.000Z')
        ]);
        const secondLoad = aiChatsData.loadAiChats(); // issued against DB B
        await secondLoad;

        expect(aiChatsData.state.chats.map((c) => c.id)).toEqual(['b-chat']);

        // DB A's stale response finally lands.
        resolveFirst([makeChat('a-chat', 'From DB A', '2026-01-01T00:00:00.000Z')]);
        await firstLoad;

        // Still DB B's data — the stale DB A response was dropped, not merged
        // or committed over it.
        expect(aiChatsData.state.chats.map((c) => c.id)).toEqual(['b-chat']);
      });

      it('discards a failure that resolves after invalidateForDatabaseSwitch, without surfacing a stale error', async () => {
        let rejectLoad: (err: Error) => void = () => {};
        mockQueryNodes.mockReturnValue(
          new Promise<Node[]>((_resolve, reject) => {
            rejectLoad = reject;
          })
        );

        const loadPromise = aiChatsData.loadAiChats();
        aiChatsData.invalidateForDatabaseSwitch();
        rejectLoad(new Error('boom in old database'));

        await loadPromise;

        expect(aiChatsData.state.error).toBeNull();
      });
    });
  });

  describe('createChat', () => {
    it('creates via createSchemaInstance and prepends the result to the list', async () => {
      mockQueryNodes.mockResolvedValue([makeChat('existing', 'Existing', '2026-01-01T00:00:00.000Z')]);
      await aiChatsData.loadAiChats();

      const created = makeChat('new-chat', '', '2026-06-01T00:00:00.000Z');
      mockCreateSchemaInstance.mockResolvedValue(created);

      const result = await aiChatsData.createChat();

      expect(mockCreateSchemaInstance).toHaveBeenCalledWith('ai-chat');
      expect(result).toEqual(created);
      expect(aiChatsData.state.chats.map((c) => c.id)).toEqual(['new-chat', 'existing']);
    });

    it('registers the new node as persisted in SharedNodeStore (regression: duplicate node-id collision)', async () => {
      // `createSchemaInstance` creates its node directly via `backendAdapter`,
      // bypassing SharedNodeStore entirely — it never lands in the store's
      // `persistedNodeIds` bookkeeping on its own. Without `createChat`
      // registering it immediately, the AI Chat viewer's first write (model
      // selection / first message) would find the id NOT marked persisted,
      // wrongly re-issue a CREATE for a row that already exists, and collide
      // with "Failed to insert node".
      // A unique id — other tests in this file also create a chat named
      // 'new-chat' and, since `sharedNodeStore` is a real, unreset singleton
      // here (unlike `aiChatsData`, which `beforeEach` does reset), reusing
      // that id would make this test's "not yet persisted" assertion
      // order-dependent on whichever test ran first.
      const created = makeChat('persisted-id-regression-chat', '', '2026-06-01T00:00:00.000Z');
      mockCreateSchemaInstance.mockResolvedValue(created);

      expect(sharedNodeStore.isNodePersisted(created.id)).toBe(false);

      await aiChatsData.createChat();

      expect(sharedNodeStore.isNodePersisted(created.id)).toBe(true);
      // `setNode`'s 'database' source normalizes the raw node (promoting
      // ai-chat's typed fields), so compare identity/content rather than
      // deep-equal the raw object `createSchemaInstance` returned.
      expect(sharedNodeStore.getNode(created.id)?.id).toBe(created.id);
      expect(sharedNodeStore.getNode(created.id)?.content).toBe(created.content);
    });

    it('sets createBusy while the create is in flight and clears it after', async () => {
      let resolveCreate: (node: Node) => void = () => {};
      mockCreateSchemaInstance.mockReturnValue(
        new Promise<Node>((resolve) => {
          resolveCreate = resolve;
        })
      );

      const createPromise = aiChatsData.createChat();
      expect(aiChatsData.createBusy).toBe(true);

      resolveCreate(makeChat('new-chat', '', '2026-06-01T00:00:00.000Z'));
      await createPromise;

      expect(aiChatsData.createBusy).toBe(false);
    });

    it('ignores a second call while a create is already in flight', async () => {
      mockCreateSchemaInstance.mockReturnValue(new Promise<Node>(() => {})); // never resolves

      const first = aiChatsData.createChat();
      const second = await aiChatsData.createChat();

      expect(second).toBeNull();
      expect(mockCreateSchemaInstance).toHaveBeenCalledOnce();
      void first; // left pending deliberately — nothing further to await
    });

    it('surfaces a failure via createError, returns null, and re-enables the button', async () => {
      mockCreateSchemaInstance.mockRejectedValue(new Error('daemon unavailable'));

      const result = await aiChatsData.createChat();

      expect(result).toBeNull();
      expect(aiChatsData.createError).toContain('daemon unavailable');
      expect(aiChatsData.createBusy).toBe(false);
      expect(aiChatsData.state.chats).toEqual([]);
    });

    it('clears a previous createError at the start of the next attempt', async () => {
      mockCreateSchemaInstance.mockRejectedValueOnce(new Error('boom'));
      await aiChatsData.createChat();
      expect(aiChatsData.createError).toContain('boom');

      mockCreateSchemaInstance.mockResolvedValueOnce(
        makeChat('new-chat', '', '2026-06-01T00:00:00.000Z')
      );
      const result = await aiChatsData.createChat();

      expect(result).not.toBeNull();
      expect(aiChatsData.createError).toBe('');
    });

    it('discards a create that resolves after invalidateForDatabaseSwitch, without writing into the list', async () => {
      let resolveCreate: (node: Node) => void = () => {};
      mockCreateSchemaInstance.mockReturnValue(
        new Promise<Node>((resolve) => {
          resolveCreate = resolve;
        })
      );

      const createPromise = aiChatsData.createChat();

      // A database switch lands while the create is still in flight.
      aiChatsData.invalidateForDatabaseSwitch();

      resolveCreate(makeChat('stale-db-chat', '', '2026-06-01T00:00:00.000Z'));
      const result = await createPromise;

      expect(result).toBeNull();
      expect(aiChatsData.state.chats).toEqual([]);
      // The node genuinely exists (in the database that was left) — this is
      // a discard, not a failure, so no error should be surfaced either.
      expect(aiChatsData.createError).toBe('');
    });

    it('discards a create that FAILS after invalidateForDatabaseSwitch, without surfacing its error', async () => {
      let rejectCreate: (err: Error) => void = () => {};
      mockCreateSchemaInstance.mockReturnValue(
        new Promise<Node>((_resolve, reject) => {
          rejectCreate = reject;
        })
      );

      const createPromise = aiChatsData.createChat();
      aiChatsData.invalidateForDatabaseSwitch();
      rejectCreate(new Error('boom in old database'));

      const result = await createPromise;

      expect(result).toBeNull();
      // The failure belonged to a database this store no longer represents —
      // must not bleed its error into the newly-active database's banner.
      expect(aiChatsData.createError).toBe('');
    });
  });

  describe('invalidateForDatabaseSwitch', () => {
    it('clears a stale createError left over from the previous database', async () => {
      mockCreateSchemaInstance.mockRejectedValue(new Error('duplicate in old database'));
      await aiChatsData.createChat();
      expect(aiChatsData.createError).toContain('duplicate in old database');

      aiChatsData.invalidateForDatabaseSwitch();

      expect(aiChatsData.createError).toBe('');
    });
  });

  describe('updateChatContent', () => {
    it('patches the matching chat in place, leaving its position and other fields untouched', async () => {
      mockQueryNodes.mockResolvedValue([
        makeChat('a', 'Chat A', '2026-01-02T00:00:00.000Z'),
        makeChat('b', 'Chat B', '2026-01-01T00:00:00.000Z')
      ]);
      await aiChatsData.loadAiChats();

      aiChatsData.updateChatContent('b', 'Renamed chat B');

      expect(aiChatsData.state.chats).toEqual([
        { id: 'a', content: 'Chat A', modifiedAt: '2026-01-02T00:00:00.000Z' },
        { id: 'b', content: 'Renamed chat B', modifiedAt: '2026-01-01T00:00:00.000Z' }
      ]);
    });

    it('is a no-op when the id is not in the loaded list', async () => {
      const original = [makeChat('a', 'Chat A', '2026-01-01T00:00:00.000Z')];
      mockQueryNodes.mockResolvedValue(original);
      await aiChatsData.loadAiChats();

      aiChatsData.updateChatContent('missing-id', 'Should not appear');

      expect(aiChatsData.state.chats).toEqual([
        { id: 'a', content: 'Chat A', modifiedAt: '2026-01-01T00:00:00.000Z' }
      ]);
    });
  });

  describe('reset', () => {
    it('clears chats, createBusy, and createError back to initial state', async () => {
      mockQueryNodes.mockResolvedValue([makeChat('a', 'A', '2026-01-01T00:00:00.000Z')]);
      await aiChatsData.loadAiChats();
      mockCreateSchemaInstance.mockRejectedValue(new Error('boom'));
      await aiChatsData.createChat();

      aiChatsData.reset();

      expect(aiChatsData.state.chats).toEqual([]);
      expect(aiChatsData.state.error).toBeNull();
      expect(aiChatsData.createBusy).toBe(false);
      expect(aiChatsData.createError).toBe('');
    });

    // reset() bumps #generation the
    // same way collectionsData.reset() does, so an in-flight load/create
    // discovered stale by a reset() (not just invalidateForDatabaseSwitch())
    // cannot write into the state the reset just established.
    it('discards a load that resolves after reset, without writing into the list', async () => {
      let resolveLoad: (nodes: Node[]) => void = () => {};
      mockQueryNodes.mockReturnValue(
        new Promise<Node[]>((resolve) => {
          resolveLoad = resolve;
        })
      );

      const loadPromise = aiChatsData.loadAiChats();
      aiChatsData.reset();

      resolveLoad([makeChat('stale', 'Stale', '2026-01-01T00:00:00.000Z')]);
      await loadPromise;

      expect(aiChatsData.state.chats).toEqual([]);
    });

    it('discards a create that resolves after reset, without writing into the list', async () => {
      let resolveCreate: (node: Node) => void = () => {};
      mockCreateSchemaInstance.mockReturnValue(
        new Promise<Node>((resolve) => {
          resolveCreate = resolve;
        })
      );

      const createPromise = aiChatsData.createChat();
      aiChatsData.reset();

      resolveCreate(makeChat('stale-chat', '', '2026-06-01T00:00:00.000Z'));
      const result = await createPromise;

      expect(result).toBeNull();
      expect(aiChatsData.state.chats).toEqual([]);
    });
  });
});
