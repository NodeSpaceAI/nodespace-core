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

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore, SimplePersistenceCoordinator } from '../../lib/services/shared-node-store.svelte';
import { structureTree } from '../../lib/stores/reactive-structure-tree.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '../../lib/plugins/ui-extensions';
import { createTestNode } from '../helpers';
import type { UpdateSource } from '../../lib/types/update-protocol';
import type { NodeWithChildren } from '../../lib/types';
import { backendAdapter } from '../../lib/services/backend-adapter';

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

  describe('DATABASE_SETTINGS_NODE_ID — always-mounted app chrome outside any open tab', () => {
    // Reproduces the real bug found in review: DATABASE_SETTINGS_NODE_ID
    // (database.svelte.ts) is a global singleton read continuously by
    // always-mounted Pro-sync chrome (ui-extensions.svelte.ts's
    // resolveProSyncVariant/activeDatabaseSettings, membership.svelte.ts) —
    // it has no structureTree relationship to any open tab and is never
    // itself a tab root. Without a pin, it is
    // unreachable — and thus evicted after the inactivity threshold — the
    // instant it's cached, REGARDLESS of how often it's refreshed (setNode
    // alone doesn't cancel a pending eviction). This exact scenario is what
    // database.svelte.ts's refreshDatabaseSettings() now guards against by
    // pinning it on every call — see database.test.ts's
    // "pins DATABASE_SETTINGS_NODE_ID reachable" tests for proof that it
    // actually does. This test proves the OTHER half: that pinning it (the
    // real production node id, the real SharedNodeStore) is what makes it
    // survive exactly the failure sequence the reviewer reproduced.
    it('survives the inactivity threshold while pinned, with only an unrelated tab open', async () => {
      store.setNode(
        createTestNode({
          id: DATABASE_SETTINGS_NODE_ID,
          nodeType: 'database-settings',
          properties: { sync_enabled: true, auth_status: 'connected' }
        }),
        databaseSource
      );

      // Mirrors what refreshDatabaseSettings() does on every refresh.
      store.pinNodes('database-settings-node', [DATABASE_SETTINGS_NODE_ID]);

      // A tab IS open — on some document entirely unrelated to the settings
      // singleton — so eviction is active, not dormant.
      store.updateOpenDocumentRoots(['some-open-document-root']);

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode(DATABASE_SETTINGS_NODE_ID)).toBeDefined();
    });

    it('WITHOUT the pin, the same sequence evicts it — this is the bug the fix closes', async () => {
      store.setNode(
        createTestNode({ id: DATABASE_SETTINGS_NODE_ID, nodeType: 'database-settings' }),
        databaseSource
      );
      // No pinNodes() call here, unlike the test above.
      store.updateOpenDocumentRoots(['some-open-document-root']);

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode(DATABASE_SETTINGS_NODE_ID)).toBeUndefined();
    });
  });

  // Eviction must keep `structureTree` consistent with the cached node set:
  // a stale edge to an evicted node is an I1 orphan, and the post-load
  // invariant check used to validate the whole global tree, so one evicted
  // document broke expanding any unrelated node afterwards.
  describe('structureTree consistency', () => {
    const cachedIds = (): Set<string> => new Set(store.getAllNodes().keys());

    const treeNode = (id: string, children: NodeWithChildren[] = []): NodeWithChildren => {
      const { id: _id, nodeType, content, version, createdAt, modifiedAt, properties } = createTestNode({ id });
      return { id, nodeType, content, version, createdAt, modifiedAt, properties, children };
    };

    /** Seed a cached document: `root` with `child-1` (itself with a child)
     * and `child-2`, edges registered the way a tree load registers them. */
    const seedDocument = (root: string): void => {
      for (const id of [root, `${root}-c1`, `${root}-c1-x`, `${root}-c2`]) {
        store.setNode(createTestNode({ id }), databaseSource);
      }
      structureTree.batchAddRelationships([
        { parentId: root, childId: `${root}-c1`, order: 1 },
        { parentId: `${root}-c1`, childId: `${root}-c1-x`, order: 1 },
        { parentId: root, childId: `${root}-c2`, order: 2 }
      ]);
    };

    afterEach(() => {
      vi.restoreAllMocks();
    });

    it('removes an evicted document from structureTree, so expanding an unrelated node afterwards loads cleanly', async () => {
      seedDocument('doc-a');
      store.setNode(createTestNode({ id: 'doc-b' }), databaseSource);
      store.setNode(createTestNode({ id: 'doc-b-heading' }), databaseSource);
      structureTree.addInMemoryRelationship('doc-b', 'doc-b-heading', 1);

      store.updateOpenDocumentRoots(['doc-a']);
      store.updateOpenDocumentRoots(['doc-b']); // doc-a's tab closed, doc-b opened

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode('doc-a')).toBeUndefined();
      expect(store.getNode('doc-a-c1-x')).toBeUndefined();
      expect(structureTree.getChildren('doc-a')).toEqual([]);
      expect(structureTree.getChildren('doc-a-c1')).toEqual([]);
      // The whole tree, not just doc-b's region, references only cached nodes.
      expect(() => structureTree.assertInvariants(cachedIds())).not.toThrow();

      // Expand a collapsed heading in doc-b: the lazy children load, with the
      // post-load invariant check the app runs outside tests.
      store.__setHierarchyInvariantChecksForTesting(true);
      vi.spyOn(backendAdapter, 'getChildrenTree').mockResolvedValue(
        treeNode('doc-b-heading', [treeNode('doc-b-item-1'), treeNode('doc-b-item-2')])
      );
      vi.spyOn(backendAdapter, 'getMentioningContainers').mockResolvedValue([]);

      await expect(store.loadChildrenTree('doc-b-heading')).resolves.toHaveLength(2);
      expect(structureTree.getChildren('doc-b-heading')).toEqual(['doc-b-item-1', 'doc-b-item-2']);
    });

    it('keeps a subtree still open in another tab, minus its edge to the evicted parent', async () => {
      seedDocument('doc-a');

      // Tab 1 shows doc-a, tab 2 is zoomed into doc-a-c1.
      store.updateOpenDocumentRoots(['doc-a', 'doc-a-c1']);
      store.updateOpenDocumentRoots(['doc-a-c1']); // tab 1 closed

      await wait(TEST_INACTIVITY_MS + 40);

      expect(store.getNode('doc-a')).toBeUndefined();
      expect(store.getNode('doc-a-c2')).toBeUndefined();
      expect(store.getNode('doc-a-c1')).toBeDefined();
      expect(store.getNode('doc-a-c1-x')).toBeDefined();

      // The still-open subtree keeps its own edges; its edge to the evicted
      // parent goes with the parent.
      expect(structureTree.getChildren('doc-a-c1')).toEqual(['doc-a-c1-x']);
      expect(structureTree.getParent('doc-a-c1')).toBeNull();
      expect(() => structureTree.assertInvariants(cachedIds())).not.toThrow();
    });

    it('defers evicting a parent while a cached child still has a pending write', async () => {
      seedDocument('doc-a');
      store.updateOpenDocumentRoots(['doc-a']);
      store.updateOpenDocumentRoots([]);

      let resolveWrite: () => void = () => {};
      const writeCompletes = new Promise<void>((resolve) => {
        resolveWrite = resolve;
      });
      SimplePersistenceCoordinator.getInstance().persist('doc-a-c2', () => writeCompletes, {
        mode: 'immediate'
      });

      await wait(TEST_INACTIVITY_MS + 40);

      // The child's pending CREATE resolves its parent from structureTree, so
      // the parent and that edge must still be there.
      expect(store.getNode('doc-a')).toBeDefined();
      expect(structureTree.getParent('doc-a-c2')).toBe('doc-a');
      // An unrelated, settled sibling subtree is evicted as usual.
      expect(store.getNode('doc-a-c1-x')).toBeUndefined();

      resolveWrite();
      await wait(2 * TEST_INACTIVITY_MS + 80);

      expect(store.getNode('doc-a-c2')).toBeUndefined();
      expect(store.getNode('doc-a')).toBeUndefined();
      expect(() => structureTree.assertInvariants(cachedIds())).not.toThrow();
    });

    it('a post-load invariant check ignores inconsistencies outside the loaded subtree', async () => {
      store.setNode(createTestNode({ id: 'doc-b' }), databaseSource);
      // A stale edge elsewhere in the tree, to a node no longer cached.
      structureTree.addInMemoryRelationship('unrelated-parent', 'uncached-child', 1);

      store.__setHierarchyInvariantChecksForTesting(true);
      vi.spyOn(backendAdapter, 'getChildrenTree').mockResolvedValue(
        treeNode('doc-b', [treeNode('doc-b-item-1')])
      );
      vi.spyOn(backendAdapter, 'getMentioningContainers').mockResolvedValue([]);

      await expect(store.loadChildrenTree('doc-b')).resolves.toHaveLength(1);
      // The whole-tree check still reports it.
      expect(() => structureTree.assertInvariants(cachedIds())).toThrow(/orphan childId "uncached-child"/);
    });
  });
});
