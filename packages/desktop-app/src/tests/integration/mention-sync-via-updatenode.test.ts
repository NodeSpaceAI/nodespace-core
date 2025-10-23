/**
 * Integration test for Issue #341: Fix mention sync in HTTP dev mode
 *
 * This test verifies that mention sync works correctly when content is updated
 * via SharedNodeStore.updateNode() (as happens during PATCH operations in HTTP dev mode),
 * not just via setNode().
 *
 * Background:
 * - The original implementation only had mention sync in setNode()
 * - In HTTP dev mode, content updates go through updateNode() via PATCH requests
 * - This meant mentions were never being synced in HTTP dev mode
 *
 * Solution:
 * - Added mention sync logic to updateNode() as well
 * - Mentions now sync whenever content changes, regardless of update path
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '$lib/services/shared-node-store';
import type { Node } from '$lib/types';

describe('Issue #341: Mention Sync via updateNode()', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    // Reset singleton and get fresh instance
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  // Helper to wait for async operations with polling
  const waitFor = async (
    assertion: () => void,
    options: { timeout?: number; interval?: number } = {}
  ): Promise<void> => {
    const timeout = options.timeout ?? 1000;
    const interval = options.interval ?? 10;
    const startTime = Date.now();

    while (Date.now() - startTime < timeout) {
      try {
        assertion();
        return; // Assertion passed
      } catch {
        // Assertion failed, wait and retry
        await new Promise((resolve) => setTimeout(resolve, interval));
      }
    }

    // Final attempt - this will throw if assertion still fails
    assertion();
  };

  // Helper to create test nodes with all required fields
  const createTestNode = (overrides: Partial<Node> = {}): Node => ({
    id: overrides.id || 'test-node-id',
    content: overrides.content || '',
    nodeType: overrides.nodeType || 'text',
    parentId: overrides.parentId !== undefined ? overrides.parentId : null,
    beforeSiblingId: overrides.beforeSiblingId !== undefined ? overrides.beforeSiblingId : null,
    containerNodeId: overrides.containerNodeId !== undefined ? overrides.containerNodeId : null,
    properties: overrides.properties || {},
    createdAt: overrides.createdAt || new Date().toISOString(),
    modifiedAt: overrides.modifiedAt || new Date().toISOString()
  });

  it('should sync mentions when content is updated via updateNode()', async () => {
    // Create initial node with no mentions
    const sourceNodeId = '11111111-1111-1111-1111-111111111111';
    const mentionedNodeId = '22222222-2222-2222-2222-222222222222';

    const initialNode = createTestNode({
      id: sourceNodeId,
      content: 'Initial content without mentions'
    });

    // Set initial node (this would normally happen on load)
    store.setNode(initialNode, { type: 'database', reason: 'initial-load' }, true);

    // Spy on mentionSyncService.syncMentions to verify it's called
    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Update content to include a mention (simulating PATCH operation in HTTP dev mode)
    const contentWithMention = `Check out [@Other Node](nodespace://${mentionedNodeId}) for details`;

    store.updateNode(
      sourceNodeId,
      { content: contentWithMention },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true } // Skip database persistence for this test
    );

    // Wait for async mention sync to complete
    await waitFor(
      () => {
        expect(syncMentionsSpy).toHaveBeenCalledWith(
          sourceNodeId,
          'Initial content without mentions',
          contentWithMention
        );
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });

  it('should NOT sync mentions when content is unchanged', async () => {
    const nodeId = '33333333-3333-3333-3333-333333333333';

    const node = createTestNode({
      id: nodeId,
      content: 'Same content'
    });

    store.setNode(node, { type: 'database', reason: 'initial-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Update node without changing content (e.g., nodeType change)
    store.updateNode(
      nodeId,
      { nodeType: 'header' },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true }
    );

    // Give time for any potential async calls (though none should happen)
    await new Promise((resolve) => setTimeout(resolve, 10));

    // Verify mention sync was NOT triggered
    expect(syncMentionsSpy).not.toHaveBeenCalled();

    syncMentionsSpy.mockRestore();
  });

  it('should sync mentions when content changes from mention to no mention', async () => {
    const sourceNodeId = '44444444-4444-4444-4444-444444444444';
    const mentionedNodeId = '55555555-5555-5555-5555-555555555555';

    const nodeWithMention = createTestNode({
      id: sourceNodeId,
      content: `See [@Node](nodespace://${mentionedNodeId})`
    });

    store.setNode(nodeWithMention, { type: 'database', reason: 'initial-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Remove mention from content
    const contentWithoutMention = 'No mentions anymore';

    store.updateNode(
      sourceNodeId,
      { content: contentWithoutMention },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true }
    );

    await waitFor(
      () => {
        expect(syncMentionsSpy).toHaveBeenCalledWith(
          sourceNodeId,
          `See [@Node](nodespace://${mentionedNodeId})`,
          contentWithoutMention
        );
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });

  it('should sync mentions when replacing one mention with another', async () => {
    const sourceNodeId = '66666666-6666-6666-6666-666666666666';
    const oldMentionId = '77777777-7777-7777-7777-777777777777';
    const newMentionId = '88888888-8888-8888-8888-888888888888';

    const nodeWithOldMention = createTestNode({
      id: sourceNodeId,
      content: `See [@Old Node](nodespace://${oldMentionId})`
    });

    store.setNode(nodeWithOldMention, { type: 'database', reason: 'initial-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Replace with new mention
    const contentWithNewMention = `See [@New Node](nodespace://${newMentionId})`;

    store.updateNode(
      sourceNodeId,
      { content: contentWithNewMention },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true }
    );

    await waitFor(
      () => {
        expect(syncMentionsSpy).toHaveBeenCalledWith(
          sourceNodeId,
          `See [@Old Node](nodespace://${oldMentionId})`,
          contentWithNewMention
        );
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });

  it('should sync mentions when content changes from mention to empty string', async () => {
    const nodeId = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb';
    const mentionId = 'cccccccc-cccc-cccc-cccc-cccccccccccc';

    const nodeWithMention = createTestNode({
      id: nodeId,
      content: `[@Node](nodespace://${mentionId})`
    });

    store.setNode(nodeWithMention, { type: 'database', reason: 'initial-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Update to empty string
    store.updateNode(
      nodeId,
      { content: '' },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true }
    );

    await waitFor(
      () => {
        expect(syncMentionsSpy).toHaveBeenCalledWith(
          nodeId,
          `[@Node](nodespace://${mentionId})`,
          ''
        );
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });

  it('should sync mentions when content changes from empty string to mention', async () => {
    const nodeId = 'dddddddd-dddd-dddd-dddd-dddddddddddd';
    const mentionId = 'eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee';

    const nodeWithEmptyContent = createTestNode({
      id: nodeId,
      content: ''
    });

    store.setNode(nodeWithEmptyContent, { type: 'database', reason: 'initial-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // Update from empty string to mention
    const contentWithMention = `[@Node](nodespace://${mentionId})`;

    store.updateNode(
      nodeId,
      { content: contentWithMention },
      { type: 'viewer', viewerId: 'test-viewer' },
      { skipPersistence: true }
    );

    await waitFor(
      () => {
        expect(syncMentionsSpy).toHaveBeenCalledWith(nodeId, '', contentWithMention);
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });

  it('should handle mention sync errors gracefully without blocking update', async () => {
    const nodeId = '99999999-9999-9999-9999-999999999999';
    const mentionId = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa';

    const node = createTestNode({
      id: nodeId,
      content: 'No mentions'
    });

    store.setNode(node, { type: 'database', reason: 'initial-load' }, true);

    // Mock syncMentions to throw an error
    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi
      .spyOn(mentionSyncService, 'syncMentions')
      .mockRejectedValue(new Error('Mention sync failed'));

    // Spy on console.error to verify error is logged
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    // Update content with mention
    const contentWithMention = `See [@Node](nodespace://${mentionId})`;

    // This should not throw even though mention sync fails
    expect(() => {
      store.updateNode(
        nodeId,
        { content: contentWithMention },
        { type: 'viewer', viewerId: 'test-viewer' },
        { skipPersistence: true }
      );
    }).not.toThrow();

    // Wait for async error handling
    await waitFor(
      () => {
        expect(consoleErrorSpy).toHaveBeenCalledWith(
          '[SharedNodeStore] Failed to sync mentions for node',
          nodeId,
          expect.any(Error)
        );
      },
      { timeout: 100 }
    );

    // Verify node was still updated
    const updatedNode = store.getNode(nodeId);
    expect(updatedNode?.content).toBe(contentWithMention);

    syncMentionsSpy.mockRestore();
    consoleErrorSpy.mockRestore();
  });

  it('should work correctly in HTTP dev mode simulation (PATCH workflow)', async () => {
    // This test simulates the exact scenario from issue #341:
    // 1. User creates a node
    // 2. User edits content to add @mention
    // 3. Content is saved via PATCH (updateNode), not setNode
    // 4. Mention relationship should be created

    const dateNodeId = 'date-node-123';
    const childNodeId = 'child-node-456';
    const mentionedNodeId = 'mentioned-node-789';

    // Simulate HTTP dev mode: node is loaded from database
    const childNode = createTestNode({
      id: childNodeId,
      content: '',
      parentId: dateNodeId,
      containerNodeId: dateNodeId
    });

    // Load node into store (simulating GET response)
    store.setNode(childNode, { type: 'database', reason: 'http-load' }, true);

    const { mentionSyncService } = await import('$lib/services/mention-sync-service');
    const syncMentionsSpy = vi.spyOn(mentionSyncService, 'syncMentions');

    // User types @ and selects a node (simulating PATCH request)
    const updatedContent = `[@Some New Node](nodespace://${mentionedNodeId})`;

    // This mimics the PATCH flow in HTTP dev mode
    store.updateNode(
      childNodeId,
      { content: updatedContent },
      { type: 'viewer', viewerId: 'date-viewer' },
      { skipPersistence: true } // Skip DB for test
    );

    await waitFor(
      () => {
        // CRITICAL: Verify mention sync was called
        // This was the bug - mention sync was NOT being called in this scenario
        expect(syncMentionsSpy).toHaveBeenCalledWith(childNodeId, '', updatedContent);
      },
      { timeout: 100 }
    );

    syncMentionsSpy.mockRestore();
  });
});
