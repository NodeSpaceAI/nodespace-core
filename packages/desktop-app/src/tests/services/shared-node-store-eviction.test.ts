/**
 * SharedNodeStore - reachability tracking & eviction
 *
 * SharedNodeStore is a single global cache shared by every open tab/pane
 * (`SharedNodeStore.getInstance()`), and previously never evicted anything:
 * once a node loaded, it stayed cached for the life of the app session even
 * after every tab/pane showing it closed. `navigation.svelte.ts` now reports
 * the current set of open tabs' document root node ids on every tab-state
 * change via `updateOpenDocumentRoots()`; a cached node unreachable from
 * every open root (walking its `structureTree` ancestor chain) becomes
 * eligible for eviction after a short inactivity window.
 *
 * These tests exercise the reachability/eviction mechanism directly at the
 * SharedNodeStore level (not through navigation.svelte.ts — see
 * navigation.test.ts's "SharedNodeStore reachability sync" block and
 * architecture-benchmarks.test.ts's multi-tab memory test for the real,
 * end-to-end wiring), locking in the two correctness constraints eviction
 * must never violate:
 *
 * - A node open in two panes/tabs simultaneously is never evicted while
 *   either still shows it.
 * - A node with a pending/unflushed write is never evicted mid-write.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SharedNodeStore, SimplePersistenceCoordinator } from '../../lib/services/shared-node-store.svelte';
import { structureTree } from '../../lib/stores/reactive-structure-tree.svelte';
import { createTestNode } from '../helpers';
import type { UpdateSource } from '../../lib/types/update-protocol';

const databaseSource: UpdateSource = { type: 'database', reason: 'test-setup' };
const TEST_INACTIVITY_MS = 30;

/** Real (not fake) wait, mirroring this codebase's own convention for
 * timer-driven coordinator tests (see persistence-coordinator-supersede-
 * settlement.test.ts) — fake timers interact poorly with the promise
 * microtask chains PersistenceCoordinator relies on. */
const wait = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

describe('SharedNodeStore - reachability tracking & eviction', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    store.__setEvictionInactivityMsForTesting(TEST_INACTIVITY_MS);
    structureTree.clear();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    structureTree.clear();
  });

  it('leaves eviction dormant until navigation ever reports open-tab state', async () => {
    store.setNode(createTestNode({ id: 'never-reported' }), databaseSource);

    await wait(TEST_INACTIVITY_MS + 20);

    // No updateOpenDocumentRoots call ever happened, so nothing was ever
    // swept as an eviction candidate — a node with no reported reachability
    // state is not the same as an unreachable one.
    expect(store.getNode('never-reported')).toBeDefined();
    expect(store.__getPendingEvictionCountForTesting()).toBe(0);
  });

  it('does not evict a reachable node', async () => {
    store.setNode(createTestNode({ id: 'doc-root' }), databaseSource);
    store.updateOpenDocumentRoots(['doc-root']);

    await wait(TEST_INACTIVITY_MS + 20);

    expect(store.getNode('doc-root')).toBeDefined();
  });

  it('does not evict immediately when a node becomes unreachable — only after the inactivity threshold', async () => {
    store.setNode(createTestNode({ id: 'doc-root' }), databaseSource);
    store.updateOpenDocumentRoots(['doc-root']);

    store.updateOpenDocumentRoots([]); // tab closed

    // Immediately after: still present (avoids thrashing on quick tab
    // switches — eviction is deferred, not instant).
    expect(store.getNode('doc-root')).toBeDefined();
    expect(store.__getPendingEvictionCountForTesting()).toBe(1);
  });

  it('evicts an unreachable node, and all of its per-node bookkeeping, once the inactivity threshold elapses', async () => {
    store.setNode(createTestNode({ id: 'doc-root', content: 'v1' }), databaseSource);
    store.updateNode('doc-root', { content: 'v2' }, { type: 'viewer', viewerId: 'v' }, { persist: false });
    expect(store.getVersion('doc-root')).toBeGreaterThan(1);

    store.updateOpenDocumentRoots(['doc-root']);
    store.updateOpenDocumentRoots([]); // tab closed

    await wait(TEST_INACTIVITY_MS + 40);

    expect(store.getNode('doc-root')).toBeUndefined();
    expect(store.hasNode('doc-root')).toBe(false);
    expect(store.getVersion('doc-root')).toBe(0); // versions map cleared, falls back to default
    expect(store.__getPendingEvictionCountForTesting()).toBe(0);
  });

  it('evicts every cached descendant of a document once the whole document becomes unreachable', async () => {
    store.setNode(createTestNode({ id: 'root' }), databaseSource);
    store.setNode(createTestNode({ id: 'child-1' }), databaseSource);
    store.setNode(createTestNode({ id: 'child-2' }), databaseSource);
    structureTree.addInMemoryRelationship('root', 'child-1', 1);
    structureTree.addInMemoryRelationship('root', 'child-2', 2);

    store.updateOpenDocumentRoots(['root']);
    store.updateOpenDocumentRoots([]); // tab closed

    await wait(TEST_INACTIVITY_MS + 40);

    expect(store.getNode('root')).toBeUndefined();
    expect(store.getNode('child-1')).toBeUndefined();
    expect(store.getNode('child-2')).toBeUndefined();
  });

  it('cancels a pending eviction when the node becomes reachable again before the threshold elapses (quick tab reopen)', async () => {
    store.setNode(createTestNode({ id: 'doc-root' }), databaseSource);
    store.updateOpenDocumentRoots(['doc-root']);
    store.updateOpenDocumentRoots([]); // closed

    expect(store.__getPendingEvictionCountForTesting()).toBe(1);

    store.updateOpenDocumentRoots(['doc-root']); // reopened before threshold elapses
    expect(store.__getPendingEvictionCountForTesting()).toBe(0);

    await wait(TEST_INACTIVITY_MS + 40);

    expect(store.getNode('doc-root')).toBeDefined();
  });

  it('never evicts a node open in two panes/tabs simultaneously while either still shows it', async () => {
    store.setNode(createTestNode({ id: 'shared-doc' }), databaseSource);

    // Two tabs (e.g. one per pane in a split view) both show shared-doc.
    store.updateOpenDocumentRoots(['shared-doc', 'other-open-doc']);

    // One of the two closes; shared-doc is still reported because the other
    // tab still shows it.
    store.updateOpenDocumentRoots(['shared-doc']);

    await wait(TEST_INACTIVITY_MS + 40);
    expect(store.getNode('shared-doc')).toBeDefined();

    // The second (and last) tab showing it closes too.
    store.updateOpenDocumentRoots([]);
    await wait(TEST_INACTIVITY_MS + 40);
    expect(store.getNode('shared-doc')).toBeUndefined();
  });

  it('never evicts a node with a pending/unflushed write, even after the inactivity threshold elapses', async () => {
    store.setNode(createTestNode({ id: 'dirty-node' }), databaseSource);
    store.updateOpenDocumentRoots(['dirty-node']);
    store.updateOpenDocumentRoots([]); // closed while a write is about to be in flight

    let resolveWrite: () => void = () => {};
    const writeCompletes = new Promise<void>((resolve) => {
      resolveWrite = resolve;
    });
    SimplePersistenceCoordinator.getInstance().persist(
      'dirty-node',
      () => writeCompletes,
      { mode: 'immediate' }
    );
    expect(SimplePersistenceCoordinator.getInstance().hasPending('dirty-node')).toBe(true);

    // Threshold elapses while the write is still in flight — must not evict.
    await wait(TEST_INACTIVITY_MS + 40);
    expect(store.getNode('dirty-node')).toBeDefined();

    // Write settles; the node is re-scheduled and evicted after the next
    // full inactivity window.
    resolveWrite();
    await wait(TEST_INACTIVITY_MS + 60);
    expect(store.getNode('dirty-node')).toBeUndefined();
  });

  it('never evicts a node with an active (uncommitted) atomic batch', async () => {
    // A faster, test-local threshold so several attempt/reschedule cycles
    // comfortably fit inside a generous wait — avoids the test depending on
    // a single real-timer firing at a precise moment.
    store.__setEvictionInactivityMsForTesting(20);

    store.setNode(createTestNode({ id: 'batched-node', nodeType: 'text' }), databaseSource);
    store.updateOpenDocumentRoots(['batched-node']);
    store.updateOpenDocumentRoots([]); // closed

    store.startBatch('batched-node', 60_000); // long timeout — won't auto-commit mid-test
    store.addToBatch('batched-node', { content: 'mid-batch edit' });

    // Let at least one attempt-then-reschedule cycle pass while the batch
    // is still active — must not evict mid-batch.
    await wait(100);
    expect(store.getNode('batched-node')).toBeDefined();

    store.cancelBatch('batched-node');

    // The next attempt (already on a ~20ms cadence) now finds no active
    // batch and no pending write, and evicts.
    await wait(100);
    expect(store.getNode('batched-node')).toBeUndefined();
  });

  it('__setEvictionInactivityMsForTesting configures the actual delay used', async () => {
    store.__setEvictionInactivityMsForTesting(5);
    store.setNode(createTestNode({ id: 'fast-evict' }), databaseSource);
    store.updateOpenDocumentRoots(['fast-evict']);
    store.updateOpenDocumentRoots([]);

    await wait(50);

    expect(store.getNode('fast-evict')).toBeUndefined();
  });
});
