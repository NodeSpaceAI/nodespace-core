/**
 * Regression tests: edit persistence amplifies writes and
 * self-conflicts during fast typing.
 *
 * Covers the SimplePersistenceCoordinator's per-node serial writer: a
 * keystroke arriving while an UpdateNode RPC is in flight must collapse into
 * a single latest-wins write that runs only after the in-flight write's
 * version confirmation lands, never re-fired per RPC round-trip and never
 * OCC-conflicting against the same client's own prior write.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import {
  CASCADE_SETTLE_TIMEOUT_MS,
  DEBOUNCED_WRITE_WAIT_MS,
  FLUSH_PENDING_TIMEOUT_MS,
  PERSISTENCE_DEBOUNCE_MS
} from '../utils/test-constants';

const makeNode = (id: string, content: string, version = 1): Node => ({
  id,
  nodeType: 'text',
  content,
  createdAt: '2024-01-01T00:00:00.000Z',
  modifiedAt: '2024-01-01T00:00:00.000Z',
  version,
  properties: {},
  mentions: []
});

const dbSource = { type: 'database' as const, reason: 'initial-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'pane-1' };

describe('Persistence serial writer regression', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  it('a keystroke arriving mid-RPC collapses into a single follow-up write, not a re-fire per round-trip', async () => {
    const nodeId = 'serial-writer-1';
    const initialNode = makeNode(nodeId, 'a', 1);

    let callCount = 0;
    // Each RPC takes a tick to resolve, simulating an in-flight round-trip.
    // This latency must STAY: it is what keeps the first write observably
    // executing while the keystroke burst below is submitted.
    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, version, node) => {
      callCount++;
      await new Promise((resolve) => setTimeout(resolve, 300));
      return {
        id: nodeId,
        nodeType: 'text',
        content: String(node.content ?? ''),
        createdAt: '2024-01-01T00:00:00.000Z',
        modifiedAt: new Date().toISOString(),
        version: version + 1,
        properties: {},
        mentions: []
      };
    });

    store.setNode(initialNode, dbSource);

    // First edit: a content-only change, so it takes the debounced path.
    // Forcing it into flight via an immediate-mode structural update would not
    // be representative, so let the real debounce fire and then queue further
    // edits while the RPC for the first is in flight.
    store.updateNode(nodeId, { content: 'ab' }, viewerSource);

    // Wait for the debounce to fire and the first RPC to actually be in
    // flight, rather than for a duration assumed to cover the debounce. This
    // is false until the debounced write starts executing, so it is a real
    // wait; the mocked RPC's 300ms latency keeps it in flight afterwards.
    await vi.waitFor(() => expect(store.isNodePersistenceExecuting(nodeId)).toBe(true), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    // A burst of further keystrokes arrives while the RPC is in flight.
    store.updateNode(nodeId, { content: 'abc' }, viewerSource);
    store.updateNode(nodeId, { content: 'abcd' }, viewerSource);
    store.updateNode(nodeId, { content: 'abcde' }, viewerSource);

    await store.flushAllPendingSaves(CASCADE_SETTLE_TIMEOUT_MS);

    // The in-flight write plus the collapsed latest-wins follow-up write is
    // exactly two RPCs — not one per queued keystroke.
    expect(callCount).toBe(2);
    const lastCall = updateSpy.mock.calls[updateSpy.mock.calls.length - 1];
    expect(lastCall[2].content).toBe('abcde');

    const stored = store.getNode(nodeId);
    expect(stored?.content).toBe('abcde');
    expect(stored?.version).toBe(3);
  }, 10000);

  it('the follow-up write always reads the version confirmed by the prior write — no self-conflict', async () => {
    const nodeId = 'serial-writer-2';
    const initialNode = makeNode(nodeId, 'x', 5);

    const versionsSeen: number[] = [];
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, version, node) => {
      versionsSeen.push(version);
      await new Promise((resolve) => setTimeout(resolve, 300));
      return {
        id: nodeId,
        nodeType: 'text',
        content: String(node.content ?? ''),
        createdAt: '2024-01-01T00:00:00.000Z',
        modifiedAt: new Date().toISOString(),
        version: version + 1,
        properties: {},
        mentions: []
      };
    });

    store.setNode(initialNode, dbSource);

    store.updateNode(nodeId, { content: 'xy' }, viewerSource);

    // Wait for the first RPC to actually be in flight (false until the
    // debounced write starts executing) rather than sleeping past the
    // debounce; the mock's 300ms latency keeps it in flight for the
    // second edit below.
    await vi.waitFor(() => expect(store.isNodePersistenceExecuting(nodeId)).toBe(true), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    // Second edit queued while first RPC (version 5) is in flight.
    store.updateNode(nodeId, { content: 'xyz' }, viewerSource);

    await store.flushAllPendingSaves(CASCADE_SETTLE_TIMEOUT_MS);

    // The second RPC must carry the version the first RPC's response
    // confirmed (6), not the stale version read before confirmation (5).
    expect(versionsSeen).toEqual([5, 6]);

    // No OCC conflict notification should have been raised.
    const versionMismatches = conflictNotifications.notifications.filter(
      (n) => n.conflictType === 'version-mismatch'
    );
    expect(versionMismatches).toHaveLength(0);
  }, 10000);

  it('an OCC conflict on the in-flight write settles the collapsed follow-up write instead of hanging it', async () => {
    const nodeId = 'serial-writer-3';
    const initialNode = makeNode(nodeId, 'a', 1);

    // This mock latency must STAY: the test needs the first RPC to still be
    // in flight when the second edit's own debounce fires, so it has to
    // outlive PERSISTENCE_DEBOUNCE_MS. Derived from that constant rather than
    // a hand-picked round number — it is not a guess at machine speed.
    const occRpcLatencyMs = PERSISTENCE_DEBOUNCE_MS * 2;
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, _version, node) => {
      await new Promise((resolve) => setTimeout(resolve, occRpcLatencyMs));
      const occError = new Error('VERSION_CONFLICT: optimistic concurrency failure') as Error & {
        code: string;
        conflictData: {
          node_id: string;
          expected: number;
          actual: number;
          current_node: Node | null;
        };
      };
      occError.code = 'VERSION_CONFLICT';
      occError.conflictData = {
        node_id: nodeId,
        expected: 1,
        actual: 2,
        current_node: { ...initialNode, content: String(node.content ?? ''), version: 2 }
      };
      throw occError;
    });

    store.setNode(initialNode, dbSource);

    // First edit starts the in-flight write that will hit an OCC conflict.
    store.updateNode(nodeId, { content: 'ab' }, viewerSource);

    // Wait for that write to actually be in flight (false until the debounce
    // fires) instead of sleeping past the debounce window.
    await vi.waitFor(() => expect(store.isNodePersistenceExecuting(nodeId)).toBe(true), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    // A second edit arrives and collapses into the latest-wins queued write
    // behind the doomed in-flight write, well BEFORE its OCC rejection lands.
    store.updateNode(nodeId, { content: 'abc' }, viewerSource);
    expect(store.hasPendingSave(nodeId)).toBe(true);

    // Must not hang: flushAllPendingSaves races each node's promise
    // against its OWN 5s internal timeout. Without the fix, the collapsed
    // queued write's promise never settles, so this call only "succeeds"
    // by burning the full internal timeout — asserting on elapsed time
    // catches that even though the call eventually resolves either way.
    const flushStart = performance.now();
    const failed = await store.flushAllPendingSaves(FLUSH_PENDING_TIMEOUT_MS);
    const flushDuration = performance.now() - flushStart;

    // A correctly-settled promise resolves once the in-flight RPC's own
    // latency elapses — bounded by that latency plus settle margin, and
    // nowhere near the internal flush timeout a hung promise would burn.
    expect(flushDuration).toBeLessThan(occRpcLatencyMs + DEBOUNCED_WRITE_WAIT_MS);

    // The queued write must be reported as settled (rejected, since it was
    // cancelled), not silently forgotten.
    expect(failed.has(nodeId)).toBe(true);

    // hasPendingSave must clear once the OCC conflict is handled — a stuck
    // `true` here would mean database broadcasts are permanently skipped for
    // this node (see setNode's skip-while-editing guard).
    expect(store.hasPendingSave(nodeId)).toBe(false);
  }, 10000);
});
