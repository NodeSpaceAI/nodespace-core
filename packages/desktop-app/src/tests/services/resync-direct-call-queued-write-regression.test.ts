/**
 * Regression: `resyncNodeFromServer`'s DIRECT call (invoked from inside a
 * failing write's own OCC-conflict catch handler, not the queued-follow-up
 * recursion) hardcoded `hasPending: false`, so it had no way to detect a
 * second, genuinely different write that arrived and got collapsed into
 * `PersistenceCoordinator`'s `queuedOperations` while the first write was
 * still executing.
 *
 * Concrete failure sequence (see the issue this closes):
 * 1. Write A is executing for node X (an in-flight `persist()` RPC).
 * 2. Write B, for the same node, is submitted while A is executing — it
 *    collapses into the single-slot `queuedOperations` map. B's optimistic
 *    value is already applied to the local store at this point.
 * 3. Write A fails with an OCC conflict whose response does NOT embed
 *    `current_node` — `updateNode()`'s OCC handling falls back to
 *    `resyncNodeFromServer(nodeId)`, the DIRECT call.
 * 4. The node is not focused, so with `hasPending` hardcoded `false`,
 *    `decideRemoteUpdate` applies the fetched (stale, pre-B) server row,
 *    silently discarding B's still-pending optimistic value.
 *
 * Empirically confirmed (see the PR this test ships with) that a naive swap
 * to a LIVE `PersistenceCoordinator.hasPending(nodeId)` read at the decision
 * point does NOT fix this correctly: `clearQueued(nodeId)` — called
 * unconditionally by both OCC handlers immediately before either hydration
 * branch runs, to stop the FAILING write's own queued retry from firing
 * against its now-stale version — already removes B from `queuedOperations`
 * (and its `pendingOperations` placeholder) before the resync's decision
 * point is ever reached. At that point `hasPending()` would read `true`
 * anyway, but ONLY because the failing write's own `executingOperations`
 * entry hasn't cleared yet (self-referential — confirmed true even in an
 * isolated failure with nothing else queued at all, which is exactly the
 * false-positive #2066's review caught when this was first tried).
 *
 * The fix: capture `PersistenceCoordinator.isQueued(nodeId)` in the OCC
 * handler BEFORE calling `clearQueued()`, and thread that captured boolean
 * into `resyncNodeFromServer` as `directCallHadQueuedWrite` for the direct
 * call to use instead of a hardcoded `false` (or a live, self-referential
 * `hasPending()` read).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator
} from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import { DEBOUNCED_WRITE_WAIT_MS, CASCADE_SETTLE_TIMEOUT_MS } from '../utils/test-constants';

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

/** A VERSION_CONFLICT error with NO embedded current_node — forces the
 * fallback (resyncNodeFromServer) path rather than #2068's direct-hydration
 * path, which is what this issue is about. */
const makeVersionConflictErrorNoCurrentNode = (nodeId: string) => ({
  message: `Version conflict on ${nodeId}`,
  code: 'VERSION_CONFLICT' as const,
  details: 'Aborted',
  conflictData: {
    node_id: nodeId,
    expected: 1,
    actual: 2,
    current_node: null
  }
});

describe('resyncNodeFromServer direct call — queued-write regression', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  it(
    'does not discard a genuinely queued second write when the first write\'s OCC ' +
      'conflict falls back to resyncNodeFromServer',
    async () => {
      const nodeId = 'queued-write-1';
      store.setNode(makeNode(nodeId, 'seed', 1), dbSource);

      const coord = SimplePersistenceCoordinator.getInstance();

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, version, node) => {
        updateCallCount++;
        if (updateCallCount === 1) {
          // Write A: takes long enough that write B is guaranteed to land
          // and collapse into queuedOperations while A is still executing.
          await new Promise((resolve) => setTimeout(resolve, 300));
          throw makeVersionConflictErrorNoCurrentNode(nodeId);
        }
        // Write B's own real (queued) persist attempt, once promoted —
        // succeeds normally against the corrected version.
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

      // The fetch resync performs after A's OCC failure — reflects server
      // state from BEFORE B was ever attempted (the server has no idea B
      // exists yet).
      const getNodeSpy = vi
        .spyOn(backendAdapter, 'getNode')
        .mockResolvedValue(makeNode(nodeId, 'server-state-before-B', 2));

      // Write A: content-only change -> debounced.
      store.updateNode(nodeId, { content: 'A-edit' }, viewerSource);

      // Wait past the debounce so A's RPC starts (still in flight — the
      // mocked RPC above takes 300ms).
      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));
      expect(coord.isExecuting(nodeId)).toBe(true);

      // Write B: submitted while A is executing -> collapses into
      // queuedOperations. B's optimistic content lands immediately.
      store.updateNode(nodeId, { content: 'B-edit-still-queued' }, viewerSource);
      expect(store.getNode(nodeId)?.content).toBe('B-edit-still-queued');

      // Let A's OCC failure and the fallback resync settle. The resync's fetch
      // is the terminal event of that cascade and the decision point this test
      // is about, so waiting on it returns as soon as the sequence under test
      // has run rather than after a fixed 2s.
      //
      // Note B does NOT produce a second updateNode call: the OCC handler's
      // clearQueued() cancels B's queued retry (see the file header), so only
      // B's optimistic local value is at stake here — which is the point.
      await vi.waitFor(
        () => {
          expect(getNodeSpy).toHaveBeenCalled();
          // The write must also have finished failing, so the resync's
          // apply-or-skip decision has actually run by the time we assert.
          // Without this the wait could return between the fetch and the
          // decision, and the regression would stop being detectable.
          expect(coord.isExecuting(nodeId)).toBe(false);
        },
        { timeout: CASCADE_SETTLE_TIMEOUT_MS }
      );

      // B's optimistic value must not have been silently clobbered by the
      // resync's stale (pre-B) server snapshot.
      const after = store.getNode(nodeId);
      expect(after?.content).toBe('B-edit-still-queued');
    },
    10000
  );

  it(
    'still applies the resync normally for an isolated OCC failure with nothing ' +
      'else queued (no false-positive skip — regression check for the #2066 round-1 finding)',
    async () => {
      const nodeId = 'isolated-failure-1';
      store.setNode(makeNode(nodeId, 'seed', 1), dbSource);

      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        throw makeVersionConflictErrorNoCurrentNode(nodeId);
      });
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode(nodeId, 'server-fresh', 5));

      store.updateNode(nodeId, { content: 'A-edit-isolated' }, viewerSource);

      // Nothing else was queued behind A — the resync must apply normally,
      // not be defeated by a self-referential false "something is pending".
      // Waiting on the resynced value IS the assertion, so this settles as
      // soon as the behaviour under test happens.
      await vi.waitFor(() => expect(store.getNode(nodeId)?.content).toBe('server-fresh'), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      // The server row must land whole — content AND version.
      expect(store.getNode(nodeId)?.version).toBe(5);
    },
    10000
  );
});
