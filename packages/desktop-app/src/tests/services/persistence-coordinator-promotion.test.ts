/**
 * SimplePersistenceCoordinator promotes a node's still-debounced write when an
 * immediate write for the same node arrives, instead of cancelling it.
 *
 * Each write captures only its own changes, so cancelling a waiting debounced
 * content write in favour of, say, a status change would lose the typed text.
 * The debounced write is started at once and the immediate write collapses
 * behind it, so both run, in order. A debounced write replacing another
 * debounced write is the intended keystroke collapse and still cancels.
 *
 * Exercises the coordinator directly, like
 * persistence-coordinator-supersede-settlement.test.ts.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  SimplePersistenceCoordinator,
  OperationCancelledError
} from '../../lib/services/shared-node-store.svelte';

describe('SimplePersistenceCoordinator - debounced write promotion', () => {
  let coordinator: SimplePersistenceCoordinator;

  beforeEach(() => {
    SimplePersistenceCoordinator.resetInstance();
    coordinator = SimplePersistenceCoordinator.getInstance();
  });

  afterEach(() => {
    SimplePersistenceCoordinator.resetInstance();
  });

  it('runs a waiting debounced write, then the immediate write, and resolves both', async () => {
    const nodeId = 'promote-1';
    const order: string[] = [];

    const debounced = coordinator.persist(
      nodeId,
      async () => {
        order.push('debounced');
      },
      { mode: 'debounce' }
    );
    const immediate = coordinator.persist(
      nodeId,
      async () => {
        order.push('immediate');
      },
      { mode: 'immediate' }
    );

    // Both settle successfully — the promoted write is not cancelled.
    await expect(debounced.promise).resolves.toBeUndefined();
    await expect(immediate.promise).resolves.toBeUndefined();
    expect(order).toEqual(['debounced', 'immediate']);
  }, 2000);

  it('keeps the node pending until both writes finish, so flushes wait for both', async () => {
    const nodeId = 'promote-2';
    let releaseDebounced!: () => void;
    const done: string[] = [];

    coordinator.persist(
      nodeId,
      () =>
        new Promise<void>((resolve) => {
          releaseDebounced = () => {
            done.push('debounced');
            resolve();
          };
        }),
      { mode: 'debounce' }
    );
    coordinator.persist(
      nodeId,
      async () => {
        done.push('immediate');
      },
      { mode: 'immediate' }
    );

    expect(coordinator.hasPending(nodeId)).toBe(true);
    const flushed = coordinator.flushAndWaitForNodes([nodeId], 2000);
    releaseDebounced();
    await flushed;

    expect(done).toEqual(['debounced', 'immediate']);
    expect(coordinator.hasPending(nodeId)).toBe(false);
  }, 3000);

  it('still cancels a debounced write replaced by another debounced write', async () => {
    const nodeId = 'promote-3';
    let firstCalls = 0;
    let secondCalls = 0;

    const first = coordinator.persist(
      nodeId,
      async () => {
        firstCalls++;
      },
      { mode: 'debounce' }
    );
    const second = coordinator.persist(
      nodeId,
      async () => {
        secondCalls++;
      },
      { mode: 'debounce' }
    );

    await expect(first.promise).rejects.toBeInstanceOf(OperationCancelledError);
    await second.promise;
    expect(firstCalls).toBe(0);
    expect(secondCalls).toBe(1);
  }, 2000);

  it('runs a pending edit before a following delete', async () => {
    const nodeId = 'promote-4';
    const order: string[] = [];

    coordinator.persist(
      nodeId,
      async () => {
        order.push('edit');
      },
      { mode: 'debounce' }
    );
    const del = coordinator.persist(
      nodeId,
      async () => {
        order.push('delete');
      },
      { mode: 'immediate' }
    );

    await del.promise;
    expect(order).toEqual(['edit', 'delete']);
  }, 2000);
});
