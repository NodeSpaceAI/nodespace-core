/**
 * Guards the test-side mirrors of SharedNodeStore's private timing constants.
 *
 * `PERSISTENCE_DEBOUNCE_MS` and `FLUSH_PENDING_TIMEOUT_MS` duplicate values that
 * live as `private readonly` fields in `shared-node-store.svelte.ts` and so
 * cannot be imported. Duplication that nothing checks is duplication that drifts:
 * if the production debounce were raised, every test waiting
 * `PERSISTENCE_DEBOUNCE_MS + DEBOUNCE_SETTLE_MS` would start failing, or worse,
 * flaking — with nothing pointing at the mirror as the cause.
 *
 * These tests fail loudly and name the file to edit instead.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';
import {
  PERSISTENCE_DEBOUNCE_MS,
  DEBOUNCE_SETTLE_MS,
  FLUSH_PENDING_TIMEOUT_MS
} from '../utils/test-constants';

describe('persistence timing constants stay in sync with SharedNodeStore', () => {
  let store: SharedNodeStore;

  const mockNode: Node = {
    id: 'timing-guard-node',
    nodeType: 'text',
    content: 'Test content',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  };

  const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'viewer-1' };
  const databaseSource: UpdateSource = { type: 'database', reason: 'test' };

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    store.clearTestErrors();
  });

  afterEach(() => {
    vi.useRealTimers();
    store.clearAll();
    SharedNodeStore.resetInstance();
    vi.clearAllMocks();
  });

  it('does not persist before PERSISTENCE_DEBOUNCE_MS has elapsed', async () => {
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockResolvedValue({ ...mockNode, version: 2 });

    vi.useFakeTimers();
    store.setNode(mockNode, databaseSource);
    store.updateNode(mockNode.id, { content: 'debounced' }, viewerSource);

    // Just short of the debounce: the write must still be pending. If the
    // production DEBOUNCE_MS were LOWERED, this fires early and fails here.
    await vi.advanceTimersByTimeAsync(PERSISTENCE_DEBOUNCE_MS - 1);
    expect(updateSpy).not.toHaveBeenCalled();
  });

  it('has persisted once PERSISTENCE_DEBOUNCE_MS + DEBOUNCE_SETTLE_MS has elapsed', async () => {
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockResolvedValue({ ...mockNode, version: 2 });

    vi.useFakeTimers();
    store.setNode(mockNode, databaseSource);
    store.updateNode(mockNode.id, { content: 'debounced' }, viewerSource);

    // The window every converted test relies on. If the production DEBOUNCE_MS
    // were RAISED, this fails here rather than as a flake somewhere else.
    await vi.advanceTimersByTimeAsync(PERSISTENCE_DEBOUNCE_MS + DEBOUNCE_SETTLE_MS);
    expect(updateSpy).toHaveBeenCalledTimes(1);
  });

  it('flushAllPending gives up after FLUSH_PENDING_TIMEOUT_MS when an operation never settles', async () => {
    // Never settles: only the internal timeout can resolve the flush.
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
      () => new Promise<Node>(() => {})
    );

    vi.useFakeTimers();
    store.setNode(mockNode, databaseSource);
    store.updateNode(mockNode.id, { content: 'hangs' }, viewerSource);

    const flushPromise = store.flushAllPending();
    let settled = false;
    void flushPromise.then(() => {
      settled = true;
    });

    // One tick short of the timeout, the flush must still be waiting.
    await vi.advanceTimersByTimeAsync(FLUSH_PENDING_TIMEOUT_MS - 1);
    expect(settled).toBe(false);

    // Crossing the timeout resolves it.
    await vi.advanceTimersByTimeAsync(1);
    await expect(flushPromise).resolves.not.toThrow();
    expect(settled).toBe(true);
  });
});
