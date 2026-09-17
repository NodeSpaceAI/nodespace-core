/**
 * Regression: `SimplePersistenceCoordinator`'s dependency-wait loop
 * (`runOperation`, `persist()`'s `dependencies` option) let a dependency's
 * cancellation reach a DEPENDENT write as a raw `OperationCancelledError` —
 * indistinguishable, at every `.catch(err => ...)` call site in this file,
 * from the dependent write being personally cancelled. Every one of those
 * sites treats `OperationCancelledError` as "expected, ignore it" (a newer
 * write for the SAME node supersedes this one, so nothing was lost). That's
 * true when a write is cancelled directly (via `cancelPending()`,
 * `clearQueued()`, or the queued-operation overwrite). It is NOT true when
 * the cancellation belongs to a DIFFERENT node this write was waiting on:
 * the dependent's own `op()` (its actual backend RPC) never even ran, and
 * nothing else is going to retry it — silently discarding a write with no
 * trace.
 *
 * Concrete failure sequence (from the issue this closes):
 * 1. Node B's persist() declares a dependency on node A (`persistenceDependencies`
 *    / `afterSiblingId` — anything that ends up in `persist()`'s `dependencies`
 *    array).
 * 2. B starts executing, reaches the dependency-wait loop, and suspends
 *    awaiting A's `pendingOperations` entry's promise.
 * 3. A's write is cancelled (superseded by a newer edit to A before A's own
 *    write ever executes) — `cancelPending()` rejects A's promise with
 *    `OperationCancelledError`.
 * 4. Before the fix: B's dependency-wait `await` throws that SAME
 *    `OperationCancelledError` instance, `runOperation`'s catch treats it as
 *    B's own failure, and B's `.catch` site sees `instanceof
 *    OperationCancelledError` and silently returns — B's write vanishes with
 *    no notification and no retry, even though B was never itself cancelled.
 *
 * Exercises `SimplePersistenceCoordinator` directly (exported for this
 * purpose — see `persistence-coordinator-supersede-settlement.test.ts`) for
 * the coordinator-level mechanism, and `SharedNodeStore.updateNode()` (the
 * real production call site, via `persistenceDependencies`) for the
 * end-to-end behavior a user would actually observe.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator,
  OperationCancelledError,
  DependencyFailedError
} from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import { DEBOUNCED_WRITE_WAIT_MS } from '../utils/test-constants';

describe('Dependency-wait cancellation propagation', () => {
  describe('SimplePersistenceCoordinator — coordinator-level mechanism', () => {
    let coordinator: SimplePersistenceCoordinator;

    beforeEach(() => {
      SimplePersistenceCoordinator.resetInstance();
      coordinator = SimplePersistenceCoordinator.getInstance();
    });

    afterEach(() => {
      SimplePersistenceCoordinator.resetInstance();
    });

    it("wraps a dependency's cancellation in DependencyFailedError instead of letting a raw OperationCancelledError propagate to the dependent", async () => {
      const nodeA = 'dep-node-a';
      const nodeB = 'dep-node-b';

      const opA = async () => {};
      let opBCalls = 0;
      const opB = async () => {
        opBCalls++;
      };

      // A is debounced — registered in pendingOperations, timer not yet fired.
      const handleA = coordinator.persist(nodeA, opA, { mode: 'debounce' });

      // B declares a dependency on A and starts immediately, suspending in
      // the dependency-wait loop on A's (still-pending) promise.
      const handleB = coordinator.persist(nodeB, opB, {
        mode: 'immediate',
        dependencies: [nodeA]
      });

      // A is superseded before its debounce timer fires — cancelPending()
      // rejects A's promise with a raw OperationCancelledError.
      coordinator.cancelPending(nodeA);
      await expect(handleA.promise).rejects.toBeInstanceOf(OperationCancelledError);

      // B was never itself cancelled — its own promise must reject with a
      // DependencyFailedError (distinguishable from "I was cancelled"), not
      // the raw OperationCancelledError that propagated from A.
      await expect(handleB.promise).rejects.toBeInstanceOf(DependencyFailedError);
      await expect(handleB.promise).rejects.not.toBeInstanceOf(OperationCancelledError);

      // B's own op() must never have run — it never got past the
      // dependency-wait loop.
      expect(opBCalls).toBe(0);
    }, 2000);

    it('DependencyFailedError carries the dependency id and the original cause', async () => {
      const nodeA = 'dep-node-a2';
      const nodeB = 'dep-node-b2';
      const opA = async () => {};
      const opB = async () => {};

      const handleA = coordinator.persist(nodeA, opA, { mode: 'debounce' });
      const handleB = coordinator.persist(nodeB, opB, {
        mode: 'immediate',
        dependencies: [nodeA]
      });

      coordinator.cancelPending(nodeA);
      await expect(handleA.promise).rejects.toBeInstanceOf(OperationCancelledError);

      try {
        await handleB.promise;
        expect.unreachable('handleB.promise must reject');
      } catch (err) {
        expect(err).toBeInstanceOf(DependencyFailedError);
        const depErr = err as DependencyFailedError;
        expect(depErr.dependencyId).toBe(nodeA);
        expect(depErr.cause).toBeInstanceOf(OperationCancelledError);
      }
    }, 2000);

    it('a genuine (non-cancellation) dependency failure is also wrapped, not just cancellation', async () => {
      const nodeA = 'dep-node-a3';
      const nodeB = 'dep-node-b3';
      const realFailure = new Error('backend write actually failed');

      const opA = () => Promise.reject(realFailure);
      let opBCalls = 0;
      const opB = async () => {
        opBCalls++;
      };

      // A executes immediately and fails for a real (non-cancellation) reason.
      const handleA = coordinator.persist(nodeA, opA, { mode: 'immediate' });
      await expect(handleA.promise).rejects.toBe(realFailure);

      // B depends on A — by the time B's persist() call is made, A has
      // already failed and been removed from pendingOperations, so this
      // covers the case where the dependency lookup itself finds nothing
      // (see the mid-flight case below for the race-with-fetch case).
      const handleB = coordinator.persist(nodeB, opB, {
        mode: 'immediate',
        dependencies: [nodeA]
      });
      // With A already gone from pendingOperations by the time B looks it
      // up, B's dependency-wait loop finds nothing to wait on and B's own
      // op() runs normally — this is the correct, existing behavior for a
      // dependency that already finished (successfully or not) before the
      // dependent even started. Documents the scope boundary: this fix
      // protects a dependency that is GENUINELY still in flight when the
      // dependent starts waiting on it (covered by the mid-flight test
      // below), not one that already settled beforehand.
      await handleB.promise;
      expect(opBCalls).toBe(1);
    }, 2000);

    it('a genuine dependency failure that is still in flight when the dependent starts waiting is also wrapped', async () => {
      const nodeA = 'dep-node-a4';
      const nodeB = 'dep-node-b4';
      const realFailure = new Error('backend write actually failed');

      let rejectOpA: (error: Error) => void = () => {};
      const opA = () =>
        new Promise<void>((_resolve, reject) => {
          rejectOpA = reject;
        });
      let opBCalls = 0;
      const opB = async () => {
        opBCalls++;
      };

      // A starts executing immediately and stays in flight (registered in
      // pendingOperations) until rejected below.
      const handleA = coordinator.persist(nodeA, opA, { mode: 'immediate' });
      expect(coordinator.isExecuting(nodeA)).toBe(true);

      // B depends on A and suspends in the dependency-wait loop while A is
      // still genuinely in flight.
      const handleB = coordinator.persist(nodeB, opB, {
        mode: 'immediate',
        dependencies: [nodeA]
      });

      rejectOpA(realFailure);
      await expect(handleA.promise).rejects.toBe(realFailure);

      // B's own promise rejects with a DependencyFailedError wrapping the
      // real failure — not silently swallowed, not misclassified as a
      // cancellation.
      await expect(handleB.promise).rejects.toBeInstanceOf(DependencyFailedError);
      try {
        await handleB.promise;
      } catch (err) {
        expect((err as DependencyFailedError).cause).toBe(realFailure);
      }
      expect(opBCalls).toBe(0);
    }, 2000);
  });

  describe('SharedNodeStore.updateNode() — end-to-end production call site', () => {
    let store: SharedNodeStore;

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

    it("does not silently swallow a dependent write when the write it depends on is cancelled — surfaces a failure instead", async () => {
      // Seed both nodes as persisted (setNode called twice, mirroring the
      // pattern used elsewhere in this suite — the UPDATE path is what
      // exercises PersistenceCoordinator.persist(), not CREATE).
      store.setNode(makeNode('node-a', 'seed-a', 1), dbSource);
      store.setNode(makeNode('node-a', 'seed-a', 1), dbSource);
      store.setNode(makeNode('node-b', 'seed-b', 1), dbSource);
      store.setNode(makeNode('node-b', 'seed-b', 1), dbSource);

      const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...makeNode('node-a', 'confirmed', 2)
      });

      // A: content-only change -> debounce mode, stays on its debounce
      // timer (not yet executing) until we let it fire below.
      store.updateNode('node-a', { content: 'a-first-edit' }, viewerSource);

      // B: depends on A, forced into immediate mode via a property change so
      // it starts synchronously and suspends in the dependency-wait loop on
      // A's still-pending write.
      store.updateNode(
        'node-b',
        { content: 'b-edit', properties: {} },
        viewerSource,
        { persistenceDependencies: ['node-a'] }
      );

      // A is superseded by a second edit BEFORE its debounce timer fires —
      // this is the natural, realistic path to cancelPending('node-a'):
      // a fresh persist() call for a node with no in-flight write cancels
      // whatever was still waiting on the debounce timer.
      store.updateNode('node-a', { content: 'a-second-edit-supersedes-first' }, viewerSource);

      // Let everything settle: A's superseding second edit's own debounce
      // timer, B's dependency-wait rejection, and the resulting .catch
      // handlers.
      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      // B's own backend RPC must never have fired — it never got past the
      // dependency-wait loop.
      const bCalls = updateSpy.mock.calls.filter((call) => call[0] === 'node-b');
      expect(bCalls).toHaveLength(0);

      // B's failure must be surfaced, not silently discarded — the store's
      // updateNode() catch site already surfaces non-cancellation failures
      // via a write-failure notification; a DependencyFailedError must hit
      // that same path instead of being misread as "node-b was cancelled,
      // ignore it" (node-b was never cancelled — node-a was).
      const bNotifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 'node-b' && n.conflictType === 'write-failure'
      );
      expect(bNotifications).toHaveLength(1);
    }, 5000);

    it('a normal cancellation (no dependency involved) is still silently ignored — regression check', async () => {
      store.setNode(makeNode('node-solo', 'seed', 1), dbSource);
      store.setNode(makeNode('node-solo', 'seed', 1), dbSource);

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...makeNode('node-solo', 'confirmed', 2)
      });

      // First edit, debounced, then superseded by a second before it fires —
      // ordinary cancellation, no dependency involved anywhere.
      store.updateNode('node-solo', { content: 'first' }, viewerSource);
      store.updateNode('node-solo', { content: 'second-supersedes-first' }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      // Must NOT surface a spurious failure notification for an ordinary,
      // expected supersede — only the propagated-dependency-failure case
      // (tested above) should newly surface anything.
      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 'node-solo'
      );
      expect(notifications).toHaveLength(0);
    }, 5000);
  });
});
