/**
 * MentionSyncService Unit Tests
 *
 * Tests link text synchronization and broken link handling
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { MentionSyncService } from '../../lib/services/mention-sync-service';
import { eventBus } from '../../lib/services/event-bus';
import type { TauriNodeService } from '../../lib/services/tauri-node-service';
import type { Node } from '../../lib/types';
import type { NodeUpdatedEvent, NodeDeletedEvent } from '../../lib/services/event-types';

describe('MentionSyncService', () => {
  let mentionSyncService: MentionSyncService;
  let mockDatabaseService: Partial<TauriNodeService>;
  let mockNodes: Map<string, Node>;

  beforeEach(() => {
    // Reset mock nodes
    mockNodes = new Map();

    // Create mock database service
    mockDatabaseService = {
      getNode: vi.fn(async (nodeId: string) => {
        return mockNodes.get(nodeId) || null;
      }),
      updateNode: vi.fn(async (nodeId: string, version: number, updates: { content?: string }) => {
        const node = mockNodes.get(nodeId);
        if (node) {
          Object.assign(node, updates);
          node.version = version + 1;
        }
        return node!;
      }),
      queryNodes: vi.fn(async (query: { mentionedBy?: string }) => {
        if (query.mentionedBy) {
          const mentionedNodeId = query.mentionedBy;
          return Array.from(mockNodes.values()).filter((node) =>
            node.content.includes(`nodespace://${mentionedNodeId}`)
          );
        }
        return [];
      })
    };

    mentionSyncService = new MentionSyncService(mockDatabaseService as TauriNodeService);
    // Reset metrics for each test
    mentionSyncService.resetMetrics();
  });

  afterEach(() => {
    // Clear event bus subscriptions to avoid test pollution
    mockNodes.clear();
  });

  describe('Link Parsing', () => {
    it('should parse markdown links correctly', () => {
      const content = 'See [Buy groceries](nodespace://task-123) for details.';
      const links = mentionSyncService.parseAllLinks(content);

      expect(links).toHaveLength(1);
      expect(links[0].displayText).toBe('Buy groceries');
      expect(links[0].nodeId).toBe('task-123');
      expect(links[0].uri).toBe('nodespace://task-123');
    });

    it('should parse multiple links in content', () => {
      const content =
        'Check [Task A](nodespace://task-1) and [Task B](nodespace://task-2) today.';
      const links = mentionSyncService.parseAllLinks(content);

      expect(links).toHaveLength(2);
      expect(links[0].nodeId).toBe('task-1');
      expect(links[1].nodeId).toBe('task-2');
    });

    it('should handle links with query parameters', () => {
      const content = 'See [Task](nodespace://task-123?deleted=true) for details.';
      const links = mentionSyncService.parseAllLinks(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('task-123');
    });

    it('should handle links with special characters in display text', () => {
      const content = 'Check [Task: Buy groceries & items](nodespace://task-123) today.';
      const links = mentionSyncService.parseAllLinks(content);

      expect(links).toHaveLength(1);
      expect(links[0].displayText).toBe('Task: Buy groceries & items');
    });
  });

  describe('Link Text Synchronization', () => {
    it('should update link text when node title changes', async () => {
      // Setup: Create a task node and a note that mentions it
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Shopping list',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'Remember to check [Buy groceries](nodespace://task-123) before going out.',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      // Action: Sync link text
      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      // Assert: Link text should be updated
      expect(results).toHaveLength(1);
      expect(results[0].nodeId).toBe('note-456');
      expect(results[0].linksUpdated).toBe(1);
      expect(results[0].newContent).toContain('[Shopping list](nodespace://task-123)');
      expect(mockDatabaseService.updateNode).toHaveBeenCalledWith('note-456', 1, {
        content: expect.stringContaining('[Shopping list](nodespace://task-123)')
      });
    });

    it('should update multiple links in same node', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Updated Task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content:
          'First: [Old Task](nodespace://task-123), Second: [Old Task](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      expect(results).toHaveLength(1);
      expect(results[0].linksUpdated).toBe(2);
      expect(results[0].newContent).toContain('[Updated Task](nodespace://task-123)');
      expect((results[0].newContent.match(/\[Updated Task\]/g) || []).length).toBe(2);
    });

    it('should update links across multiple nodes', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'New Title',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const note1: Node = {
        id: 'note-1',
        nodeType: 'text',
        content: 'See [Old Title](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      const note2: Node = {
        id: 'note-2',
        nodeType: 'text',
        content: 'Check [Old Title](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-1', note1);
      mockNodes.set('note-2', note2);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      expect(results).toHaveLength(2);
      expect(results.every((r) => r.linksUpdated === 1)).toBe(true);
      expect(mockNodes.get('note-1')?.content).toContain('[New Title](nodespace://task-123)');
      expect(mockNodes.get('note-2')?.content).toContain('[New Title](nodespace://task-123)');
    });

    it('should not update if title has not changed', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Same Title',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Same Title](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      expect(results).toHaveLength(1);
      expect(results[0].linksUpdated).toBe(0);
      expect(mockDatabaseService.updateNode).not.toHaveBeenCalled();
    });

    it('should handle nodes with no mentioning nodes', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Standalone Task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      expect(results).toHaveLength(0);
      expect(mockDatabaseService.updateNode).not.toHaveBeenCalled();
    });

    it('should extract title from markdown headers', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: '## Important Task\nWith additional details',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Old Title](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');

      expect(results[0].newContent).toContain('[Important Task](nodespace://task-123)');
    });
  });

  describe('Broken Link Handling', () => {
    it('should mark links as broken when node is deleted', async () => {
      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Shopping list](nodespace://task-123) for details.',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.markLinksAsBroken('task-123');

      expect(results).toHaveLength(1);
      expect(results[0].nodeId).toBe('note-456');
      expect(results[0].linksUpdated).toBe(1);
      expect(results[0].newContent).toContain('[Deleted Node](nodespace://task-123?deleted=true)');
    });

    it('should mark multiple broken links in same node', async () => {
      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'First: [Task A](nodespace://task-123), Second: [Task A](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.markLinksAsBroken('task-123');

      expect(results[0].linksUpdated).toBe(2);
      expect((results[0].newContent.match(/\[Deleted Node\]/g) || []).length).toBe(2);
    });

    it('should mark broken links across multiple nodes', async () => {
      const note1: Node = {
        id: 'note-1',
        nodeType: 'text',
        content: 'See [Task](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      const note2: Node = {
        id: 'note-2',
        nodeType: 'text',
        content: 'Check [Task](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('note-1', note1);
      mockNodes.set('note-2', note2);

      const results = await mentionSyncService.markLinksAsBroken('task-123');

      expect(results).toHaveLength(2);
      expect(results.every((r) => r.linksUpdated === 1)).toBe(true);
      expect(mockNodes.get('note-1')?.content).toContain(
        '[Deleted Node](nodespace://task-123?deleted=true)'
      );
      expect(mockNodes.get('note-2')?.content).toContain(
        '[Deleted Node](nodespace://task-123?deleted=true)'
      );
    });
  });

  describe('EventBus Integration', () => {
    it('should listen for node:updated events', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Updated Task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Old Task](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      // Spy on syncLinkTextForNode
      const syncSpy = vi.spyOn(mentionSyncService, 'syncLinkTextForNode');

      // Emit node:updated event
      const updateEvent: NodeUpdatedEvent = {
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'task-123',
        updateType: 'content',
        metadata: {}
      };

      eventBus.emit(updateEvent);

      // Wait for async event handling
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(syncSpy).toHaveBeenCalledWith('task-123');
    });

    it('should listen for node:deleted events', async () => {
      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Task](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('note-456', noteNode);

      // Spy on markLinksAsBroken
      const markSpy = vi.spyOn(mentionSyncService, 'markLinksAsBroken');

      // Emit node:deleted event
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'task-123',
        metadata: {}
      };

      eventBus.emit(deleteEvent);

      // Wait for async event handling
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(markSpy).toHaveBeenCalledWith('task-123');
    });

    it('should emit node:updated events after syncing', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'New Title',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Old Title](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      // Listen for emitted events
      const emittedEvents: NodeUpdatedEvent[] = [];
      const unsubscribe = eventBus.subscribe('node:updated', (event) => {
        const nodeEvent = event as NodeUpdatedEvent;
        if (nodeEvent.source === 'MentionSyncService') {
          emittedEvents.push(nodeEvent);
        }
      });

      await mentionSyncService.syncLinkTextForNode('task-123');

      // Wait for event emission
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(emittedEvents).toHaveLength(1);
      expect(emittedEvents[0].nodeId).toBe('note-456');
      expect(emittedEvents[0].updateType).toBe('metadata'); // Changed to 'metadata' to avoid re-triggering sync
      expect(emittedEvents[0].metadata?.reason).toBe('link-text-sync');

      unsubscribe();
    });
  });

  describe('Performance Metrics', () => {
    it('should track sync metrics', async () => {
      // Create a fresh instance without EventBus subscriptions to avoid cross-test pollution
      const freshMockDatabase: Partial<TauriNodeService> = {
        getNode: vi.fn(async (nodeId: string) => {
          return mockNodes.get(nodeId) || null;
        }),
        updateNode: vi.fn(async (nodeId: string, version: number, updates: { content?: string }) => {
          const node = mockNodes.get(nodeId);
          if (node) {
            Object.assign(node, updates);
            node.version = version + 1;
          }
          return node!;
        }),
        queryNodes: vi.fn(async (query: { mentionedBy?: string }) => {
          if (query.mentionedBy) {
            const mentionedNodeId = query.mentionedBy;
            return Array.from(mockNodes.values()).filter((node) =>
              node.content.includes(`nodespace://${mentionedNodeId}`)
            );
          }
          return [];
        })
      };

      const freshService = new MentionSyncService(freshMockDatabase as TauriNodeService);

      const taskNode: Node = {
        id: 'task-999',
        nodeType: 'task',
        content: 'Task Title',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-888',
        nodeType: 'text',
        content: 'See [Old](nodespace://task-999)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-999'],
        properties: {}
      };

      mockNodes.set('task-999', taskNode);
      mockNodes.set('note-888', noteNode);

      const initialMetrics = freshService.getMetrics();
      expect(initialMetrics.totalSyncs).toBe(0);

      await freshService.syncLinkTextForNode('task-999');

      const updatedMetrics = freshService.getMetrics();
      expect(updatedMetrics.totalSyncs).toBe(1);
      expect(updatedMetrics.totalLinksUpdated).toBe(1);
      expect(updatedMetrics.totalNodesUpdated).toBe(1);
      expect(updatedMetrics.avgSyncTime).toBeGreaterThan(0);
    });

    it('should allow resetting metrics', () => {
      mentionSyncService.resetMetrics();
      const metrics = mentionSyncService.getMetrics();

      expect(metrics.totalSyncs).toBe(0);
      expect(metrics.totalLinksUpdated).toBe(0);
      expect(metrics.totalNodesUpdated).toBe(0);
      expect(metrics.avgSyncTime).toBe(0);
    });
  });

  describe('Edge Cases', () => {
    it('should handle missing nodes gracefully', async () => {
      const results = await mentionSyncService.syncLinkTextForNode('nonexistent-123');
      expect(results).toHaveLength(0);
    });

    it('should handle empty content', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');
      expect(results).toHaveLength(0);
    });

    it('should handle content with special regex characters', async () => {
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: 'Task (with) [special] {chars}',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Old](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');
      expect(results[0].newContent).toContain(
        '[Task (with) [special] {chars}](nodespace://task-123)'
      );
    });

    it('should handle very long titles', async () => {
      const longTitle = 'A'.repeat(150);
      const taskNode: Node = {
        id: 'task-123',
        nodeType: 'task',
        content: longTitle,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: [],
        properties: {}
      };

      const noteNode: Node = {
        id: 'note-456',
        nodeType: 'text',
        content: 'See [Old](nodespace://task-123)',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        beforeSiblingId: null,
        version: 1,
        mentions: ['task-123'],
        properties: {}
      };

      mockNodes.set('task-123', taskNode);
      mockNodes.set('note-456', noteNode);

      const results = await mentionSyncService.syncLinkTextForNode('task-123');
      // Title should be truncated to 100 characters
      expect(results[0].newContent).toContain(`[${'A'.repeat(100)}](nodespace://task-123)`);
    });
  });
});
