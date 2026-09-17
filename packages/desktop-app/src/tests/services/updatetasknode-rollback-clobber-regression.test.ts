/**
 * Regression: `updateTaskNode()`'s failure-path catch handler unconditionally
 * called `this.nodesSet(nodeId, existingNode)` — writing this specific call's
 * OWN pre-edit snapshot (captured before ITS optimistic update was applied)
 * straight into the store on ANY write failure, OCC or not.
 *
 * Concrete failure sequence (see the issue this closes):
 * 1. Write A is executing for task node X (an in-flight, immediate-mode
 *    `updateTaskNode()` persist).
 * 2. Write B, for the same node, is submitted while A is executing —
 *    `PersistenceCoordinator.persist()`'s `isExecuting` branch collapses it
 *    into `queuedOperations`. B's own optimistic value is applied to the
 *    local store immediately (before A's RPC even settles).
 * 3. Write A's RPC rejects. Its catch handler runs `nodesSet(nodeId,
 *    existingNode)` — `existingNode` being the snapshot from BEFORE write A's
 *    own optimistic update, i.e. from before B was ever submitted. This
 *    overwrites B's optimistic value with stale content that predates it.
 * 4. Write B's real (queued, promoted) persist attempt goes on to succeed,
 *    but the damage from step 3 already discarded what the user saw.
 *
 * `updateNode()`'s sibling catch handler doesn't have this problem: it calls
 * `rollbackUpdate()`, which only rewinds bookkeeping (the pending-update
 * list, the version counter) and re-notifies with whatever the store
 * CURRENTLY holds — it never calls `nodesSet()` and never touches node
 * content. The fix makes `updateTaskNode()`'s catch handler do the same:
 * notify subscribers with the store's current value instead of force-
 * reverting to this call's own stale, pre-edit snapshot.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator
} from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import { DEBOUNCED_WRITE_WAIT_MS } from '../utils/test-constants';

const makeTaskNode = (id: string, status: string, version = 1): Node =>
  ({
    id,
    nodeType: 'task',
    content: '- [ ] seed task',
    createdAt: '2024-01-01T00:00:00.000Z',
    modifiedAt: '2024-01-01T00:00:00.000Z',
    version,
    properties: {},
    mentions: [],
    status
  }) as unknown as Node;

const dbSource = { type: 'database' as const, reason: 'initial-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'pane-1' };

describe('updateTaskNode failure-path rollback — queued-write regression', () => {
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
    "does not discard a genuinely queued second write's still-in-flight " +
      "optimistic value when the first write's non-OCC failure runs the " +
      'unconditional rollback',
    async () => {
      const nodeId = 'task-rollback-1';
      store.setNode(makeTaskNode(nodeId, 'open', 1), dbSource);

      const coord = SimplePersistenceCoordinator.getInstance();

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(async (_id, _version) => {
        updateCallCount++;
        if (updateCallCount === 1) {
          // Write A: takes long enough that write B is guaranteed to land
          // and collapse into queuedOperations while A is still executing.
          await new Promise((resolve) => setTimeout(resolve, 300));
          throw new Error('network error');
        }
        // Write B's own real (queued, promoted) persist attempt — deliberately
        // never resolves within this test. The assertion below must catch
        // the clobber from A's rollback handler while B's real write is
        // still in flight (with only its OPTIMISTIC value standing) — if B's
        // real write were allowed to complete, its own success handler's
        // Object.assign would silently re-correct the value on its own,
        // masking a merely-transient clobber rather than proving the fix.
        await new Promise(() => {
          /* never resolves */
        });
        throw new Error('unreachable');
      });

      // Write A: mode is always 'immediate' for updateTaskNode, so the
      // persist starts synchronously — no debounce wait needed.
      store.updateTaskNode(nodeId, { status: 'in_progress' }, viewerSource);
      expect(coord.isExecuting(nodeId)).toBe(true);

      // Write B: submitted while A is executing -> collapses into
      // queuedOperations. B's optimistic status lands immediately.
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);
      expect((store.getNode(nodeId) as unknown as { status: string }).status).toBe('done');

      // Let A's failure (and its rollback handler) fire, and B's queued
      // write get promoted into execution (call #2 above) — but stop well
      // before any further settlement, since call #2 never resolves.
      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));
      expect(updateCallCount).toBe(2);
      expect(coord.isExecuting(nodeId)).toBe(true);

      // B's still-in-flight optimistic value must not have been silently
      // clobbered by A's stale pre-edit snapshot reverting the node to
      // 'open'.
      const after = store.getNode(nodeId) as unknown as { status: string };
      expect(after.status).toBe('done');
      expect(after.status).not.toBe('open');
    },
    10000
  );

  it(
    'leaves the optimistic edit in place for an isolated non-OCC failure with ' +
      'nothing else queued, instead of force-reverting it (matches updateNode())',
    async () => {
      const nodeId = 'task-rollback-isolated-1';
      store.setNode(makeTaskNode(nodeId, 'open', 1), dbSource);

      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(new Error('offline'));

      store.updateTaskNode(nodeId, { status: 'in_progress' }, viewerSource);

      // Confirms the failure was actually caught and handled. (The
      // user-visible write-failure notification for this same scenario is
      // covered by occ-notification-dedup.test.ts's updateTaskNode()
      // describe block, not asserted here — this test's focus is the
      // rollback-clobber behavior.)
      await vi.waitFor(() => {
        expect(store.getTestErrors().length).toBeGreaterThan(0);
      });

      // Nothing else was queued behind this write — the optimistic value is
      // left in place (the store's own generic non-OCC behavior, matching
      // updateNode()'s), not force-reverted.
      const after = store.getNode(nodeId) as unknown as { status: string };
      expect(after.status).toBe('in_progress');
    },
    10000
  );
});
