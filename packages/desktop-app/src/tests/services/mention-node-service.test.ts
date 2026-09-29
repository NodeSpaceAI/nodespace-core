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

  it('still resolves with the id when the follow-up store load fails', async () => {
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue({ id: 'x', placement: null });
    vi.spyOn(sharedNodeStore, 'ensureNode').mockRejectedValue(new Error('grpc down'));

    await expect(createMentionTargetNode('Fresh')).resolves.toEqual(expect.any(String));
  });

  it('populates the real store from the backend read', async () => {
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue({ id: 'x', placement: null });
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(async (id: string) => ({
      id,
      nodeType: 'text',
      content: 'Fresh',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {}
    }));

    const id = await createMentionTargetNode('Fresh');
    expect(sharedNodeStore.getNode(id)?.content).toBe('Fresh');
  });
});
