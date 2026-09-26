/**
 * SimplePersistenceCoordinator keeps a node marked as executing from the
 * moment an in-flight write finishes until the write queued behind it starts.
 *
 * The queued write starts a microtask after the finished write's `finally`
 * block. A `persist()` running in between — typically code awaiting the
 * finished write's promise — must queue behind the queued write rather than
 * cancel its placeholder and start a second write alongside it.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SimplePersistenceCoordinator } from '../../lib/services/shared-node-store.svelte';

describe('SimplePersistenceCoordinator - queued write handoff gap', () => {
  let coordinator: SimplePersistenceCoordinator;

  beforeEach(() => {
    SimplePersistenceCoordinator.resetInstance();
    coordinator = SimplePersistenceCoordinator.getInstance();
  });

  afterEach(() => {
    SimplePersistenceCoordinator.resetInstance();
  });

  it('queues a persist() made between a write finishing and its queued write starting', async () => {
    const nodeId = 'node-gap';
    const ran: string[] = [];
    let inFlight = 0;
    let maxInFlight = 0;
    const releases: Array<() => void> = [];

    /** An operation that records concurrency and stays in flight until released. */
    const tracked = (name: string) => () =>
      new Promise<void>((resolve) => {
        inFlight++;
        maxInFlight = Math.max(maxInFlight, inFlight);
        ran.push(name);
        releases.push(() => {
          inFlight--;
          resolve();
        });
      });

    const first = coordinator.persist(nodeId, tracked('first'), { mode: 'immediate' });
    const second = coordinator.persist(nodeId, tracked('second'), {
      mode: 'immediate',
      collapseKey: 'second'
    });

    // Runs in the gap: after `first` settles, before `second` has started.
    let third: { promise: Promise<void> } | undefined;
    let executingInGap: boolean | undefined;
    const gap = first.promise.then(() => {
      executingInGap = coordinator.isExecuting(nodeId);
      third = coordinator.persist(nodeId, tracked('third'), { mode: 'immediate' });
    });

    releases.shift()?.();
    await gap;
    expect(executingInGap).toBe(true);

    // `second` starts; `third` waits behind it rather than running alongside.
    await expect.poll(() => ran).toEqual(['first', 'second']);
    expect(inFlight).toBe(1);

    releases.shift()?.();
    await second.promise;
    await expect.poll(() => ran).toEqual(['first', 'second', 'third']);

    releases.shift()?.();
    await third?.promise;

    expect(maxInFlight).toBe(1);
    expect(coordinator.isExecuting(nodeId)).toBe(false);
    expect(coordinator.hasPending(nodeId)).toBe(false);
  }, 2000);
});
