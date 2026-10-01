/**
 * SharedNodeStore - ADR-053 database epoch guard
 *
 * ADR-053 ("One Daemon, Multiple Local Databases") lets the desktop hot-swap
 * the active database. `clearAll()` (invoked by the switch) bumps a monotonic
 * database epoch. A read (e.g. loadChildren/getNode) dispatched against the
 * previous database whose response resolves *after* the switch captured the
 * old epoch and must be dropped — otherwise the previous database's rows are
 * written into the now-active store as orphans (invisible in the outliner but
 * reachable via global search / mention resolution until the next reload).
 *
 * These tests lock that guard in place: a read whose response lands after the
 * epoch advances must NOT populate the store, while a read within the same
 * epoch must apply exactly as before.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import type { Node } from '../../lib/types';

describe('SharedNodeStore - ADR-053 database epoch guard', () => {
  let store: SharedNodeStore;

  const makeNode = (id: string): Node => ({
    lifecycleStatus: 'active',
    id,
    nodeType: 'text',
    content: `content-${id}`,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  });

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  it('bumps the epoch on clearAll', () => {
    const before = store.currentEpoch();
    store.clearAll();
    expect(store.currentEpoch()).toBe(before + 1);
  });

  it('drops a read whose response resolves after the active database switched', async () => {
    // Defer getChildren so the epoch can advance while the read is in flight.
    let resolveChildren: (nodes: Node[]) => void = () => {};
    const childrenPromise = new Promise<Node[]>((resolve) => {
      resolveChildren = resolve;
    });
    vi.spyOn(backendAdapter, 'getChildren').mockReturnValue(childrenPromise);

    // Seed the parent so loadChildrenForParent skips the parent prefetch and the
    // only in-flight read is getChildren.
    store.setNode(makeNode('parent'), { type: 'database', reason: 'seed' });
    const epochBefore = store.currentEpoch();

    // Read dispatched under epoch N (against "database A").
    const loadPromise = store.loadChildrenForParent('parent');

    // The active database switches while the read is in flight: clearAll bumps
    // the epoch to N+1 and empties the store (as the real switch does).
    store.clearAll();
    expect(store.currentEpoch()).toBe(epochBefore + 1);

    // Database A's response lands after the switch.
    resolveChildren([makeNode('stale-child')]);
    const result = await loadPromise;

    // The stale row must NOT have populated the now-active (empty) store.
    expect(store.hasNode('stale-child')).toBe(false);
    expect(store.getNodeCount()).toBe(0);
    expect(result).toEqual([]);
  });

  it('applies a read that resolves within the same epoch', async () => {
    vi.spyOn(backendAdapter, 'getChildren').mockResolvedValue([makeNode('fresh-child')]);

    store.setNode(makeNode('parent'), { type: 'database', reason: 'seed' });

    // No intervening switch: the read applies exactly as before.
    const result = await store.loadChildrenForParent('parent');

    expect(store.hasNode('fresh-child')).toBe(true);
    expect(result.map((n) => n.id)).toEqual(['fresh-child']);
  });
});
