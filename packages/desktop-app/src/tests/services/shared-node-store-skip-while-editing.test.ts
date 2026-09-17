/**
 * Tests for the skip-while-editing guard in SharedNodeStore.setNode().
 *
 * Repro context: typing corruption and Enter relocating text. Root cause:
 * daemon broadcasts of just-confirmed writes
 * arrive via the WatchNodes gRPC stream while the user is still typing.
 * The unguarded `setNode()` clobbers the optimistic store with the older
 * server-confirmed state.
 *
 * The guard skips the clobber when source.type === 'database' AND the
 * node is actively focused OR has unsaved local changes pending.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { focusManager } from '../../lib/services/focus-manager.svelte';
import { structureTree } from '../../lib/stores/reactive-structure-tree.svelte';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';
import { CASCADE_SETTLE_TIMEOUT_MS } from '../utils/test-constants';

describe('SharedNodeStore — skip-while-editing guard', () => {
  let store: SharedNodeStore;

  const makeNode = (id: string, content: string, version = 1): Node => ({
    id,
    nodeType: 'text',
    content,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version,
    properties: {},
    mentions: []
  });

  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-1'
  };

  const databaseSource: UpdateSource = {
    type: 'database',
    reason: 'domain-event'
  };

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    focusManager.clearEditing();
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  it('skips clobbering the content of a focused node on a database event', () => {
    // User-typed optimistic state
    const optimisticNode = makeNode('n1', 'hello world', 1);
    store.setNode(optimisticNode, viewerSource);

    // User focuses the node (actively editing)
    focusManager.focusNode('n1', 'default');

    // Daemon broadcast lands with the older confirmed content
    const stalerNode = makeNode('n1', 'hell', 2);
    store.setNode(stalerNode, databaseSource);

    // Local content stays at the user's optimistic state. Crucially the
    // local node's `.version` is also unchanged — mutating it inside the
    // reactive Map would cause Svelte to re-render and remount the focused
    // textarea (resetting selectionStart → triggers the text-relocation bug).
    const after = store.getNode('n1');
    expect(after?.content).toBe('hello world');
    expect(after?.version).toBe(1);
  });

  it('skips clobbering when the node has pending persistence even if unfocused', () => {
    // Plant a node and put it into the user-edit path so the persistence
    // coordinator is engaged with a debounced write for it.
    const initial = makeNode('n2', 'initial', 1);
    store.setNode(initial, viewerSource);

    // Trigger a viewer-side update that schedules a debounced persist.
    // Persistence stays in the "pending" bucket because we don't flush.
    store.updateNode('n2', { content: 'user-typing-this' }, viewerSource);

    // Daemon broadcast lands with the older confirmed content.
    const stalerNode = makeNode('n2', 'initial', 2);
    store.setNode(stalerNode, databaseSource);

    // Optimistic content survives the broadcast; local .version untouched.
    const after = store.getNode('n2');
    expect(after?.content).toBe('user-typing-this');
    expect(after?.version).toBe(1);
  });

  it('does apply database events for non-focused, non-dirty nodes (regression check)', () => {
    // Seed via the database path so no persistence is scheduled — mirrors
    // the production case where the node arrived from the daemon and the
    // user hasn't touched it yet.
    const initial = makeNode('n3', 'before', 1);
    store.setNode(initial, databaseSource);

    // No focus, no pending writes → genuine remote update should land.
    const remoteUpdate = makeNode('n3', 'after', 5);
    store.setNode(remoteUpdate, databaseSource);

    expect(store.getNode('n3')?.content).toBe('after');
    expect(store.getNode('n3')?.version).toBe(5);
  });

  it('does apply viewer-source updates to a focused node (user actions are authoritative)', () => {
    const initial = makeNode('n4', 'before', 1);
    store.setNode(initial, viewerSource);
    focusManager.focusNode('n4', 'default');

    // The user themselves is the source — this is their own typed change.
    const userEdit = makeNode('n4', 'after', 1);
    store.setNode(userEdit, viewerSource);

    expect(store.getNode('n4')?.content).toBe('after');
  });

  it('does apply database events when the node has never been seen locally', () => {
    // First time the local store sees this node — the guard's "existingNode"
    // check ensures we still accept the new state.
    focusManager.focusNode('n5', 'default'); // even with focus on the id
    const incoming = makeNode('n5', 'fresh from cloud', 1);
    store.setNode(incoming, databaseSource);

    expect(store.getNode('n5')?.content).toBe('fresh from cloud');
  });

  it('preserves the optimistic content AND leaves local version untouched (no reactive mutation)', () => {
    const optimistic = makeNode('n6', 'local-newer', 3);
    store.setNode(optimistic, viewerSource);
    focusManager.focusNode('n6', 'default');

    const broadcast = makeNode('n6', 'cloud-older', 7);
    store.setNode(broadcast, databaseSource);

    const after = store.getNode('n6');
    expect(after?.content).toBe('local-newer'); // content preserved
    expect(after?.version).toBe(3); // local version NOT touched
  });

  it('never stashes a broadcast version, preserving OCC for every database-sourced event (ADR-026 C5 extension)', () => {
    // Before the ADR-026 C5 extension, the guard tried to distinguish "my own write
    // echoing back" from "a different client's write" by comparing the
    // broadcast's content against what this client last sent, and stashed
    // the broadcast version only for a plausible own-echo. The daemon now
    // suppresses a connection's own write echoes before they ever reach
    // WatchNodes (`packages/daemon/src/services/node_service.rs`), so every
    // database-sourced broadcast the frontend receives is guaranteed
    // foreign — there is no case left where stashing a broadcast version is
    // safe, and the guard never does so. The next UpdateNode always carries
    // this client's own local version, so a real conflict surfaces via OCC
    // instead of silently overwriting the foreign write.
    const optimistic = makeNode('foreign', 'alice typed this', 3);
    store.setNode(optimistic, viewerSource);
    focusManager.focusNode('foreign', 'default');

    const foreignBroadcast = makeNode('foreign', 'bob wrote something else', 9);
    store.setNode(foreignBroadcast, databaseSource);

    // Local content is preserved (guard still fired — we keep optimistic).
    expect(store.getNode('foreign')?.content).toBe('alice typed this');
    expect(store.getNode('foreign')?.version).toBe(3);
    // The next UpdateNode still uses the local version — an OCC conflict
    // against the backend's v9 will surface it rather than silently losing it.
    expect(store.computeOccVersionForUpdate('foreign')).toBe(3);
  });

  it('preserves insertAfterNodeId when structureTree agrees on parent', () => {
    // The persistence-time stale-sibling check consults `structureTree`
    // (the authoritative source for hierarchy via has_child edges) so the
    // hint survives whenever the tree confirms the same parent.
    // Node.parentId no longer exists — structureTree is the only source.
    //
    // Tests the decision via `shouldClearStaleInsertAfter`, the helper
    // the persistence closure calls — locks in the production code path
    // without mocking the Tauri-side IPC.
    structureTree.clear();
    const existingA = makeNode('a', 'existing', 1);
    store.setNode(existingA, databaseSource);
    structureTree.addChild({ parentId: 'D', childId: 'a', order: 1 });

    // structureTree says sibling 'a' is under 'D'. New node 'b' is being
    // inserted with currentParentId='D'. The hint must be preserved.
    expect(store.shouldClearStaleInsertAfter('a', 'D')).toBe(false);

    // Sanity: if structureTree disagrees, the hint IS cleared.
    expect(store.shouldClearStaleInsertAfter('a', 'OTHER_PARENT')).toBe(true);

    // Sanity: if structureTree has no opinion, the hint is preserved
    // (backend retry loop will handle it).
    expect(store.shouldClearStaleInsertAfter('unknown', 'D')).toBe(false);

    structureTree.clear();
  });

  it('applies the next database event normally once the user blurs (guard only fires while actively editing)', () => {
    store.setNode(makeNode('n7', 'seed', 3), databaseSource);

    // Focus the node so the next database event hits the guard.
    focusManager.focusNode('n7', 'default');
    store.setNode(makeNode('n7', 'older', 9), databaseSource);
    // Verify the local view didn't change (guard fired).
    expect(store.getNode('n7')?.content).toBe('seed');
    expect(store.getNode('n7')?.version).toBe(3);

    // User blurs. Subsequent database event no longer matches the guard;
    // the normal setNode path runs and writes the new state.
    focusManager.clearEditing();
    store.setNode(makeNode('n7', 'cloud-current', 15), databaseSource);

    const after = store.getNode('n7');
    expect(after?.content).toBe('cloud-current');
    expect(after?.version).toBe(15);
  });

  // batchSetNodes (used by doLoadChildrenTree with a database source)
  // must apply the SAME skip-while-editing guard as setNode — a concurrent tree
  // reload would otherwise overwrite a child being edited mid-keystroke.
  describe('batchSetNodes guard', () => {
    it('skips clobbering a focused node in a database-source batch', () => {
      const optimistic = makeNode('b1', 'hello world', 1);
      store.setNode(optimistic, viewerSource);
      focusManager.focusNode('b1', 'default');

      // A tree (re)load batch lands with the older confirmed snapshot.
      store.batchSetNodes([makeNode('b1', 'hell', 2)], databaseSource);

      const after = store.getNode('b1');
      expect(after?.content).toBe('hello world');
      expect(after?.version).toBe(1);
    });

    it('skips clobbering a node with pending persistence in a batch even if unfocused', () => {
      store.setNode(makeNode('b2', 'initial', 1), viewerSource);
      store.updateNode('b2', { content: 'user-typing-this' }, viewerSource);

      store.batchSetNodes([makeNode('b2', 'initial', 2)], databaseSource);

      const after = store.getNode('b2');
      expect(after?.content).toBe('user-typing-this');
      expect(after?.version).toBe(1);
    });

    it('applies database-source batch updates for non-focused, non-dirty nodes (regression)', () => {
      store.setNode(makeNode('b3', 'before', 1), databaseSource);

      store.batchSetNodes([makeNode('b3', 'after', 5)], databaseSource);

      expect(store.getNode('b3')?.content).toBe('after');
      expect(store.getNode('b3')?.version).toBe(5);
    });

    it('guards only the edited node, applying the rest of the batch', () => {
      // b4 is being edited; b5 is untouched. One batch reload touches both.
      store.setNode(makeNode('b4', 'editing-this', 1), viewerSource);
      focusManager.focusNode('b4', 'default');
      store.setNode(makeNode('b5', 'old', 1), databaseSource);

      store.batchSetNodes(
        [makeNode('b4', 'stale', 2), makeNode('b5', 'new', 7)],
        databaseSource
      );

      // b4 preserved (guard fired), b5 applied (no guard).
      expect(store.getNode('b4')?.content).toBe('editing-this');
      expect(store.getNode('b4')?.version).toBe(1);
      expect(store.getNode('b5')?.content).toBe('new');
      expect(store.getNode('b5')?.version).toBe(7);
    });
  });

  // A foreign write to an actively-edited node is skipped to protect the
  // optimistic text — but it must NOT be silent. Surface a version-mismatch
  // conflict notification so the user knows another writer changed the node.
  describe('foreign-write conflict signal', () => {
    const versionMismatchFor = (nodeId: string) =>
      conflictNotifications.notifications.filter(
        (n) => n.nodeId === nodeId && n.conflictType === 'version-mismatch'
      );

    it('raises a version-mismatch notification when a foreign broadcast hits a focused node', () => {
      store.setNode(makeNode('shared', 'hello world', 5), viewerSource);
      focusManager.focusNode('shared', 'default');

      // Bob's foreign write.
      store.setNode(makeNode('shared', 'totally different text', 7), databaseSource);

      expect(versionMismatchFor('shared')).toHaveLength(1);
      // Optimistic content still protected (the clobber was skipped).
      expect(store.getNode('shared')?.content).toBe('hello world');
    });

    it('dedupes repeated foreign broadcasts for the same node', () => {
      store.setNode(makeNode('dup', 'hello world', 5), viewerSource);
      focusManager.focusNode('dup', 'default');

      store.setNode(makeNode('dup', 'foreign one', 7), databaseSource);
      store.setNode(makeNode('dup', 'foreign two', 8), databaseSource);

      expect(versionMismatchFor('dup')).toHaveLength(1);
    });

    // ADR-026's C5 extension: the daemon suppresses a connection's own write echoes
    // before they ever reach WatchNodes, so the frontend never receives a
    // database-sourced broadcast that is its own echo — there is no "own
    // echo, don't notify" case left to test here. Every database-sourced
    // broadcast reaching an actively-edited node is a genuine foreign write
    // and always raises the conflict notification, including one whose
    // content happens to be identical to what the client last saw (the old
    // content-comparison heuristic would have misclassified this as an echo
    // — the daemon's authoritative signal does not need content at all).
    it('still notifies even when the broadcast content is identical to what was last seen (no content-based echo guessing)', () => {
      store.setNode(makeNode('hydrated', 'hello world', 5), databaseSource);
      focusManager.focusNode('hydrated', 'default');

      store.setNode(makeNode('hydrated', 'hello world', 6), databaseSource);

      expect(versionMismatchFor('hydrated')).toHaveLength(1);
    });

    it('notifies on a genuine foreign write to a focused node', () => {
      store.setNode(makeNode('hydrated2', 'hello world', 5), databaseSource);
      focusManager.focusNode('hydrated2', 'default');

      store.setNode(makeNode('hydrated2', 'a foreign edit', 7), databaseSource);

      expect(versionMismatchFor('hydrated2')).toHaveLength(1);
      // The local content is still protected (the clobber was skipped).
      expect(store.getNode('hydrated2')?.content).toBe('hello world');
    });
  });

  // resyncNodeFromServer() writes the fetched row via the same guard-free
  // path a `database`-sourced setNode() would, but historically bypassed
  // the skip-while-editing guard entirely (it calls the private `nodesSet`
  // directly). It's the fallback `updateNode()` uses when an OCC (version-
  // conflict) failure arrives without an embedded authoritative node —
  // rarer than an ordinary write failure, but a real user could still be
  // actively typing in the conflicting node when one lands, and an
  // unguarded resync would silently revert their in-progress edit to
  // whatever the server had at conflict time.
  //
  // Guards on `isFocused` only — deliberately NOT `hasPending`, unlike
  // `setNode()`'s use of the same underlying policy. `PersistenceCoordinator
  // .hasPending()` is true while an operation is executing, and there is
  // only one executing slot per node: when resyncNodeFromServer is called
  // from a failed write's own catch handler, that very write still occupies
  // it (its `finally` hasn't run yet) — so `hasPending` at that moment is
  // this failing write's own not-yet-cleared bookkeeping, not evidence of a
  // genuinely different pending edit. That's self-referential and racy
  // (depends on mock-vs-real network timing), and checking it would defeat
  // this recovery path for its single most common case: an isolated
  // failure with nothing else in flight. `isFocused` has no such
  // self-reference and covers the scenario the guard exists for. A separate,
  // non-racy check (the fetched-node identity comparison inside
  // resyncNodeFromServer itself) additionally bails if some other write
  // landed for this node while the fetch was in flight.
  describe('resyncNodeFromServer guard', () => {
    it("does not clobber a focused node's optimistic content with a stale server snapshot", async () => {
      store.setNode(makeNode('resync-1', 'server-old', 1), databaseSource);
      focusManager.focusNode('resync-1', 'default');
      // User keeps typing locally (optimistic, not yet confirmed).
      store.updateNode('resync-1', { content: 'user is typing this' }, viewerSource, {
        skipPersistence: true
      });

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('resync-1', 'server-old', 1));

      await store.resyncNodeFromServer('resync-1');

      const after = store.getNode('resync-1');
      expect(after?.content).toBe('user is typing this');
    });

    it('still marks the node as persisted on a guard-skip, mirroring setNode()', async () => {
      // A node whose local persisted-bookkeeping has drifted (e.g. after a
      // reload) but is actively focused — the skip must protect content,
      // not silently leave persistedNodeIds out of sync too.
      store.setNode(makeNode('resync-1b', 'server-old', 1), viewerSource, true);
      focusManager.focusNode('resync-1b', 'default');

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('resync-1b', 'server-old', 1));

      expect(store.isNodePersisted('resync-1b')).toBe(false);
      await store.resyncNodeFromServer('resync-1b');
      expect(store.isNodePersisted('resync-1b')).toBe(true);
    });

    it('does not clobber a different write that lands locally while the fetch is in flight', async () => {
      // Not a focus/pending scenario — this is the identity-based guard:
      // regardless of focus state, if the node this resync is about to
      // apply to has been replaced by ANY other write since the fetch
      // started, the fetch reflects state from before that write and must
      // not overwrite it.
      store.setNode(makeNode('resync-race', 'server-old', 1), databaseSource);

      let resolveFetch!: (node: Node) => void;
      vi.spyOn(backendAdapter, 'getNode').mockImplementation(
        () => new Promise((resolve) => (resolveFetch = resolve))
      );

      const resyncPromise = store.resyncNodeFromServer('resync-race');

      // A second, unrelated write lands for the same node while the fetch
      // above is still pending.
      store.updateNode(
        'resync-race',
        { content: 'a different write landed' },
        viewerSource,
        { skipPersistence: true }
      );

      // The fetch now resolves with what the server had *before* that
      // second write happened.
      resolveFetch(makeNode('resync-race', 'server-old', 1));
      await resyncPromise;

      expect(store.getNode('resync-race')?.content).toBe('a different write landed');
    });

    // Documents the scope boundary above, rather than asserting protection
    // that (deliberately) doesn't exist here: an unfocused node with an
    // unrelated pending debounced write is NOT shielded from a resync by
    // this guard — only `setNode()` gets that (via the real, non-racy
    // `hasPending` check, safe there because setNode is never called from
    // inside the very operation it might be racing against).
    it('does NOT protect a merely-pending (not focused) node — known scope limit, see guard comment above', async () => {
      store.setNode(makeNode('resync-2', 'server-old', 1), databaseSource);
      store.updateNode('resync-2', { content: 'debounced-edit-in-flight' }, viewerSource);

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('resync-2', 'server-fresh', 5));

      await store.resyncNodeFromServer('resync-2');

      expect(store.getNode('resync-2')?.content).toBe('server-fresh');
    });

    it('still resyncs normally for a non-focused, non-pending node (regression check)', async () => {
      store.setNode(makeNode('resync-3', 'local-stale', 1), databaseSource);

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('resync-3', 'server-fresh', 9));

      await store.resyncNodeFromServer('resync-3');

      const after = store.getNode('resync-3');
      expect(after?.content).toBe('server-fresh');
      expect(after?.version).toBe(9);
    });

    it('applies normally once the user blurs (guard only fires while actively editing)', async () => {
      store.setNode(makeNode('resync-4', 'server-old', 1), databaseSource);
      focusManager.focusNode('resync-4', 'default');

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('resync-4', 'server-fresh', 4));

      // Still focused — the resync must be skipped.
      await store.resyncNodeFromServer('resync-4');
      expect(store.getNode('resync-4')?.content).toBe('server-old');

      // User blurs — the same resync call now applies normally.
      focusManager.clearEditing();
      await store.resyncNodeFromServer('resync-4');
      expect(store.getNode('resync-4')?.content).toBe('server-fresh');
    });
  });

  // `updateNode()`'s OCC-conflict catch handler has two hydration paths: a
  // DIRECT one when the daemon embeds `current_node` in the conflict payload
  // (`this.nodesSet(nodeId, currentNode)`), and a FALLBACK
  // (`resyncNodeFromServer`, tested above) when it doesn't. The
  // skip-while-editing guard was added to the fallback only — the direct path wrote
  // straight into the store with no equivalent check. This block is the
  // direct-path counterpart to the `resyncNodeFromServer guard` block above.
  describe('OCC direct-hydration guard', () => {
    const makeVersionConflictError = (currentNode: Node | null) => ({
      message: 'Version conflict',
      code: 'VERSION_CONFLICT' as const,
      details: 'Aborted',
      conflictData: {
        node_id: currentNode?.id ?? 'unknown',
        expected: 1,
        actual: 2,
        current_node: currentNode
      }
    });

    // Mirrors ai-chat-occ-conflict-regression.test.ts's seeding helper:
    // `setNode` only marks a database-sourced node persisted once the store
    // has already seen it, so this must happen twice — otherwise the write
    // routes to `createNode` and the mocked `updateNode` rejection never
    // fires (the OCC catch handler under test is on the UPDATE path only).
    function seedPersisted(id: string, content: string, version = 1): void {
      const node = makeNode(id, content, version);
      store.setNode(node, databaseSource);
      store.setNode(node, databaseSource);
    }

    it("does not clobber a focused node's optimistic content when the OCC conflict response embeds current_node", async () => {
      seedPersisted('occ-1', 'server-old', 1);
      focusManager.focusNode('occ-1', 'default');

      const daemonCurrentNode = makeNode('occ-1', 'daemon-conflict-content', 2);
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateNode('occ-1', { content: 'user is typing this', properties: {} }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, 50));

      const after = store.getNode('occ-1');
      expect(after?.content).toBe('user is typing this');
      // Local `.version` is also untouched — same reactivity-safety reason
      // as the setNode guard above (mutating it would remount a focused
      // editor mid-keystroke).
      expect(after?.version).toBe(1);
    });

    it('still marks the node as persisted on a guard-skip, mirroring resyncNodeFromServer', async () => {
      seedPersisted('occ-2', 'server-old', 1);
      focusManager.focusNode('occ-2', 'default');

      const daemonCurrentNode = makeNode('occ-2', 'daemon-conflict-content', 2);
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateNode('occ-2', { content: 'still typing', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(store.isNodePersisted('occ-2')).toBe(true);
    });

    it('still applies the OCC hydration normally for a non-focused, non-pending node (regression check)', async () => {
      seedPersisted('occ-3', 'server-old', 1);
      // Not focused, nothing else pending.

      const daemonCurrentNode = makeNode('occ-3', 'daemon-conflict-content', 2);
      const getNodeSpy = vi.spyOn(backendAdapter, 'getNode');
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateNode(
        'occ-3',
        { content: 'a write from this client', properties: {} },
        viewerSource
      );
      await new Promise((resolve) => setTimeout(resolve, 50));

      const after = store.getNode('occ-3');
      expect(after?.content).toBe('daemon-conflict-content');
      expect(after?.version).toBe(2);
      // Direct-hydration path applied — the fallback (resyncNodeFromServer,
      // which calls backendAdapter.getNode) never fires.
      expect(getNodeSpy).not.toHaveBeenCalled();
    });

    it('raises exactly one version-mismatch conflict notification on a guard-skip', async () => {
      seedPersisted('occ-4', 'server-old', 1);
      focusManager.focusNode('occ-4', 'default');

      const daemonCurrentNode = makeNode('occ-4', 'daemon-conflict-content', 2);
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateNode('occ-4', { content: 'user is typing this', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 50));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 'occ-4' && n.conflictType === 'version-mismatch'
      );
      expect(notifications).toHaveLength(1);
    });

    // #2069 fixed resyncNodeFromServer's direct call (the fallback branch
    // above this guard) to stop discarding a genuinely queued second write.
    // This guard's `hasPending` was the identical hardcoded-`false` bug on
    // the DIRECT-HYDRATION branch (this describe block) — sharing the exact
    // same `hadQueuedWrite` value the OCC handler now captures before
    // `clearQueued()` runs, just previously not wired into this branch's own
    // `decideRemoteUpdate` call.
    it('does not discard a genuinely queued second write when the daemon embeds current_node (direct-hydration branch)', async () => {
      seedPersisted('occ-5', 'seed', 1);
      // Not focused — the ONLY thing protecting the queued write here is the
      // hasPending signal, not the isFocused one already covered above.

      // Write A's RPC must not settle until write B has landed and collapsed
      // into queuedOperations. A real-time delay would only be a guess that B
      // gets there first; this gate makes the ordering explicit and costs no
      // wall-clock time.
      let releaseWriteA!: () => void;
      const writeAGate = new Promise<void>((resolve) => (releaseWriteA = resolve));

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, version, node) => {
        updateCallCount++;
        if (updateCallCount === 1) {
          // Write A: held open until the test releases it, guaranteeing B is
          // queued while A is still executing.
          await writeAGate;
          throw makeVersionConflictError(makeNode('occ-5', 'daemon-conflict-content', 2));
        }
        // Write B's own eventual real persist attempt, once promoted.
        return {
          id: 'occ-5',
          nodeType: 'text',
          content: String(node.content ?? ''),
          createdAt: '2024-01-01T00:00:00.000Z',
          modifiedAt: new Date().toISOString(),
          version: version + 1,
          properties: {},
          mentions: []
        };
      });

      // Write A: a property change -> immediate mode, starts executing
      // synchronously (no debounce wait needed to get it "in flight").
      store.updateNode('occ-5', { content: 'A-edit', properties: {} }, viewerSource);

      // Write B: submitted synchronously while A is still executing (A's
      // mocked RPC hasn't settled yet) -> collapses into queuedOperations.
      // B's optimistic content lands immediately.
      store.updateNode('occ-5', { content: 'B-edit-still-queued', properties: {} }, viewerSource);
      expect(store.getNode('occ-5')?.content).toBe('B-edit-still-queued');

      // B is queued; let A's RPC fail now.
      releaseWriteA();

      // This test asserts a NEGATIVE (B's value must survive), so it cannot
      // wait on its own end state — that already holds the moment B's
      // optimistic write lands, and the wait would return without the
      // direct-hydration branch ever having run. Wait instead on markers that
      // A's OCC failure was actually processed: the conflict notification it
      // raises, plus the coordinator going idle so the hydrate-or-skip
      // decision has definitely run before the assertion below.
      await vi.waitFor(
        () => {
          expect(
            conflictNotifications.notifications.filter(
              (n) => n.nodeId === 'occ-5' && n.conflictType === 'version-mismatch'
            ).length
          ).toBeGreaterThanOrEqual(1);
          expect(store.isNodePersistenceExecuting('occ-5')).toBe(false);
          expect(store.hasPendingSave('occ-5')).toBe(false);
        },
        { timeout: CASCADE_SETTLE_TIMEOUT_MS }
      );

      // B's optimistic value must not have been silently clobbered by the
      // direct-hydration branch's stale (pre-B) conflict payload.
      expect(store.getNode('occ-5')?.content).toBe('B-edit-still-queued');
    }, 10000);
  });

  // updateTaskNode()'s OCC-conflict catch handler has the identical
  // direct-hydration shape updateNode() had before it was fixed: a DIRECT path when
  // the daemon embeds `current_node` in the conflict payload
  // (`this.nodesSet(nodeId, currentNode)`), unguarded against a node the
  // user is actively editing. This block is the task-node counterpart to the
  // `OCC direct-hydration guard` block above.
  describe('updateTaskNode OCC direct-hydration guard', () => {
    type TaskLikeNode = Node & {
      status: string;
      priority?: string;
      dueDate?: string | null;
      startedAt?: string | null;
      completedAt?: string | null;
    };

    const makeTaskNode = (id: string, content: string, version = 1): TaskLikeNode => ({
      id,
      nodeType: 'task',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version,
      properties: {},
      mentions: [],
      status: 'todo'
    });

    const makeVersionConflictError = (currentNode: TaskLikeNode | null) => ({
      message: 'Version conflict',
      code: 'VERSION_CONFLICT' as const,
      details: 'Aborted',
      conflictData: {
        node_id: currentNode?.id ?? 'unknown',
        expected: 1,
        actual: 2,
        current_node: currentNode
      }
    });

    // Same double-setNode rationale as seedPersisted above, adapted for a
    // task node: the first call marks it seen, the second marks it
    // persisted, so updateTaskNode's own persist attempt goes through
    // backendAdapter.updateTaskNode (the path under test) rather than a
    // create fallback.
    function seedPersistedTask(id: string, content: string, version = 1): void {
      const node = makeTaskNode(id, content, version);
      store.setNode(node, databaseSource);
      store.setNode(node, databaseSource);
    }

    it("does not apply the daemon's conflicting current_node onto a focused task node", async () => {
      seedPersistedTask('t-occ-1', 'server-old', 1);
      focusManager.focusNode('t-occ-1', 'default');

      const daemonCurrentNode = makeTaskNode('t-occ-1', 'daemon-conflict-content', 2);
      daemonCurrentNode.status = 'done';
      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateTaskNode('t-occ-1', { status: 'in-progress' }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, 50));

      const after = store.getNode('t-occ-1') as unknown as TaskLikeNode | undefined;
      // As of #2088, updateTaskNode()'s catch handler matches updateNode()'s:
      // it leaves the optimistic edit in place on any failure (no unconditional
      // revert to the pre-edit existingNode snapshot) — so the guard here is
      // protecting the user's own in-progress optimistic edit ("in-progress")
      // against the DAEMON's conflicting snapshot landing on top of it while
      // the node is still being edited. What must hold: the daemon's
      // conflicting version/status ("done", v2) must not apply while focused —
      // that would silently advance the node's version out from under an
      // in-progress edit.
      expect(after?.status).not.toBe('done');
      expect(after?.version).not.toBe(2);
      expect(after?.status).toBe('in-progress');
      expect(after?.version).toBe(1);
    });

    it('still marks the task node as persisted on a guard-skip, mirroring updateNode', async () => {
      seedPersistedTask('t-occ-2', 'server-old', 1);
      focusManager.focusNode('t-occ-2', 'default');

      const daemonCurrentNode = makeTaskNode('t-occ-2', 'daemon-conflict-content', 2);
      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateTaskNode('t-occ-2', { status: 'in-progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(store.isNodePersisted('t-occ-2')).toBe(true);
    });

    it('still applies the OCC hydration normally for a non-focused, non-pending task node (regression check)', async () => {
      seedPersistedTask('t-occ-3', 'server-old', 1);
      // Not focused, nothing else pending.

      const daemonCurrentNode = makeTaskNode('t-occ-3', 'daemon-conflict-content', 2);
      daemonCurrentNode.status = 'done';
      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateTaskNode('t-occ-3', { status: 'in-progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 50));

      const after = store.getNode('t-occ-3') as unknown as TaskLikeNode | undefined;
      expect(after?.status).toBe('done');
      expect(after?.version).toBe(2);
    });

    it('raises exactly one version-mismatch conflict notification on a task-node guard-skip', async () => {
      seedPersistedTask('t-occ-4', 'server-old', 1);
      focusManager.focusNode('t-occ-4', 'default');

      const daemonCurrentNode = makeTaskNode('t-occ-4', 'daemon-conflict-content', 2);
      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makeVersionConflictError(daemonCurrentNode)
      );

      store.updateTaskNode('t-occ-4', { status: 'in-progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 50));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 't-occ-4' && n.conflictType === 'version-mismatch'
      );
      expect(notifications).toHaveLength(1);
    });

    // #2088: updateTaskNode()'s catch handler used to do an unconditional
    // `this.nodesSet(nodeId, existingNode)` BEFORE this guard ever ran (on
    // ANY failure, OCC or not) — unlike updateNode(), whose rollbackUpdate()
    // never touches node content. A write A failure could clobber a later
    // write B's still-queued optimistic value regardless of what this
    // guard's own hasPending-aware decision would have done. Fixed by
    // removing the unconditional revert (mirroring rollbackUpdate()'s
    // no-op-on-content behavior); this test proves write B's value survives
    // both A's failure-path handling and the direct-hydration guard's own
    // skip decision.
    it('#2088: does not discard a genuinely queued second write when an earlier write fails (unconditional existingNode rollback clobbers it)', async () => {
      const nodeId = 't-occ-queued-1';
      seedPersistedTask(nodeId, 'server-old', 1);
      // Not focused: hasPending (write B still queued) is the mechanism
      // under test here, not the isFocused guard covered above.

      // Same gate as the updateNode counterpart above: A's RPC is held open
      // until B has landed, making the ordering explicit instead of guessing
      // it with a real-time delay.
      let releaseWriteA!: () => void;
      const writeAGate = new Promise<void>((resolve) => (releaseWriteA = resolve));

      let updateCallCount = 0;
      vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(async () => {
        updateCallCount++;
        if (updateCallCount === 1) {
          // Write A: held open until the test releases it, guaranteeing B is
          // queued while A is still executing.
          await writeAGate;
          const daemonCurrentNode = makeTaskNode(nodeId, 'daemon-conflict-content', 2);
          daemonCurrentNode.status = 'done';
          throw makeVersionConflictError(daemonCurrentNode);
        }
        // `clearQueued()` (called unconditionally inside the OCC branch,
        // before the hydration decision) cancels write B's queued retry —
        // it must never actually re-attempt its own persist.
        throw new Error('write B should have been cancelled via clearQueued(), not re-attempted');
      });

      // Write A: immediate mode, starts executing synchronously.
      store.updateTaskNode(nodeId, { status: 'in-progress' }, viewerSource);

      // Write B: submitted synchronously while A is still executing ->
      // collapses into queuedOperations. B's optimistic status lands
      // immediately.
      store.updateTaskNode(nodeId, { status: 'done-by-b' }, viewerSource);
      expect((store.getNode(nodeId) as unknown as TaskLikeNode).status).toBe('done-by-b');

      // B is queued; let A's RPC fail now.
      releaseWriteA();

      // Another NEGATIVE assertion (B's value must survive), so waiting on
      // the end state would return immediately and prove nothing. Wait on
      // evidence that A's OCC failure was handled — the version-mismatch
      // notification it raises — and that the coordinator has gone idle, so
      // the (now-removed) revert and the guard's skip decision have both had
      // their chance to run before the assertions below.
      await vi.waitFor(
        () => {
          expect(
            conflictNotifications.notifications.filter(
              (n) => n.nodeId === nodeId && n.conflictType === 'version-mismatch'
            ).length
          ).toBeGreaterThanOrEqual(1);
          expect(store.isNodePersistenceExecuting(nodeId)).toBe(false);
          expect(store.hasPendingSave(nodeId)).toBe(false);
        },
        { timeout: CASCADE_SETTLE_TIMEOUT_MS }
      );

      const after = store.getNode(nodeId) as unknown as TaskLikeNode;
      // B's optimistic value must survive both A's failure-path handling and
      // the direct-hydration guard correctly skipping the daemon's stale
      // (pre-B) conflict snapshot.
      expect(after.status).toBe('done-by-b');
      expect(after.status).not.toBe('todo'); // A's stale pre-edit snapshot
      expect(after.status).not.toBe('done'); // the daemon's conflicting snapshot
      expect(updateCallCount).toBe(1);
    });
  });
});
