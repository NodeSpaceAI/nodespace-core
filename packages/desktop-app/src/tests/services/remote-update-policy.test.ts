/**
 * Unit tests for the extracted remote-update policy (issue: extract explicit
 * remote-update policy from the skip-while-editing logic previously inlined
 * in SharedNodeStore.setNode / batchSetNodes).
 *
 * End-to-end behavior through the store is covered by
 * `shared-node-store-skip-while-editing.test.ts`; these tests pin the pure
 * decision function's contract in isolation.
 */

import { describe, it, expect } from 'vitest';
import { decideRemoteUpdate, shouldSkipStaleAiChatUpdate } from '$lib/services/remote-update-policy';
import type { Node } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';

function makeNode(overrides: Partial<Node> = {}): Node {
  return {
    id: 'n1',
    nodeType: 'text',
    content: 'content',
    createdAt: '2024-01-01T00:00:00Z',
    modifiedAt: '2024-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: [],
    ...overrides
  } as Node;
}

const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'v1' };
const databaseSource: UpdateSource = { type: 'database', reason: 'domain-event' };
const notEditing = { isFocused: false, hasPending: false };

describe('decideRemoteUpdate', () => {
  it('applies when source is not database', () => {
    const decision = decideRemoteUpdate(makeNode(), makeNode(), viewerSource, {
      isFocused: true,
      hasPending: false
    });
    expect(decision.apply).toBe(true);
  });

  it('applies when there is no existing node (first sighting)', () => {
    const decision = decideRemoteUpdate(makeNode(), undefined, databaseSource, {
      isFocused: true,
      hasPending: false
    });
    expect(decision.apply).toBe(true);
  });

  it('applies when the node is not actively edited (not focused, no pending write)', () => {
    const decision = decideRemoteUpdate(makeNode(), makeNode(), databaseSource, notEditing);
    expect(decision.apply).toBe(true);
  });

  it('skips (does not apply) a database update to a focused node', () => {
    const decision = decideRemoteUpdate(
      makeNode({ content: 'incoming' }),
      makeNode({ content: 'local' }),
      databaseSource,
      { isFocused: true, hasPending: false }
    );
    expect(decision.apply).toBe(false);
  });

  it('skips a database update to a node with a pending write even if unfocused', () => {
    const decision = decideRemoteUpdate(
      makeNode({ content: 'incoming' }),
      makeNode({ content: 'local' }),
      databaseSource,
      { isFocused: false, hasPending: true }
    );
    expect(decision.apply).toBe(false);
  });

  // ADR-026 C5 extension: the daemon suppresses a connection's own write
  // echoes before they ever reach WatchNodes, so a database-sourced event
  // from the SAME connection can no longer reach this policy — but a
  // genuinely newer foreign write (a different window, or a sync-service
  // pull) still must always notify.
  it('notifies a genuinely newer foreign write to an actively-edited node', () => {
    const decision = decideRemoteUpdate(
      makeNode({ content: 'bob wrote this', version: 9 }),
      makeNode({ content: 'alice typed this', version: 3 }),
      databaseSource,
      { isFocused: true, hasPending: false }
    );
    expect(decision.apply).toBe(false);
    if (decision.apply) throw new Error('unreachable');
    expect(decision.notifyConflict).toBe(true);
  });

  it('does not notify for a stale broadcast whose version is not ahead of the local version', () => {
    // The daemon-side echo suppression covers same-connection writes, but not
    // a stale replay from a writer running inside the daemon (it writes through
    // its own NodeService::with_client(...), a separate path — see this
    // module's doc comment). A broadcast whose version is not strictly ahead
    // of the local optimistic version can still arrive from that path and
    // must be dropped silently instead of raising a phantom conflict
    // notification.
    const decision = decideRemoteUpdate(
      makeNode({ content: 'hell', version: 4 }),
      makeNode({ content: 'hello world', version: 5 }),
      databaseSource,
      { isFocused: true, hasPending: false }
    );
    expect(decision.apply).toBe(false);
    if (decision.apply) throw new Error('unreachable');
    expect(decision.notifyConflict).toBe(false);
  });

  it('treats an incoming node with no numeric version as conservatively notifying', () => {
    const decision = decideRemoteUpdate(
      makeNode({ content: 'hello world', version: undefined as unknown as number }),
      makeNode({ content: 'hello world', version: 3 }),
      databaseSource,
      { isFocused: true, hasPending: false }
    );
    expect(decision.apply).toBe(false);
    if (decision.apply) throw new Error('unreachable');
    expect(decision.notifyConflict).toBe(true);
  });
});

describe('shouldSkipStaleAiChatUpdate', () => {
  const chat = (version?: number): Node =>
    ({ ...makeNode({ nodeType: 'ai-chat-native' }), version }) as Node;

  it('returns false for non-ai-chat nodes', () => {
    expect(
      shouldSkipStaleAiChatUpdate(
        makeNode({ nodeType: 'text', version: 1 }),
        makeNode({ nodeType: 'text', version: 5 }),
        databaseSource
      )
    ).toBe(false);
  });

  it('returns false for viewer-sourced updates', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(1), chat(5), viewerSource)).toBe(false);
  });

  it('returns false when there is no existing node', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(1), undefined, databaseSource)).toBe(false);
  });

  it('skips a snapshot whose version is older', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(4), chat(5), databaseSource)).toBe(true);
  });

  it('applies a strictly newer snapshot', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(6), chat(5), databaseSource)).toBe(false);
  });

  it('applies an equal-version snapshot when nothing is pending', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(3), chat(3), databaseSource)).toBe(false);
    expect(shouldSkipStaleAiChatUpdate(chat(3), chat(3), databaseSource, false)).toBe(false);
  });

  it('applies when a version is missing', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(9), chat(undefined), databaseSource)).toBe(false);
  });

  // Regression coverage for the model-selection revert bug: a property-only
  // optimistic write (e.g. model selection) does not bump the local node's
  // `.version` — only the write's own response does — so while it is still
  // in flight, an unrelated echo (e.g. the node's own creation broadcast)
  // can race in and report the SAME version the local node is still sitting
  // at, while actually being the pre-write snapshot.
  it('skips an equal-version snapshot when a local write is pending', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(1), chat(1), databaseSource, true)).toBe(true);
  });

  it('a strictly newer incoming version still applies even while pending', () => {
    expect(shouldSkipStaleAiChatUpdate(chat(2), chat(1), databaseSource, true)).toBe(false);
  });
});
