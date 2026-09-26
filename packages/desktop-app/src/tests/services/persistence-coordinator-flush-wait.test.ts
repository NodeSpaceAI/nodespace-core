/**
 * SimplePersistenceCoordinator.flushAndWaitForNodes() waits for the node's
 * current pending entry, not just the one it saw when the flush started.
 *
 * A queued write replaced during the flush rejects with
 * OperationCancelledError while the in-flight write and its replacement are
 * still running; the flush must keep waiting on the replacement. The flush
 * must also never delete a pending entry it did not start, such as the
 * placeholder registered for a write queued behind the one it started.
 *
 * Exercises the coordinator directly, like
 * persistence-coordinator-promotion.test.ts.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SimplePersistenceCoordinator } from '../../lib/services/shared-node-store.svelte';

/** A write that runs until the test releases it. */
function blockedWrite(done: string[], name: string) {
  let release!: () => void;
  const operation = () =>
    new Promise<void>((resolve) => {
      release = () => {
        done.push(name);
        resolve();
      };
    });
  return { operation, release: () => release() };
}

const flushMicrotasks = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('SimplePersistenceCoordinator - flushAndWaitForNodes', () => {
  let coordinator: SimplePersistenceCoordinator;

  beforeEach(() => {
    SimplePersistenceCoordinator.resetInstance();
    coordinator = SimplePersistenceCoordinator.getInstance();
  });

  afterEach(() => {
    SimplePersistenceCoordinator.resetInstance();
  });

  it('keeps waiting when the queued write it waits on is replaced', async () => {
    const nodeId = 'flush-replaced';
    const done: string[] = [];
    const inFlight = blockedWrite(done, 'in-flight');
    const replacement = blockedWrite(done, 'replacement');

    coordinator.persist(nodeId, inFlight.operation, { mode: 'immediate' });
    const replaced = coordinator.persist(nodeId, async () => {}, { mode: 'debounce' });
    replaced.promise.catch(() => {});

    let settled = false;
    const flushed = coordinator.flushAndWaitForNodes([nodeId], 2000).then((failed) => {
      settled = true;
      return failed;
    });

    // Replace the queued write the flush is waiting on.
    coordinator.persist(nodeId, replacement.operation, { mode: 'debounce' });
    await flushMicrotasks();
    expect(settled).toBe(false);

    inFlight.release();
    await flushMicrotasks();
    expect(settled).toBe(false);

    replacement.release();
    expect(await flushed).toEqual(new Set());
    expect(done).toEqual(['in-flight', 'replacement']);
  }, 3000);

  it('reports a failure when the queued write is cancelled with nothing replacing it', async () => {
    const nodeId = 'flush-cleared';
    const done: string[] = [];
    const inFlight = blockedWrite(done, 'in-flight');

    coordinator.persist(nodeId, inFlight.operation, { mode: 'immediate' });
    const queued = coordinator.persist(nodeId, async () => {}, { mode: 'debounce' });
    queued.promise.catch(() => {});

    const flushed = coordinator.flushAndWaitForNodes([nodeId], 2000);
    coordinator.clearQueued(nodeId);
    inFlight.release();

    expect(await flushed).toEqual(new Set([nodeId]));
  }, 3000);

  it('does not delete the placeholder of a write queued behind the one it started', async () => {
    const nodeId = 'flush-placeholder';
    const done: string[] = [];
    const first = blockedWrite(done, 'first');
    const second = blockedWrite(done, 'second');

    coordinator.persist(nodeId, first.operation, { mode: 'debounce' });
    const flushed = coordinator.flushAndWaitForNodes([nodeId], 2000);
    // The flush started the debounced write; this one queues behind it.
    coordinator.persist(nodeId, second.operation, { mode: 'debounce' });

    first.release();
    expect(await flushed).toEqual(new Set());
    await flushMicrotasks();

    // The second write is still running, so the node must still be pending.
    expect(done).toEqual(['first']);
    expect(coordinator.isPending(nodeId)).toBe(true);

    second.release();
    await flushMicrotasks();
    expect(coordinator.isPending(nodeId)).toBe(false);
  }, 3000);

  it('reports a timeout as a failure', async () => {
    const nodeId = 'flush-timeout';
    const stuck = blockedWrite([], 'stuck');

    coordinator.persist(nodeId, stuck.operation, { mode: 'immediate' });
    expect(await coordinator.flushAndWaitForNodes([nodeId], 20)).toEqual(new Set([nodeId]));
    stuck.release();
  }, 3000);
});
