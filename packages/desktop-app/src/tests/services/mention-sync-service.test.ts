import { describe, it, expect, beforeEach, vi } from 'vitest';
import { MentionSyncService } from '$lib/services/mention-sync-service';
import type { BackendAdapter } from '$lib/services/backend-adapter';

describe('MentionSyncService', () => {
  let service: MentionSyncService;
  let mockAdapter: BackendAdapter;

  beforeEach(() => {
    // Create a mock adapter with spy functions
    mockAdapter = {
      createNodeMention: vi.fn().mockResolvedValue(undefined),
      deleteNodeMention: vi.fn().mockResolvedValue(undefined)
    } as unknown as BackendAdapter;

    service = new MentionSyncService(mockAdapter);
  });

  describe('extractMentions', () => {
    it('should extract mentions from markdown format', () => {
      const content =
        'Check [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000) for details';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe('123e4567-e89b-12d3-a456-426614174000');
    });

    it('should extract multiple mentions from markdown', () => {
      const content = `
        See [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000) and
        also [@Node B](nodespace://987e6543-e21c-34d5-b678-526614174000)
      `;
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(2);
      expect(mentions).toContain('123e4567-e89b-12d3-a456-426614174000');
      expect(mentions).toContain('987e6543-e21c-34d5-b678-526614174000');
    });

    it('should extract mentions with node/ prefix', () => {
      const content = 'Check [@Node](nodespace://node/123e4567-e89b-12d3-a456-426614174000)';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe('123e4567-e89b-12d3-a456-426614174000');
    });

    it('should extract mentions with query parameters', () => {
      const content = '[@Node](nodespace://123e4567-e89b-12d3-a456-426614174000?hierarchy=true)';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe('123e4567-e89b-12d3-a456-426614174000');
    });

    it('should extract plain format nodespace URIs', () => {
      const content = 'Check nodespace://987e6543-e21c-34d5-b678-526614174000 for more info';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe('987e6543-e21c-34d5-b678-526614174000');
    });

    it('should handle mixed markdown and plain formats', () => {
      const content = `
        Markdown: [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000)
        Plain: nodespace://987e6543-e21c-34d5-b678-526614174000
      `;
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(2);
      expect(mentions).toContain('123e4567-e89b-12d3-a456-426614174000');
      expect(mentions).toContain('987e6543-e21c-34d5-b678-526614174000');
    });

    it('should deduplicate mentions', () => {
      const dedupeTestId = '123e4567-e89b-12d3-a456-426614174111';
      const content = `
        [@Node A](nodespace://${dedupeTestId})
        [@Node A Again](nodespace://${dedupeTestId})
      `;
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe(dedupeTestId);
    });

    it('should handle dedupe across markdown and plain formats', () => {
      const content = `
        [@Node](nodespace://123e4567-e89b-12d3-a456-426614174000)
        nodespace://123e4567-e89b-12d3-a456-426614174000
      `;
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
    });

    it('should ignore invalid UUIDs', () => {
      const content =
        '[@Bad](nodespace://not-a-uuid) and valid [@Good](nodespace://123e4567-e89b-12d3-a456-426614174000)';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(1);
      expect(mentions[0]).toBe('123e4567-e89b-12d3-a456-426614174000');
    });

    it('should return empty array for content without mentions', () => {
      const content = 'This is just plain text with no links';
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(0);
    });

    it('should return empty array for empty content', () => {
      const mentions = service.extractMentions('');

      expect(mentions).toHaveLength(0);
    });

    it('should extract mentions with various markdown link text', () => {
      const content = `
        [@Simple](nodespace://123e4567-e89b-12d3-a456-426614174000)
        [@With spaces](nodespace://234e5678-e89b-12d3-a456-426614174000)
        [@With-dashes](nodespace://345e6789-e89b-12d3-a456-426614174000)
        [@With_underscores](nodespace://456e7890-e89b-12d3-a456-426614174000)
      `;
      const mentions = service.extractMentions(content);

      expect(mentions).toHaveLength(4);
    });
  });

  describe('syncMentions', () => {
    it('should add new mentions when content changes', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);

      const oldContent = 'No mentions here';
      const newContent = 'Check [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000)';

      await service.syncMentions('source-node', oldContent, newContent);

      expect(createSpy).toHaveBeenCalledWith('source-node', '123e4567-e89b-12d3-a456-426614174000');
    });

    it('should remove deleted mentions', async () => {
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = 'Check [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000)';
      const newContent = 'No mentions anymore';

      await service.syncMentions('source-node', oldContent, newContent);

      expect(deleteSpy).toHaveBeenCalledWith('source-node', '123e4567-e89b-12d3-a456-426614174000');
    });

    it('should handle both additions and removals', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = 'Check [@Old](nodespace://aaaaaaaa-e89b-12d3-a456-426614174000)';
      const newContent = 'Check [@New](nodespace://bbbbbbbb-e89b-12d3-a456-426614174000)';

      await service.syncMentions('source-node', oldContent, newContent);

      expect(createSpy).toHaveBeenCalledWith('source-node', 'bbbbbbbb-e89b-12d3-a456-426614174000');
      expect(deleteSpy).toHaveBeenCalledWith('source-node', 'aaaaaaaa-e89b-12d3-a456-426614174000');
    });

    it('should not modify mentions that remain unchanged', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const content = 'Check [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000)';

      await service.syncMentions('source-node', content, content);

      expect(createSpy).not.toHaveBeenCalled();
      expect(deleteSpy).not.toHaveBeenCalled();
    });

    it('should not create self-references', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);

      const content = 'Check [@Self](nodespace://my-own-id-234567-e89b-12d3-a456-426614174000)';

      await service.syncMentions(
        'my-own-id-234567-e89b-12d3-a456-426614174000',
        undefined,
        content
      );

      expect(createSpy).not.toHaveBeenCalled();
    });

    it('should handle empty old content (new node)', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);

      const newContent = 'Check [@Node A](nodespace://123e4567-e89b-12d3-a456-426614174000)';

      await service.syncMentions('new-node', undefined, newContent);

      expect(createSpy).toHaveBeenCalledWith('new-node', '123e4567-e89b-12d3-a456-426614174000');
    });

    it('should handle content that becomes empty (delete all mentions)', async () => {
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = `
        [@A](nodespace://123e4567-e89b-12d3-a456-426614174000)
        [@B](nodespace://234e5678-e89b-12d3-a456-426614174000)
      `;

      await service.syncMentions('node-123', oldContent, '');

      expect(deleteSpy).toHaveBeenCalledTimes(2);
    });

    it('should continue even if create fails', async () => {
      vi.spyOn(mockAdapter, 'createNodeMention').mockRejectedValue(new Error('Create failed'));
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = 'Old [@A](nodespace://123e4567-e89b-12d3-a456-426614174000)';
      const newContent = 'New [@B](nodespace://234e5678-e89b-12d3-a456-426614174000)';

      // Should not throw
      await service.syncMentions('node-123', oldContent, newContent);

      // Delete should still be called even though create failed
      expect(deleteSpy).toHaveBeenCalledWith('node-123', '123e4567-e89b-12d3-a456-426614174000');
    });

    it('should continue even if delete fails', async () => {
      vi.spyOn(mockAdapter, 'deleteNodeMention').mockRejectedValue(new Error('Delete failed'));
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);

      const oldContent = 'Old [@A](nodespace://123e4567-e89b-12d3-a456-426614174000)';
      const newContent = 'New [@B](nodespace://234e5678-e89b-12d3-a456-426614174000)';

      // Should not throw
      await service.syncMentions('node-123', oldContent, newContent);

      // Create should still be called even though delete failed
      expect(createSpy).toHaveBeenCalledWith('node-123', '234e5678-e89b-12d3-a456-426614174000');
    });

    it('should handle multiple simultaneous adds and removes', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = `
        [@A](nodespace://aaa-4567-e89b-12d3-a456-426614174000)
        [@B](nodespace://bbb-5678-e89b-12d3-a456-426614174000)
        [@C](nodespace://ccc-6789-e89b-12d3-a456-426614174000)
      `;

      const newContent = `
        [@B](nodespace://bbb-5678-e89b-12d3-a456-426614174000)
        [@D](nodespace://ddd-7890-e89b-12d3-a456-426614174000)
        [@E](nodespace://eee-8901-e89b-12d3-a456-426614174000)
      `;

      await service.syncMentions('node-123', oldContent, newContent);

      // B should remain, A and C should be deleted, D and E should be added
      expect(createSpy).toHaveBeenCalledTimes(2);
      expect(createSpy).toHaveBeenCalledWith('node-123', 'ddd-7890-e89b-12d3-a456-426614174000');
      expect(createSpy).toHaveBeenCalledWith('node-123', 'eee-8901-e89b-12d3-a456-426614174000');

      expect(deleteSpy).toHaveBeenCalledTimes(2);
      expect(deleteSpy).toHaveBeenCalledWith('node-123', 'aaa-4567-e89b-12d3-a456-426614174000');
      expect(deleteSpy).toHaveBeenCalledWith('node-123', 'ccc-6789-e89b-12d3-a456-426614174000');
    });

    it('should handle mentions with mixed formats in sync', async () => {
      const createSpy = vi.spyOn(mockAdapter, 'createNodeMention').mockResolvedValue(undefined);
      const deleteSpy = vi.spyOn(mockAdapter, 'deleteNodeMention').mockResolvedValue(undefined);

      const oldContent = 'Old [@A](nodespace://aaa-4567-e89b-12d3-a456-426614174000)';
      const newContent = 'New nodespace://bbb-5678-e89b-12d3-a456-426614174000 link';

      await service.syncMentions('node-123', oldContent, newContent);

      expect(createSpy).toHaveBeenCalledWith('node-123', 'bbb-5678-e89b-12d3-a456-426614174000');
      expect(deleteSpy).toHaveBeenCalledWith('node-123', 'aaa-4567-e89b-12d3-a456-426614174000');
    });
  });
});
