/**
 * OCC/subtree-access-denied errors lose their shape before reaching the
 * outer `handle.promise.catch()`.
 *
 * The real command errors crossing the Tauri/gRPC boundary (VERSION_CONFLICT,
 * SUBTREE_ACCESS_DENIED — see `isVersionConflict`/`isSubtreeAccessDenied` in
 * `$lib/types/errors`) are plain objects, not `instanceof Error`. Every
 * `PersistenceCoordinator.persist()` closure's own `catch (dbError)` wraps
 * whatever it re-throws via `dbError instanceof Error ? dbError : new
 * Error(String(dbError))` — losing `.code`/`.conflictData` entirely — before
 * `throw error`. The OUTER `handle.promise.catch((err) => ...)` then can't
 * classify `err` by shape anymore, so a naive `if (isVersionConflict(err))
 * return;` guard there can never match, regardless of what happened inside
 * the closure.
 *
 * `updateNode()`'s closure DOES raise its own specific `version-mismatch`
 * notification for a genuine OCC conflict — so the outer guard failing to
 * match meant a SECOND, generic `write-failure` notification piled on top
 * of it every time. Fixed the same way as the analogous bug in
 * `deleteNode()` of the same store: a boolean captured inside the closure, set right where the
 * specific notification is raised, checked by the outer catch instead of
 * re-deriving the classification from the already-stripped `err`.
 *
 * `setNode()`'s closure, investigated as part of this fix, turns out NOT to
 * have the presumed twin bug: its own `catch (dbError)` has no OCC-specific
 * branch at all — no specific notification is ever raised there for an OCC
 * failure to duplicate. The dead `isVersionConflict(err)` check that used to
 * sit in its outer catch was removed as misleading dead code rather than
 * "fixed" with a captured flag, since there was never a first notification
 * to guard against duplicating. The second describe block below locks in
 * that setNode()'s OCC failures correctly surface exactly one (generic)
 * notification — confirmed here rather than just asserted in a comment.
 *
 * `updateTaskNode()`'s closure DOES have the OCC-specific branch (it
 * unconditionally raises its own `version-mismatch` notification whenever
 * `occError` is truthy) — but until fixed, its OUTER catch had no
 * `write-failure` fallback for the non-OCC case AT ALL, unlike the other
 * three methods above. That gap meant a genuine non-OCC failure (network
 * error, daemon offline, validation error) surfaced ZERO user-visible
 * notification — worse than a duplicate, a silent failure. Fixed by adding
 * the same captured-boolean-gated fallback the other three methods already
 * have. The third describe block below covers both: exactly one
 * notification on an OCC conflict (not a duplicate, matching updateNode()'s
 * shape), and exactly one (the fallback) on a non-OCC failure (not zero).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

describe('SharedNodeStore — OCC notification dedup preserves error shape', () => {
  let store: SharedNodeStore;

  const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'viewer-1' };
  const databaseSource: UpdateSource = { type: 'database', reason: 'seed' };
  // updateNode()'s persist mode is 'immediate' for a structural/property/
  // nodeType/type-specific change regardless of source, and the tests below
  // change `content` alongside `properties: {}` to land in that bucket — so
  // viewerSource reaches the actual write here. setNode()'s persistence
  // closure is gated differently: `shouldPersist = source.type !== 'viewer'
  // || isNewNode` (shared-node-store.svelte.ts) — a viewer-sourced setNode
  // call on an already-persisted (not new) node never even attempts to
  // persist, so its OCC tests below need a non-viewer source to reach the
  // UPDATE branch at all.
  const mcpSource: UpdateSource = { type: 'mcp-server', serverId: 'test-mcp' };

  const makeNode = (id: string, content: string, version = 1): Node => ({
    id,
    nodeType: 'text',
    content,
    createdAt: '2024-01-01T00:00:00.000Z',
    modifiedAt: '2024-01-01T00:00:00.000Z',
    version,
    properties: {},
    mentions: []
  });

  const makeTaskNode = (id: string, status: string, version = 1): Node =>
    ({
      id,
      nodeType: 'task',
      content: '- [ ] seed task',
      createdAt: '2024-01-01T00:00:00.000Z',
      modifiedAt: '2024-01-01T00:00:00.000Z',
      version,
      properties: {},
      mentions: [],
      status
    }) as unknown as Node;

  // Real daemon error shape (plain object, NOT instanceof Error) — same
  // convention every other OCC test in this suite uses (e.g.
  // ai-chat-occ-conflict-regression.test.ts's makeVersionConflictError).
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

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  describe('updateNode()', () => {
    it('raises exactly one notification (version-mismatch) on an OCC conflict, not a duplicate write-failure', async () => {
      store.setNode(makeNode('u-occ-1', 'seed', 1), databaseSource);
      store.setNode(makeNode('u-occ-1', 'seed', 1), databaseSource);

      // Fallback path (no current_node embedded) — exercises
      // resyncNodeFromServer, not the direct-hydration branch, but the
      // notification-dedup bug is in the shared outer catch, not either
      // hydration branch.
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(null)
      );
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeNode('u-occ-1', 'server-fresh', 2));

      store.updateNode('u-occ-1', { content: 'user-edit', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 'u-occ-1'
      );
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('version-mismatch');
    });

    it('still raises the generic write-failure notification for a genuine non-OCC failure (regression check)', async () => {
      store.setNode(makeNode('u-occ-2', 'seed', 1), databaseSource);
      store.setNode(makeNode('u-occ-2', 'seed', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        new Error('network error: connection refused')
      );

      store.updateNode('u-occ-2', { content: 'user-edit', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 'u-occ-2'
      );
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('write-failure');
    });
  });

  describe('setNode()', () => {
    it('raises exactly one (generic) notification for an OCC conflict — no specific notification exists here to duplicate', async () => {
      // mcp-server source so the UPDATE branch runs regardless of isNewNode
      // (see the class-level comment on mcpSource above).
      store.setNode(makeNode('s-occ-1', 'seed', 1), databaseSource);
      store.setNode(makeNode('s-occ-1', 'seed', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makeVersionConflictError(null)
      );

      store.setNode(makeNode('s-occ-1', 'external-edit', 1), mcpSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 's-occ-1'
      );
      // setNode()'s persistence closure has no OCC-specific branch, so an
      // OCC failure surfaces through the same generic path any other
      // failure does — exactly one notification, not zero (silently
      // dropped) and not two (duplicated).
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('write-failure');
    });

    it('still raises exactly one notification for a genuine non-OCC failure (regression check)', async () => {
      store.setNode(makeNode('s-occ-2', 'seed', 1), databaseSource);
      store.setNode(makeNode('s-occ-2', 'seed', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        new Error('network error: connection refused')
      );

      store.setNode(makeNode('s-occ-2', 'external-edit', 1), mcpSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 's-occ-2'
      );
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('write-failure');
    });
  });

  describe('updateTaskNode()', () => {
    it('raises exactly one notification (version-mismatch) on an OCC conflict, not a duplicate write-failure', async () => {
      store.setNode(makeTaskNode('t-occ-1', 'open', 1), databaseSource);

      // Fallback path (no current_node embedded) — exercises
      // resyncNodeFromServer, not the direct-hydration branch, but the
      // notification-dedup bug is in the shared outer catch, not either
      // hydration branch.
      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makeVersionConflictError(null)
      );
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(makeTaskNode('t-occ-1', 'open', 2));

      store.updateTaskNode('t-occ-1', { status: 'in_progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 't-occ-1'
      );
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('version-mismatch');
    });

    it('raises the generic write-failure notification for a genuine non-OCC failure (was previously silently swallowed — the bug this fix closes)', async () => {
      store.setNode(makeTaskNode('t-occ-2', 'open', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        new Error('network error: connection refused')
      );

      store.updateTaskNode('t-occ-2', { status: 'in_progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === 't-occ-2'
      );
      // Before the fix: this was 0 — updateTaskNode()'s outer catch had no
      // fallback for a non-OCC failure at all, so a genuine write failure
      // surfaced with zero user-visible signal.
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('write-failure');
    });
  });
});
