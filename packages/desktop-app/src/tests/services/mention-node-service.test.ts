/**
 * createMentionTargetNode: the @mention "create new" path writes through
 * `create_node`, whose echo the daemon suppresses for this window, so the
 * service must load the node into `sharedNodeStore` itself.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { createMentionTargetNode } from '$lib/services/mention-node-service';

afterEach(() => vi.restoreAllMocks());

describe('createMentionTargetNode', () => {
  it('creates a text node with the title and loads it into the shared store', async () => {
    const createSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockResolvedValue({ id: 'ignored', placement: null });
    const ensureSpy = vi.spyOn(sharedNodeStore, 'ensureNode').mockResolvedValue(undefined);

    const id = await createMentionTargetNode('Fresh idea');

    expect(createSpy).toHaveBeenCalledWith(
      expect.objectContaining({ id, content: 'Fresh idea', nodeType: 'text' })
    );
    expect(ensureSpy).toHaveBeenCalledWith(id);
    expect(createSpy.mock.invocationCallOrder[0]).toBeLessThan(ensureSpy.mock.invocationCallOrder[0]);
  });

  it('does not touch the store when the backend create fails', async () => {
    vi.spyOn(backendAdapter, 'createNode').mockRejectedValue(new Error('boom'));
    const ensureSpy = vi.spyOn(sharedNodeStore, 'ensureNode').mockResolvedValue(undefined);

    await expect(createMentionTargetNode('x')).rejects.toThrow('boom');
    expect(ensureSpy).not.toHaveBeenCalled();
  });
});
