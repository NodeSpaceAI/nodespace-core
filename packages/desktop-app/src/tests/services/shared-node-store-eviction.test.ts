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

  describe('pinNodes — reachability for consumers outside the structureTree walk', () => {
    // Reproduces the real bug: a node with NO structureTree parent — exactly
    // what a query-view result row looks like (it lives under whatever
    // parent document it was originally created in, not as a child of the
    // query node) or what an ensureNode()-resolved [[wikilink]] reference
    // looks like (never touches structureTree at all). Against the
    // structureTree-only reachability check, this node is unreachable from
    // any open tab the instant it's cached — even while a still-open query
    // view/reference is the only thing currently displaying it. This whole
    // describe block would fail on the pre-pin code: every node here has no
    // parent and is never itself an open document root, so isReachable()
    // would report it unreachable from the moment it's cached, and it would
    // be evicted after the threshold regardless of the pin calls below.

    it('a node reachable only via an explicit pin (e.g. a query-view row) is never evicted while pinned', async () => {
      // A tab IS open (on a document this node has no structural relation
      // to), so eviction is active — not exercising the "dormant" case.
      store.updateOpenDocumentRoots(['some-open-tab-root']);

      store.setNode(createTestNode({ id: 'query-row-1' }), databaseSource);
      store.pinNodes('query-view-instance-1', ['query-row-1']);

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode('query-row-1')).toBeDefined();
    });

    it('a wikilink-style reference (ensureNode, no structureTree edge) is never evicted while its component keeps it pinned', async () => {
      store.updateOpenDocumentRoots(['some-open-tab-root']);
      store.setNode(createTestNode({ id: 'wikilink-target' }), databaseSource);

      // Mirrors node-ref-inline.svelte / node-card-inline.svelte: pin on
      // resolve, keep pinning across re-renders (same owner id, same call).
      store.pinNodes('node-ref-instance-1', ['wikilink-target']);
      await wait(TEST_INACTIVITY_MS + 20);
      expect(store.getNode('wikilink-target')).toBeDefined();

      // Still pinned after another window — a long-hovering/long-open
      // reference must not eventually lose the race with the timer.
      await wait(TEST_INACTIVITY_MS + 20);
      expect(store.getNode('wikilink-target')).toBeDefined();
    });

    it('unpinning (component unmount) makes the node evictable again after the threshold', async () => {
      store.updateOpenDocumentRoots(['some-open-tab-root']);
      store.setNode(createTestNode({ id: 'query-row-2' }), databaseSource);
      store.pinNodes('query-view-instance-2', ['query-row-2']);

      await wait(TEST_INACTIVITY_MS + 20);
      expect(store.getNode('query-row-2')).toBeDefined(); // still pinned

      store.unpinAll('query-view-instance-2'); // component unmounted / row scrolled out

      await wait(TEST_INACTIVITY_MS + 40);
      expect(store.getNode('query-row-2')).toBeUndefined();
    });

    it('a node pinned by two owners stays reachable until BOTH unpin (query view + inline reference to the same row)', async () => {
      store.updateOpenDocumentRoots(['some-open-tab-root']);
      store.setNode(createTestNode({ id: 'shared-cross-ref' }), databaseSource);

      store.pinNodes('query-view-instance-3', ['shared-cross-ref']);
      store.pinNodes('node-ref-instance-3', ['shared-cross-ref']);

      store.unpinAll('query-view-instance-3'); // one consumer goes away
      await wait(TEST_INACTIVITY_MS + 40);
      expect(store.getNode('shared-cross-ref')).toBeDefined(); // the other still pins it

      store.unpinAll('node-ref-instance-3'); // the last one goes away
      await wait(TEST_INACTIVITY_MS + 40);
      expect(store.getNode('shared-cross-ref')).toBeUndefined();
    });

    it('a changing pin set (query results updating) only keeps the CURRENT rows reachable', async () => {
      store.updateOpenDocumentRoots(['some-open-tab-root']);
      store.setNode(createTestNode({ id: 'stale-row' }), databaseSource);
      store.setNode(createTestNode({ id: 'fresh-row' }), databaseSource);

      store.pinNodes('query-view-instance-4', ['stale-row']);
      // Query re-executed: 'stale-row' no longer matches, 'fresh-row' does.
      // pinNodes replaces the owner's set wholesale, not as a delta.
      store.pinNodes('query-view-instance-4', ['fresh-row']);

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode('stale-row')).toBeUndefined(); // no longer pinned by anything
      expect(store.getNode('fresh-row')).toBeDefined(); // currently pinned
    });

    it('a node with no open-document report at all stays dormant-reachable even when unpinned (baseline: pinning is additive, not a new dormancy gate)', () => {
      // No updateOpenDocumentRoots call in this test — mirrors the existing
      // "leaves eviction dormant until navigation ever reports open-tab
      // state" guarantee; pinNodes must not change that default.
      store.setNode(createTestNode({ id: 'never-reported-2' }), databaseSource);
      store.pinNodes('some-owner', []); // no-op pin, exercises the empty-set path
      expect(store.__getPendingEvictionCountForTesting()).toBe(0);
    });
  });
});
