/**
 * Subtree-access-denied refusal: the optimistic delete removes the node from the
 * store BEFORE the backend call. When the daemon refuses the cascade delete
 * (ADR-041 access gate) the frontend must restore the optimistically-removed node
 * — nothing was actually deleted — and surface the refusal to the UI.
 *
 * A non-refusal error must keep today's behavior: the optimistic removal stands.
 *
 * Per project rules, we spy on the real `backendAdapter` singleton with
 * `vi.spyOn` (never `vi.mock` a singleton — it leaks across the forks pool) and
 * drive the real `SharedNodeStore` singleton.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { getSubtreeAccessDeniedState } from '../../lib/services/subtree-access-denied.svelte';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import { CASCADE_SETTLE_TIMEOUT_MS } from '../utils/test-constants';

const NODE_ID = 'a1b2c3d4-0000-4000-8000-000000000001';

const dbSource = { type: 'database' as const, reason: 'test-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'test-viewer' };

const makeNode = (overrides: Partial<Node> = {}): Node =>
  ({
    id: NODE_ID,
    nodeType: 'text',
    content: 'about to be deleted',
    createdAt: '2024-01-01T00:00:00.000Z',
    modifiedAt: '2024-01-01T00:00:00.000Z',
    version: 4,
    properties: {},
    ...overrides
  }) as Node;

const makeRefusalError = (inaccessibleCount: number) => ({
  message: `Delete refused: subtree contains ${inaccessibleCount} node(s) not accessible to the current actor`,
  code: 'SUBTREE_ACCESS_DENIED',
  details: 'FailedPrecondition',
  conflictData: { inaccessibleCount }
});

/**
 * Seed the store so `deleteNode` sees a known, persisted node. `setNode` from a
 * database source only marks a node persisted once the store has already seen it,
 * so this runs twice (mirrors the OCC regression test's seeding).
 */
function seedPersistedNode(store: SharedNodeStore): void {
  const node = makeNode();
  store.setNode(node, dbSource);
  store.setNode(node, dbSource);
}

describe('deleteNode subtree-access-denied restore', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    getSubtreeAccessDeniedState().dismiss();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    getSubtreeAccessDeniedState().dismiss();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  it('restores the optimistically-removed node and surfaces the refusal', async () => {
    seedPersistedNode(store);
    expect(store.getNode(NODE_ID)).toBeDefined();

    vi.spyOn(backendAdapter, 'deleteNode').mockRejectedValueOnce(makeRefusalError(3));

    store.deleteNode(NODE_ID, viewerSource);

    // The node is removed optimistically, then restored once the rejection lands.
    // The removal above is synchronous, so the store genuinely holds nothing for
    // this id when the wait starts — it can only pass on the restore itself.
    await vi.waitFor(() => expect(store.getNode(NODE_ID)).toBeDefined(), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    const restored = store.getNode(NODE_ID);
    expect(restored).toBeDefined();
    expect(restored?.version).toBe(4);

    // The refusal is surfaced to the globally-mounted modal with the count.
    const refusal = getSubtreeAccessDeniedState();
    expect(refusal.pending).not.toBeNull();
    expect(refusal.pending?.inaccessibleCount).toBe(3);

    // The specific subtree-access-denied modal is the ONLY notification for
    // this event — the outer handle.promise.catch() must not pile a second,
    // generic write-failure toast on top of it for the same refusal.
    const writeFailures = conflictNotifications.notifications.filter(
      (n) => n.nodeId === NODE_ID && n.conflictType === 'write-failure'
    );
    expect(writeFailures).toHaveLength(0);
  }, 3000);

  it('leaves the node deleted for a non-refusal backend error, and surfaces a write-failure notification', async () => {
    seedPersistedNode(store);

    vi.spyOn(backendAdapter, 'deleteNode').mockRejectedValueOnce(
      new Error('network unreachable')
    );

    store.deleteNode(NODE_ID, viewerSource);

    // This test asserts a NEGATIVE (the node must STAY deleted), so it cannot
    // wait on its own end state — the optimistic removal already made that true
    // synchronously, and the wait would return without the rejection having been
    // handled at all. The write-failure notification is the observable marker
    // that the error path actually ran.
    await vi.waitFor(
      () =>
        expect(
          conflictNotifications.notifications.filter(
            (n) => n.nodeId === NODE_ID && n.conflictType === 'write-failure'
          )
        ).toHaveLength(1),
      { timeout: CASCADE_SETTLE_TIMEOUT_MS }
    );

    // Non-refusal errors keep today's behavior: the optimistic removal stands.
    expect(store.getNode(NODE_ID)).toBeUndefined();
    expect(getSubtreeAccessDeniedState().pending).toBeNull();

    // Unlike a subtree-access-denied refusal (which has its own specific
    // notification), a plain deletion failure has no other signal — it must
    // surface a generic write-failure notification, not be silently
    // dropped (the previous behavior for this catch site).
    const writeFailures = conflictNotifications.notifications.filter(
      (n) => n.nodeId === NODE_ID && n.conflictType === 'write-failure'
    );
    expect(writeFailures).toHaveLength(1);
  }, 3000);

  it('invokes onRefused on a refusal so a caller can restore its own view state', async () => {
    // The reactive view service also removes the node from its own `_rootNodeIds`
    // (the top-level view's source of truth), which the store can't reach — so on a
    // refusal the store calls this callback to let that layer restore too.
    seedPersistedNode(store);
    const onRefused = vi.fn();

    vi.spyOn(backendAdapter, 'deleteNode').mockRejectedValueOnce(makeRefusalError(2));
    store.deleteNode(NODE_ID, viewerSource, false, [], onRefused);

    // Wait on the callback under test — it is unconditionally uncalled when the
    // wait starts, so this can only pass once the refusal has been handled.
    await vi.waitFor(() => expect(onRefused).toHaveBeenCalledTimes(1), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    expect(onRefused).toHaveBeenCalledTimes(1);
  }, 3000);

  it('does NOT invoke onRefused for a non-refusal error', async () => {
    seedPersistedNode(store);
    const onRefused = vi.fn();

    vi.spyOn(backendAdapter, 'deleteNode').mockRejectedValueOnce(new Error('network unreachable'));
    store.deleteNode(NODE_ID, viewerSource, false, [], onRefused);

    // Another NEGATIVE assertion (`onRefused` must not fire), which is trivially
    // true before the rejection is handled. Wait on the write-failure
    // notification — raised only by the non-refusal branch — so the assertion
    // below runs against a rejection that has actually been processed.
    await vi.waitFor(
      () =>
        expect(
          conflictNotifications.notifications.filter(
            (n) => n.nodeId === NODE_ID && n.conflictType === 'write-failure'
          )
        ).toHaveLength(1),
      { timeout: CASCADE_SETTLE_TIMEOUT_MS }
    );

    expect(onRefused).not.toHaveBeenCalled();
  }, 3000);
});
