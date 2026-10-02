/**
 * SharedNodeStore - reconnect staleness
 *
 * The desktop app's WatchNodes bridge (`watcher.rs`) reconnects with backoff
 * on any daemon disruption (crash, restart, transient h2 error) but opens a
 * fresh live-forward stream with no catch-up replay: `node:created` /
 * `node:updated` / `node:deleted` events that would have landed during the
 * outage are gone, not delayed. A node already cached before the outage was
 * never told it might be missing an update, so `ensureNode`'s cache-first
 * check trusted mere presence forever — the observed symptom was an
 * AI chat conversation with a full, intact multi-turn history in the
 * database rendering as empty in the UI after navigating away and back
 * across a daemon restart, because the frontend never re-confirmed a cache
 * entry it had already hydrated before the outage.
 *
 * These tests lock in the fix at the store layer: `markPossiblyStaleAfterReconnect`
 * (invoked once per daemon reconnect — see the module-level `onDaemonReconnect`
 * wiring in shared-node-store.svelte.ts) marks every already-cached node as
 * possibly-stale without evicting it, and `ensureNode` re-confirms a
 * possibly-stale node against the backend instead of returning the cached
 * value outright — modeling the concrete "reconnect, then navigate back to
 * an already-open node" repro end-to-end through the real `setNode`
 * write path (so the existing OCC / skip-while-editing guards apply exactly
 * as they do for any other database-sourced update).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

describe('SharedNodeStore - reconnect staleness', () => {
  let store: SharedNodeStore;

  const databaseSource: UpdateSource = { type: 'database', reason: 'ensure-node' };

  // `content` stands in for what the node holds: the tests assert which copy of
  // the chat the store ends up with after a reconnect.
  const makeChatNode = (id: string, content: string, version = 1): Node => ({
    lifecycleStatus: 'active',
    id,
    nodeType: 'ai-chat-native',
    content,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version,
    properties: {},
    mentions: []
  });

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  it('a node fetched before any reconnect is not possibly-stale', () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);
    expect(store.isPossiblyStale('chat-1')).toBe(false);
  });

  it('an uncached node id is never reported possibly-stale', () => {
    store.markPossiblyStaleAfterReconnect();
    expect(store.isPossiblyStale('never-seen')).toBe(false);
  });

  it('marks an already-cached node possibly-stale on reconnect, without evicting it', () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);

    store.markPossiblyStaleAfterReconnect();

    // Not evicted — getNode still returns the last-known content so an open
    // viewer keeps rendering it with no flicker while a refresh is pending.
    expect(store.getNode('chat-1')?.content).toBe('cached');
    expect(store.isPossiblyStale('chat-1')).toBe(true);
  });

  it('a node cached AFTER the reconnect is not retroactively marked stale', () => {
    store.markPossiblyStaleAfterReconnect();
    store.setNode(makeChatNode('chat-2', 'later'), databaseSource);

    expect(store.isPossiblyStale('chat-2')).toBe(false);
  });

  it(
    'ensureNode re-confirms a possibly-stale node against the backend and applies the ' +
      'refreshed content (the reconnect-then-navigate-back repro)',
    async () => {
      // Before the outage: the conversation is cached with its full history.
      store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);
      expect(store.getNode('chat-1')?.content).toBe('cached');

      // Outage + reconnect: WatchNodes dropped whatever happened while it was
      // down (in the real repro, nothing more was appended — the point is the
      // frontend can no longer prove that without asking).
      store.markPossiblyStaleAfterReconnect();
      expect(store.isPossiblyStale('chat-1')).toBe(true);

      // Backend is asked again — DB content was intact all along; this was
      // never a persistence bug.
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeChatNode('chat-1', 'refreshed', 2));

      const result = await store.ensureNode('chat-1');

      expect(backendAdapter.getNode).toHaveBeenCalledWith('chat-1');
      expect(result?.content).toBe('refreshed');
      // The re-confirm is itself a write, so the entry is fresh again.
      expect(store.isPossiblyStale('chat-1')).toBe(false);
    }
  );

  it('ensureNode does not re-fetch a cached node that is not possibly-stale', async () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);
    vi.spyOn(backendAdapter, 'getNode');

    const result = await store.ensureNode('chat-1');

    expect(backendAdapter.getNode).not.toHaveBeenCalled();
    expect(result?.content).toBe('cached');
  });

  it('a failed re-confirm leaves the node possibly-stale for the next attempt, without throwing away the cached copy', async () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);
    store.markPossiblyStaleAfterReconnect();

    vi.spyOn(backendAdapter, 'getNode').mockRejectedValue(new Error('daemon still unreachable'));

    await expect(store.ensureNode('chat-1')).rejects.toThrow('daemon still unreachable');

    // The stale cached copy is untouched — still visible, still flagged for retry.
    expect(store.getNode('chat-1')?.content).toBe('cached');
    expect(store.isPossiblyStale('chat-1')).toBe(true);
  });

  it('concurrent ensureNode calls for the same possibly-stale node de-dupe to one fetch', async () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);
    store.markPossiblyStaleAfterReconnect();

    let resolveGetNode: (node: Node) => void = () => {};
    const pending = new Promise<Node>((resolve) => {
      resolveGetNode = resolve;
    });
    vi.spyOn(backendAdapter, 'getNode').mockReturnValue(pending);

    const first = store.ensureNode('chat-1');
    const second = store.ensureNode('chat-1');

    resolveGetNode(makeChatNode('chat-1', 'refreshed', 2));
    await Promise.all([first, second]);

    expect(backendAdapter.getNode).toHaveBeenCalledTimes(1);
  });

  it('a second reconnect after a successful refresh flags the node stale again', async () => {
    store.setNode(makeChatNode('chat-1', 'cached'), databaseSource);

    store.markPossiblyStaleAfterReconnect();
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeChatNode('chat-1', 'refreshed', 2));
    await store.ensureNode('chat-1');
    expect(store.isPossiblyStale('chat-1')).toBe(false);

    store.markPossiblyStaleAfterReconnect();
    expect(store.isPossiblyStale('chat-1')).toBe(true);
  });
});
