/**
 * Regression: an OCC conflict during an active ai-chat turn must leave the
 * store holding the node's real final state, not a half-typed snapshot.
 *
 * The ai-chat viewer derives its typing indicator / Stop button from the
 * node's TOP-LEVEL `turnStatus`, and renders the reply from TOP-LEVEL `messages`
 * (`AiChatNode` is the flat wire shape the daemon's `node_to_typed_value`
 * guarantees). The OCC conflict payload crosses the same sync boundary as a
 * daemon broadcast, so it has to arrive — and be hydrated — in that same flat
 * shape.
 *
 * Before the fix the daemon serialized the raw storage `Node` into
 * `current_node`, leaving `status`/`messages` buried under
 * `properties['ai-chat']`. Hydrating that into the store gave the viewer a
 * node with no top-level `turnStatus` and no `messages`, which is what
 * stranded the UI on "processing" after the turn had already completed and
 * persisted.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import type { Node } from '../../lib/types';
import type { AiChatNode } from '../../lib/types/ai-chat-node';
import { CASCADE_SETTLE_TIMEOUT_MS } from '../utils/test-constants';

const CHAT_ID = 'bf2c1788-76ff-4d79-9c6f-7755d13b0c21';

const dbSource = { type: 'database' as const, reason: 'test-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'ai-chat-viewer' };

/** An ai-chat node in the flat wire shape the daemon actually sends. */
const makeChatNode = (overrides: Partial<AiChatNode> = {}): Node =>
  ({
    id: CHAT_ID,
    nodeType: 'ai-chat',
    content: '',
    createdAt: '2024-01-01T00:00:00.000Z',
    modifiedAt: '2024-01-01T00:00:00.000Z',
    version: 3,
    turnStatus: 'processing',
    sessionStatus: 'active',
    provider: 'native',
    model: 'e4b',
    messages: [{ role: 'user', content: 'What is on my plate today?' }],
    ...overrides
  }) as unknown as Node;

const makeVersionConflictError = (currentNode: Node | null) => ({
  message: `Version conflict on ${CHAT_ID}: expected 3, got 4`,
  code: 'VERSION_CONFLICT',
  details: 'Aborted',
  conflictData: {
    node_id: CHAT_ID,
    expected: 3,
    actual: 4,
    current_node: currentNode
  }
});

/** Read the node back the way the viewer does. */
const readChat = (store: SharedNodeStore): AiChatNode | undefined =>
  store.getNode(CHAT_ID) as unknown as AiChatNode | undefined;

describe('ai-chat OCC conflict during an active turn', () => {
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

  /**
   * The turn-completion state the daemon has already persisted by the time the
   * frontend's racing write is rejected: reply appended, status back to idle.
   */
  const completedTurn = makeChatNode({
    // `idle` is what the daemon actually writes on turn completion
    // (`append_assistant_message` / `write_ai_chat_turn_status`), so the
    // fixture mirrors production rather than a value it never sends.
    version: 4,
    turnStatus: 'idle',
    messages: [
      { role: 'user', content: 'What is on my plate today?' },
      { role: 'assistant', content: 'You have two tasks due today.' }
    ]
  });

  /**
   * Seed the store so `updateNode` takes the UPDATE path. `setNode` only marks
   * a database-sourced node persisted once the store has already seen it, so
   * this has to happen twice — otherwise the write routes to `createNode` and
   * the mocked `updateNode` rejection never fires.
   */
  function seedProcessingNode(): void {
    const initial = makeChatNode();
    store.setNode(initial, dbSource);
    store.setNode(initial, dbSource);
  }

  it('hydrates the completed turn so the UI leaves the processing state', async () => {
    seedProcessingNode();

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
      makeVersionConflictError(completedTurn)
    );

    // The viewer's optimistic write that loses the race with the daemon.
    store.updateNode(
      CHAT_ID,
      { properties: { turnStatus: 'processing', messages: [{ role: 'user', content: 'ping' }] } },
      viewerSource
    );

    // Wait for the conflict payload to be hydrated rather than for a duration
    // assumed to cover debounce + rejection + hydration.
    await vi.waitFor(() => expect(readChat(store)?.turnStatus).toBe('idle'), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    const chat = readChat(store);

    // The exact bug: the viewer's `isProcessing` is
    // `turnStatus === 'processing'`. Hydrating an unflattened payload left
    // this undefined AND dropped the reply, so the indicator hung with
    // nothing to show.
    expect(chat?.turnStatus).toBe('idle');
    expect(chat?.turnStatus).not.toBe('processing');

    // The reply the daemon already persisted must be visible.
    expect(chat?.messages).toHaveLength(2);
    expect(chat?.messages?.[1]).toMatchObject({
      role: 'assistant',
      content: 'You have two tasks due today.'
    });

    // Version must track the winning write so the next send doesn't re-conflict.
    expect(chat?.version).toBe(4);
  }, 5000);

  it('keeps the flat shape when the payload arrives nested under properties', async () => {
    seedProcessingNode();

    // Defense in depth: even if a conflict payload reaches the client in the
    // raw storage shape, hydration must not strand the viewer with an
    // undefined top-level `turnStatus`.
    const nestedPayload = {
      id: CHAT_ID,
      nodeType: 'ai-chat',
      content: '',
      createdAt: '2024-01-01T00:00:00.000Z',
      modifiedAt: '2024-01-01T00:00:00.000Z',
      version: 4,
      properties: {
        'ai-chat': {
          turn_status: 'idle',
          session_status: 'active',
          messages: [
            { role: 'user', content: 'What is on my plate today?' },
            { role: 'assistant', content: 'You have two tasks due today.' }
          ]
        }
      }
    } as unknown as Node;

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
      makeVersionConflictError(nestedPayload)
    );

    store.updateNode(
      CHAT_ID,
      { properties: { turnStatus: 'processing' } },
      viewerSource
    );

    // Wait for hydration to REPLACE the optimistic 'processing' value. Waiting
    // merely for turnStatus to be defined would return immediately — the
    // optimistic write already set it — and assert nothing.
    await vi.waitFor(() => expect(readChat(store)?.turnStatus).not.toBe('processing'), {
      timeout: CASCADE_SETTLE_TIMEOUT_MS
    });

    const chat = readChat(store);

    // `turnStatus` must be a real top-level value the viewer can read, never
    // undefined — an undefined turnStatus is what left the Stop button wedged.
    expect(chat?.turnStatus).toBeDefined();
    expect(chat?.turnStatus).not.toBe('processing');
    expect(Array.isArray(chat?.messages)).toBe(true);
  }, 5000);

  it('does not install a conflict payload older than what the store already has', async () => {
    // Both writers into this store — conflict hydration and daemon broadcast —
    // must agree on which snapshot wins. A broadcast that already delivered a
    // newer turn must not be undone by an out-of-order conflict payload.
    seedProcessingNode();
    store.setNode(completedTurn, dbSource);

    const stalePayload = makeChatNode({
      version: 2,
      turnStatus: 'processing',
      messages: [{ role: 'user', content: 'What is on my plate today?' }]
    });

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
      makeVersionConflictError(stalePayload)
    );
    // The stale branch falls through to a server resync; keep that from
    // re-introducing state and masking what this test is asserting.
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(completedTurn);

    const rollbacksBefore = store.getMetrics().rollbackCount;

    store.updateNode(CHAT_ID, { properties: { turnStatus: 'processing' } }, viewerSource);

    // This test asserts a NEGATIVE (the stale payload must not be installed),
    // so it cannot wait on the end state — that state is already true before
    // the conflict is handled, and the wait would return immediately without
    // exercising anything. Wait instead on the conflict having been processed;
    // the rollback counter is the observable marker of that.
    await vi.waitFor(
      () => expect(store.getMetrics().rollbackCount).toBeGreaterThan(rollbacksBefore),
      { timeout: CASCADE_SETTLE_TIMEOUT_MS }
    );

    const chat = readChat(store);
    expect(chat?.version).toBe(4);
    expect(chat?.turnStatus).not.toBe('processing');
    expect(chat?.messages).toHaveLength(2);
  }, 5000);

  it('surfaces the conflict to the user and records the rollback', async () => {
    seedProcessingNode();

    const before = store.getMetrics().rollbackCount;

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValueOnce(
      makeVersionConflictError(completedTurn)
    );

    store.updateNode(
      CHAT_ID,
      { properties: { turnStatus: 'processing' } },
      viewerSource
    );

    // Wait on the notification being surfaced — the behaviour under test.
    await vi.waitFor(
      () =>
        expect(
          conflictNotifications.notifications.filter(
            (n) => n.nodeId === CHAT_ID && n.conflictType === 'version-mismatch'
          ).length
        ).toBeGreaterThanOrEqual(1),
      { timeout: CASCADE_SETTLE_TIMEOUT_MS }
    );

    expect(store.getMetrics().rollbackCount).toBeGreaterThan(before);
  }, 5000);
});
