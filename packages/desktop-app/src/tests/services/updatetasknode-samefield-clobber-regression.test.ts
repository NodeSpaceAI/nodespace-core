/**
 * Regression for the SAME-field concurrent-write race left open after the
 * fix in `updatetasknode-success-clobber-regression.test.ts` (which only
 * closed the DIFFERENT-field case).
 *
 * Concrete failure sequence this test reproduces (pre-fix):
 * 1. Write A changes `status` for node X. Its optimistic apply lands, then
 *    its persist() RPC is sent and is still in flight.
 * 2. Write B, for the SAME node, ALSO changes `status` (a different value)
 *    while A is still executing. `PersistenceCoordinator.persist()`'s
 *    `isExecuting` branch collapses B into `queuedOperations` — a real RPC
 *    for B is not sent yet. B's own optimistic `status` DOES land in the
 *    store immediately, synchronously, at call time, overwriting A's
 *    optimistic value.
 * 3. Write A's RPC resolves successfully. Its response's `status` reflects
 *    the server's view as of A's OWN request — which never included B's
 *    change, since B hasn't been sent yet.
 * 4. Even with the DIFFERENT-field fix in place, A's success handler still
 *    applies `status` from its own response, because A's own `update` DID
 *    specify `status` — field-scoping alone can't tell that a NEWER write
 *    for that exact same field has since landed. This transiently reverts
 *    the store to A's stale status, clobbering B's optimistic value, until
 *    B's own real (queued, promoted) write later resolves and re-applies
 *    the correct final value.
 *
 * This is self-correcting (B's own write always resolves the value
 * eventually) but produces a real, if brief, wrong-value window. Fixed by
 * a per-node-per-field write-sequence counter (`bumpTypedFieldSeq` /
 * `getTypedFieldSeq`): each write captures the sequence number for every
 * field it touches at optimistic-apply time, and only applies that field
 * from its own response if the field's sequence hasn't moved on since —
 * i.e. no newer same-field write raced in ahead of this write's response.
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

type TaskLikeNode = Node & {
  status: string;
  priority?: string;
};

const makeTaskNode = (id: string, status: string, priority: string, version = 1): TaskLikeNode =>
  ({
    id,
    nodeType: 'task',
    content: '- [ ] seed task',
    createdAt: '2024-01-01T00:00:00.000Z',
    modifiedAt: '2024-01-01T00:00:00.000Z',
    version,
    properties: {},
    mentions: [],
    status,
    priority
  }) as unknown as TaskLikeNode;

const dbSource = { type: 'database' as const, reason: 'initial-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'pane-1' };

describe('updateTaskNode success-path clobber — same-field concurrent-write regression', () => {
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
    "does not let write A's successful RPC response transiently revert " +
      "write B's still-in-flight optimistic value for the SAME field " +
      '(status)',
    async () => {
      const nodeId = 'samefield-clobber-1';
      store.setNode(makeTaskNode(nodeId, 'open', 'low', 1), dbSource);

      const coord = SimplePersistenceCoordinator.getInstance();

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(
        async (_id, version, _updateArg) => {
          updateCallCount++;
          if (updateCallCount === 1) {
            // Write A: takes long enough that write B is guaranteed to land
            // and collapse into queuedOperations while A is still executing.
            await new Promise((resolve) => setTimeout(resolve, 300));
            // A's response reflects the server's view of A's OWN request:
            // status changed to 'in-progress' (A's edit). It does NOT know
            // about B's later 'done' change.
            return {
              id: nodeId,
              nodeType: 'task' as const,
              content: '- [ ] seed task',
              createdAt: '2024-01-01T00:00:00.000Z',
              modifiedAt: new Date().toISOString(),
              version: version + 1,
              status: 'in-progress',
              priority: 'low',
              dueDate: undefined,
              startedAt: undefined,
              completedAt: undefined
            };
          }
          // Write B's own real (queued, promoted) persist attempt —
          // deliberately never resolves within this test. The assertion
          // below must catch a transient clobber from A's success handler
          // while B's real write is still in flight (only its OPTIMISTIC
          // value standing) — if B's real write were allowed to complete,
          // its own success handler would silently re-correct the value on
          // its own, masking a merely-transient clobber rather than proving
          // the fix.
          await new Promise(() => {
            /* never resolves */
          });
          throw new Error('unreachable');
        }
      );

      // Write A: mode is always 'immediate' for updateTaskNode, so the
      // persist starts synchronously — no debounce wait needed.
      store.updateTaskNode(nodeId, { status: 'in-progress' }, viewerSource);
      expect(coord.isExecuting(nodeId)).toBe(true);

      // Write B: submitted while A is executing -> collapses into
      // queuedOperations. B's optimistic status ('done') lands immediately,
      // overwriting A's optimistic 'in-progress'.
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);
      const afterB = store.getNode(nodeId) as unknown as TaskLikeNode;
      expect(afterB.status).toBe('done');

      // Let A's success handler fire, and B's queued write get promoted into
      // execution (call #2 above). Waiting on that promotion is exact; a fixed
      // duration would only be a guess that it has happened by now, and call #2
      // never resolves so there is nothing further to settle.
      await vi.waitFor(() => expect(updateCallCount).toBe(2), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      // B's still-in-flight optimistic status must survive A's success
      // handler — A's response's status is stale (pre-dates B's change) and
      // must not be applied now that a newer same-field write has landed.
      expect(after.status).toBe('done');
    },
    10000
  );

  it(
    "still converges to write B's correct final status once B's own real " +
      'persist attempt completes (regression check: the fix does not break ' +
      'normal eventual consistency, and is genuinely self-correcting)',
    async () => {
      const nodeId = 'samefield-clobber-2';
      store.setNode(makeTaskNode(nodeId, 'open', 'low', 1), dbSource);

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(
        async (_id, version, updateArg) => {
          updateCallCount++;
          if (updateCallCount === 1) {
            await new Promise((resolve) => setTimeout(resolve, 300));
            return {
              id: nodeId,
              nodeType: 'task' as const,
              content: '- [ ] seed task',
              createdAt: '2024-01-01T00:00:00.000Z',
              modifiedAt: new Date().toISOString(),
              version: version + 1,
              status: 'in-progress',
              priority: 'low',
              dueDate: undefined,
              startedAt: undefined,
              completedAt: undefined
            };
          }
          // Write B's real persist, once promoted: succeeds normally,
          // reflecting the server's merged state (B's own final status).
          return {
            id: nodeId,
            nodeType: 'task' as const,
            content: '- [ ] seed task',
            createdAt: '2024-01-01T00:00:00.000Z',
            modifiedAt: new Date().toISOString(),
            version: version + 1,
            status: updateArg.status ?? 'open',
            priority: 'low',
            dueDate: undefined,
            startedAt: undefined,
            completedAt: undefined
          };
        }
      );

      store.updateTaskNode(nodeId, { status: 'in-progress' }, viewerSource);
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);

      // Wait for BOTH writes to land. DEBOUNCED_WRITE_WAIT_MS names the wait for
      // a single write, so using it here would under-state what is being awaited;
      // waiting on the call count itself is both exact and faster.
      await vi.waitFor(() => expect(updateCallCount).toBe(2), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      expect(after.status).toBe('done');
    },
    10000
  );

  it(
    'does not suppress an unrelated, non-racing field when only one ' +
      'field is actually racing (field-scoping is preserved)',
    async () => {
      const nodeId = 'samefield-clobber-3';
      store.setNode(makeTaskNode(nodeId, 'open', 'low', 1), dbSource);

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(
        async (_id, version, _updateArg) => {
          updateCallCount++;
          if (updateCallCount === 1) {
            // Write A changes BOTH status and priority. status will race
            // with write B; priority will not.
            await new Promise((resolve) => setTimeout(resolve, 300));
            return {
              id: nodeId,
              nodeType: 'task' as const,
              content: '- [ ] seed task',
              createdAt: '2024-01-01T00:00:00.000Z',
              modifiedAt: new Date().toISOString(),
              version: version + 1,
              status: 'in-progress',
              priority: 'high',
              dueDate: undefined,
              startedAt: undefined,
              completedAt: undefined
            };
          }
          await new Promise(() => {
            /* never resolves */
          });
          throw new Error('unreachable');
        }
      );

      store.updateTaskNode(nodeId, { status: 'in-progress', priority: 'high' }, viewerSource);
      // Write B only touches status — priority is not racing.
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));
      expect(updateCallCount).toBe(2);

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      // status: B's optimistic value must survive (the racing field).
      expect(after.status).toBe('done');
      // priority: A's confirmed value must still apply normally — it was
      // never touched by B, so it isn't racing and must not be suppressed.
      expect(after.priority).toBe('high');
    },
    10000
  );
});
