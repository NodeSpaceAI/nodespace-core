/**
 * Regression tests: node content not persisting to database.
 *
 * Covers the write → flush → read-back cycle so that edits made in the Tauri
 * desktop app reliably reach the backend.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node } from '../../lib/types';

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

const dbSource = { type: 'database' as const, reason: 'initial-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'pane-1' };

describe('Node content persistence regression', () => {
  let store: SharedNodeStore;

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

  it('write → flush → read-back: content written via updateNode reaches the backend', async () => {
    const nodeId = 'persist-regression-1';
    const initialNode = makeNode(nodeId, 'Original content', 1);
    const updatedContent = 'Edited content that must persist';

    // Simulate the backend storing the update and returning an incremented version
    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValueOnce({
      ...initialNode,
      content: updatedContent,
      version: 2
    });

    // Load the node as if the daemon pushed it
    store.setNode(initialNode, dbSource);

    // User edits the node in the viewer
    store.updateNode(nodeId, { content: updatedContent }, viewerSource);

    // Wait for the debounced persistence to flush
    await store.flushAllPendingSaves(3000);

    // The backend must have received the correct content
    expect(updateSpy).toHaveBeenCalledOnce();
    const [calledNodeId, , calledNode] = updateSpy.mock.calls[0];
    expect(calledNodeId).toBe(nodeId);
    expect(calledNode.content).toBe(updatedContent);

    // The in-memory store must reflect the backend-confirmed version
    const stored = store.getNode(nodeId);
    expect(stored?.content).toBe(updatedContent);
    expect(stored?.version).toBe(2);
  }, 5000);

  it('write → flush → read-back: content persists across multiple sequential edits', async () => {
    const nodeId = 'persist-regression-2';
    const initialNode = makeNode(nodeId, 'Start', 1);

    let callCount = 0;
    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, _v, node) => {
      callCount++;
      return {
        id: nodeId,
        nodeType: node.nodeType ?? 'text',
        content: node.content ?? '',
        createdAt: '2024-01-01T00:00:00.000Z',
        modifiedAt: new Date().toISOString(),
        version: 1 + callCount,
        properties: node.properties ?? {}
      };
    });

    store.setNode(initialNode, dbSource);

    store.updateNode(nodeId, { content: 'Edit 1' }, viewerSource);
    store.updateNode(nodeId, { content: 'Edit 2' }, viewerSource);
    store.updateNode(nodeId, { content: 'Edit 3 — final' }, viewerSource);

    await store.flushAllPendingSaves(3000);

    // PersistenceCoordinator debounces and coalesces rapid edits into a single
    // backend call. The exact call count is non-deterministic (could be 1 or 3
    // depending on timer resolution), so we assert only that it was called at
    // all and that the final write carried the last content.
    expect(updateSpy).toHaveBeenCalled();
    const lastCall = updateSpy.mock.calls[updateSpy.mock.calls.length - 1];
    expect(lastCall[2].content).toBe('Edit 3 — final');
  }, 5000);

  it('write → flush: a new node created via setNode is persisted via createNode', async () => {
    const nodeId = 'persist-regression-3';
    const newNode = makeNode(nodeId, 'Brand new node content', 1);

    // createNode returns the node ID and its placement, not a Node object
    const createSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockResolvedValueOnce({ id: nodeId, placement: null });

    // New node arrives from viewer (not yet in DB)
    store.setNode(newNode, viewerSource);

    await store.flushAllPendingSaves(3000);

    expect(createSpy).toHaveBeenCalledOnce();
    const [createdInput] = createSpy.mock.calls[0];
    expect((createdInput as { id?: string }).id).toBe(nodeId);
    expect((createdInput as { content?: string }).content).toBe('Brand new node content');
  }, 5000);

  it('write failure surfaces a write-failure conflict notification', async () => {
    const nodeId = 'persist-regression-4';
    const initialNode = makeNode(nodeId, 'Content', 1);

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
      new Error('Network error: daemon unreachable')
    );

    store.setNode(initialNode, dbSource);
    store.updateNode(nodeId, { content: 'Edit that will fail' }, viewerSource);

    await store.flushAllPendingSaves(3000);

    const notifications = conflictNotifications.notifications;
    expect(notifications.length).toBeGreaterThanOrEqual(1);
    const writeFailure = notifications.find((n) => n.conflictType === 'write-failure');
    expect(writeFailure).toBeDefined();
    expect(writeFailure?.nodeId).toBe(nodeId);
  }, 5000);

  it('rapid content edits with nodeType redundantly bundled (real updateNodeContent shape) debounce into one write, no spurious conflict toast', async () => {
    // Regression for a "conflicted with a remote change" toast firing on
    // effectively every keystroke. Root cause: reactive-node-service.svelte's
    // updateNodeContent() always bundles the CURRENT (unchanged) nodeType
    // alongside content on every keystroke, to keep a slash-command type
    // conversion from racing a content update. But
    // isNodeTypeChange used to be `'nodeType' in changes` — mere presence,
    // not a value comparison — so that redundant nodeType forced
    // mode: 'immediate' on every keystroke instead of 'debounce'. Immediate
    // mode fires an RPC per keystroke, and the broadcast echo for an early
    // keystroke can land after later keystrokes have moved the local content
    // on, misclassifying the echo as a foreign write and firing a conflict
    // notification (and, under fast enough typing, corrupting content).
    //
    // This test exercises the EXACT payload shape the real call site sends
    // (content + unchanged nodeType together) — the other tests in this file
    // send `{ content }` alone, which never exercised this path.
    const nodeId = 'persist-regression-nodetype-bundle';
    const initialNode = makeNode(nodeId, '', 1);

    let updateCallCount = 0;
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (_id, _v, node) => {
      updateCallCount++;
      return {
        id: nodeId,
        nodeType: node.nodeType ?? 'text',
        content: node.content ?? '',
        createdAt: '2024-01-01T00:00:00.000Z',
        modifiedAt: new Date().toISOString(),
        version: 1 + updateCallCount,
        properties: node.properties ?? {}
      };
    });

    store.setNode(initialNode, dbSource);

    // Simulate keystroke-by-keystroke typing, each call bundling the
    // unchanged nodeType exactly as updateNodeContent() does in production.
    // flushAllPendingSaves force-fires debounce timers early, so it can't
    // distinguish immediate vs. debounced mode — the distinguishing signal is
    // synchronous: `mode: 'immediate'` calls executeOperation() (and thus the
    // mocked backendAdapter.updateNode) SYNCHRONOUSLY within persist(), before
    // any await; `mode: 'debounce'` schedules a setTimeout and calls nothing
    // until it fires. Check the call count BEFORE any flush/await.
    const keystrokes = ['a', 'ab', 'abc', 'abcd', 'abcde'];
    for (const partial of keystrokes) {
      store.updateNode(nodeId, { content: partial, nodeType: 'text' }, viewerSource);
    }

    // Immediate mode would have fired at least the first keystroke's RPC
    // synchronously by now; debounce mode has fired none yet.
    expect(updateCallCount).toBe(0);

    await store.flushAllPendingSaves(3000);

    // No spurious version-mismatch conflict notification from the redundant
    // nodeType forcing immediate mode.
    const spuriousConflicts = conflictNotifications.notifications.filter(
      (n) => n.nodeId === nodeId && n.conflictType === 'version-mismatch'
    );
    expect(spuriousConflicts).toHaveLength(0);

    expect(store.getNode(nodeId)?.content).toBe('abcde');
  }, 5000);

  it('database broadcast does not clobber an actively-edited node', async () => {
    const nodeId = 'persist-regression-5';
    const initialNode = makeNode(nodeId, 'Server state', 1);
    const userTyped = 'User is typing this right now';

    vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
      ...initialNode,
      version: 2
    });

    // Load initial node
    store.setNode(initialNode, dbSource);

    // User starts editing (makes the node "focused" for OCC purposes via pending op)
    store.updateNode(nodeId, { content: userTyped }, viewerSource);

    // Daemon echoes back an older server snapshot while the user is still typing
    const olderServerSnapshot = makeNode(nodeId, 'Older server snapshot', 1);
    store.setNode(olderServerSnapshot, dbSource);

    // The in-memory store must still hold the user's latest content
    expect(store.getNode(nodeId)?.content).toBe(userTyped);

    await store.flushAllPendingSaves(3000);
  }, 5000);

  it("a properties-only update syncs the backend's flattened top-level fields back into the local node", async () => {
    // Regression for the ai-chat viewer getting stuck on "Choose a model to
    // get started" after selecting a model. Root cause: this store's
    // updateNode() only copied `.version` from the backend's response back
    // into the local node. The optimistic write sends the un-flattened
    // `{ properties: { provider, model } }` shape (matching storage), but
    // `node_to_typed_value` on the backend promotes type-specific fields like
    // ai-chat's provider/model to genuinely top-level fields on its response.
    // Viewers read those top-level fields directly (e.g. `node?.provider`),
    // so without syncing the full response back, they stayed undefined
    // forever even though the write succeeded server-side.
    const nodeId = 'persist-regression-properties-flatten';
    const initialNode = makeNode(nodeId, '', 1);
    // Mark the node persisted so updateNode() takes the UPDATE path (not
    // CREATE) — setNode() only marks a node persisted for a database-sourced
    // load when the store has already seen it once (see setNode's
    // existingNode guard), so this simulates the node having been loaded on
    // a prior sync rather than freshly created in this test.
    store.setNode(initialNode, dbSource);
    store.setNode(initialNode, dbSource);

    vi.spyOn(backendAdapter, 'updateNode').mockResolvedValueOnce({
      ...initialNode,
      version: 2,
      properties: { provider: 'native', model: 'gemma-4-e4b-q4km' },
      // Simulates node_to_typed_value's flattening: promoted to top-level,
      // as the backend does for ai-chat's provider/model, task's
      // status/priority, etc.
      provider: 'native',
      model: 'gemma-4-e4b-q4km'
    } as unknown as Node);

    store.updateNode(
      nodeId,
      { properties: { provider: 'native', model: 'gemma-4-e4b-q4km' } },
      viewerSource
    );

    await store.flushAllPendingSaves(3000);

    const stored = store.getNode(nodeId) as Node & { provider?: string; model?: string };
    expect(stored?.version).toBe(2);
    expect(stored?.provider).toBe('native');
    expect(stored?.model).toBe('gemma-4-e4b-q4km');
  }, 5000);
});
