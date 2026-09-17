/**
 * Regression: `updateTaskNode()`'s SUCCESS path (inside the
 * `PersistenceCoordinator.getInstance().persist(...)` closure, a few lines
 * above the OCC catch handler) used to read `this.nodes.get(nodeId)` at the
 * moment this specific write's RPC resolved and mutate it in place via
 * `Object.assign(localNode, { status, priority, dueDate, assignee,
 * startedAt, completedAt })` — unconditionally, using ALL of this write's
 * own response fields, not just the ones it actually asked to change.
 *
 * Concrete failure sequence (verified against the pre-fix code before this
 * fix was applied — see the issue this closes):
 * 1. Write A changes one task field (e.g. priority) for node X. Its
 *    optimistic apply lands, then its persist() RPC is sent and is still in
 *    flight.
 * 2. Write B, for the SAME node, changes a DIFFERENT field (e.g. status)
 *    while A is still executing. `PersistenceCoordinator.persist()`'s
 *    `isExecuting` branch collapses B into `queuedOperations` (a real RPC
 *    for B is not sent yet — only one write executes per node at a time).
 *    B's own optimistic update (built from A's already-optimistic node) DOES
 *    land in the store immediately, synchronously, at call time.
 * 3. Write A's RPC resolves successfully. Its response reflects the
 *    server's view as of A's OWN request — which never included B's change,
 *    since B hasn't been sent yet.
 * 4. A's success handler read `this.nodes.get(nodeId)` — which is now B's
 *    optimistic object, not A's own — and unconditionally `Object.assign`ed
 *    ALL of A's response's type-specific fields onto it. This included
 *    fields A never changed (e.g. `status`), stamped with A's stale pre-B
 *    value, clobbering B's optimistic edit before B's own write had even
 *    been attempted.
 *
 * Fixed by limiting the patch applied from each write's response to only
 * the fields THAT write's own request actually asked to change (mirroring
 * what `localChanges` already does for the optimistic apply at the top of
 * `updateTaskNode()`). A field a write didn't touch is left exactly as the
 * store currently has it, however it got there.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator
} from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import { CASCADE_SETTLE_TIMEOUT_MS } from '../utils/test-constants';

type TaskLikeNode = Node & {
  status: string;
  priority?: string;
  dueDate?: string | null;
  startedAt?: string | null;
  completedAt?: string | null;
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

describe('updateTaskNode success-path clobber — queued-write regression', () => {
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
    "does not let write A's successful RPC response clobber write B's " +
      "still-in-flight optimistic field for a DIFFERENT field via shared-" +
      'reference Object.assign',
    async () => {
      const nodeId = 'success-clobber-1';
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
            // priority changed to 'high' (A's edit), status still 'open'
            // (A never touched it — this is the field B is about to change).
            return {
              id: nodeId,
              nodeType: 'task' as const,
              content: '- [ ] seed task',
              createdAt: '2024-01-01T00:00:00.000Z',
              modifiedAt: new Date().toISOString(),
              version: version + 1,
              status: 'open',
              priority: 'high',
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
          // its own success handler's Object.assign would silently
          // re-correct the value on its own, masking a merely-transient
          // clobber rather than proving the fix.
          await new Promise(() => {
            /* never resolves */
          });
          throw new Error('unreachable');
        }
      );

      // Write A: mode is always 'immediate' for updateTaskNode, so the
      // persist starts synchronously — no debounce wait needed.
      store.updateTaskNode(nodeId, { priority: 'high' }, viewerSource);
      expect(coord.isExecuting(nodeId)).toBe(true);

      // Write B: submitted while A is executing -> collapses into
      // queuedOperations. B's optimistic status lands immediately, built
      // from A's already-optimistic node (priority: 'high' carried over).
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);
      const afterB = store.getNode(nodeId) as unknown as TaskLikeNode;
      expect(afterB.status).toBe('done');
      expect(afterB.priority).toBe('high');

      // Let A's success handler fire, and B's queued write get promoted into
      // execution (call #2 above). Waiting on that promotion is exact; a fixed
      // duration would only be a guess that it has happened by now, and call #2
      // never resolves so there is nothing further to settle.
      await vi.waitFor(() => expect(updateCallCount).toBe(2), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      // B's still-in-flight optimistic status must survive A's success
      // handler — A never asked to change status, so its response's status
      // field (a stale pre-B snapshot) must not be applied.
      expect(after.status).toBe('done');
      expect(after.priority).toBe('high');
    },
    10000
  );

  it(
    'still converges to both writes\' correct fields once B\'s own real ' +
      'persist attempt completes (regression check: the fix does not break ' +
      'normal eventual consistency)',
    async () => {
      const nodeId = 'success-clobber-2';
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
              status: 'open',
              priority: 'high',
              dueDate: undefined,
              startedAt: undefined,
              completedAt: undefined
            };
          }
          // Write B's real persist, once promoted: succeeds normally,
          // reflecting the server's merged state (A's priority + B's own
          // status change).
          return {
            id: nodeId,
            nodeType: 'task' as const,
            content: '- [ ] seed task',
            createdAt: '2024-01-01T00:00:00.000Z',
            modifiedAt: new Date().toISOString(),
            version: version + 1,
            status: updateArg.status ?? 'open',
            priority: 'high',
            dueDate: undefined,
            startedAt: undefined,
            completedAt: undefined
          };
        }
      );

      store.updateTaskNode(nodeId, { priority: 'high' }, viewerSource);
      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);

      // Wait for BOTH writes to land. A single-write debounce constant would
      // under-state what is being awaited here; waiting on the call count
      // itself is both exact and faster.
      await vi.waitFor(() => expect(updateCallCount).toBe(2), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      expect(after.status).toBe('done');
      expect(after.priority).toBe('high');
    },
    10000
  );
});
