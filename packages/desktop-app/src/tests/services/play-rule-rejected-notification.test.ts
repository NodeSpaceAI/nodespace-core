/**
 * SharedNodeStore PLAY_RULE_REJECTED handling → conflictNotifications store.
 *
 * Verifies both write paths that can receive a `PlayRuleRejected` `CommandError`
 * from `backendAdapter` surface the rejecting rule's own author-supplied
 * `message` (not a generic string), tagged with `conflictType:
 * 'play-rule-rejected'`, and exactly once (not doubled with a second, generic
 * `write-failure` notification via the outer `handle.promise.catch()` —
 * mirrors the dedup fix `occ-notification-dedup.test.ts` covers for OCC/
 * subtree-access-denied errors, applied here to the newer PlayRuleRejected
 * branch).
 *
 * `updateNode()` and `updateTaskNode()` are the two call sites the daemon's
 * synchronous invariant dispatch (`dispatch_invariant_rules_in_tx` /
 * `dispatch_invariant_rules_for_update_in_tx`) can reach — see each
 * `describe` block below for how each method's own recovery pattern differs.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

describe('SharedNodeStore → conflictNotifications (PLAY_RULE_REJECTED)', () => {
  let store: SharedNodeStore;

  const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'viewer-1' };
  const databaseSource: UpdateSource = { type: 'database', reason: 'seed' };

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
  // convention every other coded-error test in this suite uses (see
  // occ-notification-dedup.test.ts's makeVersionConflictError).
  const makePlayRuleRejectedError = (nodeId: string, ruleMessage: string) => ({
    message: `Play rule 'No closing with open sub-issues' (play play-1) rejected the write to node ${nodeId}: ${ruleMessage}`,
    code: 'PLAY_RULE_REJECTED' as const,
    details: 'FailedPrecondition',
    conflictData: {
      node_id: nodeId,
      play_id: 'play-1',
      rule_name: 'No closing with open sub-issues',
      message: ruleMessage
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
    it('surfaces exactly one play-rule-rejected notification carrying the rule\'s own message, and rolls back via rollbackUpdate()', async () => {
      const nodeId = 'u-rej-1';
      store.setNode(makeNode(nodeId, 'seed', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makePlayRuleRejectedError(nodeId, "Can't close a task with open sub-issues")
      );

      const metricsBefore = store.getMetrics();

      store.updateNode(nodeId, { content: 'user-edit', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === nodeId
      );
      // Exactly one — not doubled with the outer catch's generic
      // write-failure fallback (occConflictAlreadyNotified must be set).
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('play-rule-rejected');
      expect(notifications[0].message).toBe("Can't close a task with open sub-issues");

      // rollbackUpdate() was called (bookkeeping: pendingUpdates entry
      // removed, version rolled back, rollbackCount metric incremented) —
      // same call this method's OCC branch already makes.
      const metricsAfter = store.getMetrics();
      expect(metricsAfter.rollbackCount).toBeGreaterThan(metricsBefore.rollbackCount);
    });

    it('does not surface a second, generic write-failure notification alongside the play-rule-rejected one', async () => {
      const nodeId = 'u-rej-2';
      store.setNode(makeNode(nodeId, 'seed', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
        makePlayRuleRejectedError(nodeId, 'Rejected: field is locked')
      );

      store.updateNode(nodeId, { content: 'user-edit', properties: {} }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === nodeId
      );
      expect(notifications).toHaveLength(1);
      expect(notifications.some((n) => n.conflictType === 'write-failure')).toBe(false);
    });
  });

  describe('updateTaskNode()', () => {
    it("surfaces exactly one play-rule-rejected notification carrying the rule's own message", async () => {
      const nodeId = 't-rej-1';
      store.setNode(makeTaskNode(nodeId, 'open', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makePlayRuleRejectedError(nodeId, "Can't move to done with open sub-issues")
      );

      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === nodeId
      );
      expect(notifications).toHaveLength(1);
      expect(notifications[0].conflictType).toBe('play-rule-rejected');
      expect(notifications[0].message).toBe("Can't move to done with open sub-issues");
    });

    it('does not surface a second, generic write-failure notification alongside the play-rule-rejected one', async () => {
      const nodeId = 't-rej-2';
      store.setNode(makeTaskNode(nodeId, 'open', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makePlayRuleRejectedError(nodeId, 'Rejected: status is locked')
      );

      store.updateTaskNode(nodeId, { status: 'in_progress' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      const notifications = conflictNotifications.notifications.filter(
        (n) => n.nodeId === nodeId
      );
      expect(notifications).toHaveLength(1);
      expect(notifications.some((n) => n.conflictType === 'write-failure')).toBe(false);
    });

    it('reconciles the store to its current state (no crash, no stuck pending state) after the rejection, mirroring this method\'s non-rollbackUpdate() recovery pattern', async () => {
      const nodeId = 't-rej-3';
      store.setNode(makeTaskNode(nodeId, 'open', 1), databaseSource);

      vi.spyOn(backendAdapter, 'updateTaskNode').mockRejectedValueOnce(
        makePlayRuleRejectedError(nodeId, 'Rejected')
      );

      store.updateTaskNode(nodeId, { status: 'done' }, viewerSource);
      await new Promise((resolve) => setTimeout(resolve, 100));

      // The store still holds a valid node for this id afterward — the
      // rejection path notified subscribers with whatever the store
      // currently holds rather than leaving it in an inconsistent state.
      const stored = store.getNode(nodeId);
      expect(stored).toBeDefined();
      expect(stored?.id).toBe(nodeId);
    });
  });
});
