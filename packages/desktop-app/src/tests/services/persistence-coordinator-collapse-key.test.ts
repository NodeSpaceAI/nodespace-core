/**
 * SimplePersistenceCoordinator only lets a later write replace a waiting one
 * when the waiting write allows it (`PersistOptions.collapseKey`), and exposes
 * the persistence sequence a move uses to tell which writes its flush covers.
 *
 * A write that carries changes no later write re-sends (a generic write's
 * non-content fields) must survive a later write for the same node queued
 * behind the same in-flight RPC; a burst of keystrokes must still collapse.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  SimplePersistenceCoordinator,
  OperationCancelledError
} from '../../lib/services/shared-node-store.svelte';

/** An operation that stays in flight until `release()` is called. */
function heldOperation() {
  let release: () => void = () => {};
  const operation = () =>
    new Promise<void>((resolve) => {
      release = resolve;
    });
  return { operation, release: () => release() };
}

describe('SimplePersistenceCoordinator - collapse keys', () => {
  let coordinator: SimplePersistenceCoordinator;

  beforeEach(() => {
    SimplePersistenceCoordinator.resetInstance();
    coordinator = SimplePersistenceCoordinator.getInstance();
  });

  afterEach(() => {
    SimplePersistenceCoordinator.resetInstance();
  });

  it('keeps a keyed queued write when a write with a different key queues behind it, and runs both in order', async () => {
    const nodeId = 'node-keep';
    const held = heldOperation();
    const ran: string[] = [];

    coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    const fields = coordinator.persist(nodeId, async () => void ran.push('fields'), {
      mode: 'immediate',
      collapseKey: 'fields-1'
    });
    const content = coordinator.persist(nodeId, async () => void ran.push('content'), {
      mode: 'debounce',
      collapseKey: 'content'
    });

    held.release();
    await Promise.all([fields.promise, content.promise]);
    expect(ran).toEqual(['fields', 'content']);
  }, 2000);

  it('lets a later write with the same key replace a queued one', async () => {
    const nodeId = 'node-same-key';
    const held = heldOperation();
    const ran: string[] = [];

    coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    const first = coordinator.persist(nodeId, async () => void ran.push('first'), {
      mode: 'debounce',
      collapseKey: 'content'
    });
    const second = coordinator.persist(nodeId, async () => void ran.push('second'), {
      mode: 'debounce',
      collapseKey: 'content'
    });

    await expect(first.promise).rejects.toBeInstanceOf(OperationCancelledError);
    held.release();
    await second.promise;
    expect(ran).toEqual(['second']);
  }, 2000);

  it('lets any later write replace a queued write without a key', async () => {
    const nodeId = 'node-unkeyed';
    const held = heldOperation();
    const ran: string[] = [];

    coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    const unkeyed = coordinator.persist(nodeId, async () => void ran.push('unkeyed'), {
      mode: 'immediate'
    });
    const keyed = coordinator.persist(nodeId, async () => void ran.push('keyed'), {
      mode: 'immediate',
      collapseKey: 'fields-1'
    });

    await expect(unkeyed.promise).rejects.toBeInstanceOf(OperationCancelledError);
    held.release();
    await keyed.promise;
    expect(ran).toEqual(['keyed']);
  }, 2000);

  it('starts a keyed debounced write instead of cancelling it when a write with a different key arrives', async () => {
    const nodeId = 'node-debounced';
    const ran: string[] = [];

    const fields = coordinator.persist(nodeId, async () => void ran.push('fields'), {
      mode: 'debounce',
      collapseKey: 'fields-1'
    });
    const content = coordinator.persist(nodeId, async () => void ran.push('content'), {
      mode: 'debounce',
      collapseKey: 'content'
    });

    await Promise.all([fields.promise, content.promise]);
    expect(ran).toEqual(['fields', 'content']);
  }, 2000);

  it('makes flush helpers wait for every queued write, not just the first', async () => {
    const nodeId = 'node-flush';
    const held = heldOperation();
    const ran: string[] = [];

    coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    coordinator.persist(nodeId, async () => void ran.push('fields'), {
      mode: 'immediate',
      collapseKey: 'fields-1'
    });
    coordinator.persist(nodeId, async () => void ran.push('content'), {
      mode: 'immediate',
      collapseKey: 'content'
    });

    const flushed = coordinator.flushAndWaitForNodes([nodeId]);
    held.release();
    expect(await flushed).toEqual(new Set());
    expect(ran).toEqual(['fields', 'content']);
    expect(coordinator.hasPending(nodeId)).toBe(false);
  }, 2000);

  it('clearQueued settles every queued write and leaves nothing pending', async () => {
    const nodeId = 'node-clear';
    const held = heldOperation();

    const inFlight = coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    const a = coordinator.persist(nodeId, async () => {}, {
      mode: 'immediate',
      collapseKey: 'fields-1'
    });
    const b = coordinator.persist(nodeId, async () => {}, {
      mode: 'immediate',
      collapseKey: 'content'
    });

    coordinator.clearQueued(nodeId);
    await expect(a.promise).rejects.toBeInstanceOf(OperationCancelledError);
    await expect(b.promise).rejects.toBeInstanceOf(OperationCancelledError);
    expect(coordinator.isQueued(nodeId)).toBe(false);

    held.release();
    await inFlight.promise;
    expect(coordinator.hasPending(nodeId)).toBe(false);
  }, 2000);

  it('reports the persistence sequence of the executing write, including a queued one once it runs', async () => {
    const nodeId = 'node-sequence';
    const held = heldOperation();
    const seen: Array<number | undefined> = [];

    coordinator.persist(nodeId, held.operation, { mode: 'immediate' });
    const firstSequence = coordinator.sequence();
    expect(coordinator.executingSequence(nodeId)).toBe(firstSequence);

    const queued = coordinator.persist(
      nodeId,
      async () => void seen.push(coordinator.executingSequence(nodeId)),
      { mode: 'immediate' }
    );
    const queuedSequence = coordinator.sequence();
    expect(queuedSequence).toBeGreaterThan(firstSequence);

    held.release();
    await queued.promise;
    expect(seen).toEqual([queuedSequence]);
    expect(coordinator.executingSequence(nodeId)).toBeUndefined();
  }, 2000);
});
