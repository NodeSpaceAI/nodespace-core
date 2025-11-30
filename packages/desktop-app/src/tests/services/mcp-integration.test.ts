/**
 * MCP Integration Tests (Phase 3)
 *
 * These tests simulate MCP server behavior to validate the architecture
 * is ready for MCP integration (Issue #112). Tests use SharedNodeStore
 * directly with domain events architecture for real-time sync.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource, NodeUpdate } from '../../lib/types/update-protocol';

describe('Phase 3: MCP Integration (Simulated)', () => {
  let store: SharedNodeStore;
  const mockNode: Node = {
    id: 'test-node-1',
    nodeType: 'text',
    content: 'Initial content',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  };

  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-1'
  };

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
  });

  // ========================================================================
  // Basic MCP Update Handling
  // ========================================================================

  describe('handleExternalUpdate', () => {
    it('should handle MCP server updates', () => {
      // Setup: Create initial node
      store.setNode(mockNode, viewerSource, true);

      // Simulate MCP server update
      const mcpUpdate: NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Updated by AI agent' },
        source: { type: 'mcp-server', serverId: 'test-server', agentId: 'agent-1' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('mcp-server', mcpUpdate);

      // Verify update was applied
      const updated = store.getNode(mockNode.id);
      expect(updated?.content).toBe('Updated by AI agent');
    });

    it('should handle database sync updates', () => {
      store.setNode(mockNode, viewerSource, true);

      const dbUpdate: NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Synced from database' },
        source: { type: 'database', reason: 'sync' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('database', dbUpdate);

      const updated = store.getNode(mockNode.id);
      expect(updated?.content).toBe('Synced from database');
    });

    it('should handle generic external updates', () => {
      store.setNode(mockNode, viewerSource, true);

      const externalUpdate: NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'External system update' },
        source: { type: 'mcp-server', serverId: 'sync-service' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('external', externalUpdate);

      const updated = store.getNode(mockNode.id);
      expect(updated?.content).toBe('External system update');
    });

    it('should warn for updates to non-existent nodes', () => {
      const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const mcpUpdate: NodeUpdate = {
        nodeId: 'non-existent',
        changes: { content: 'Update' },
        source: { type: 'mcp-server' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('mcp-server', mcpUpdate);

      expect(consoleSpy).toHaveBeenCalledWith(
        expect.stringContaining('External update for non-existent node')
      );

      consoleSpy.mockRestore();
    });
  });

  // ========================================================================
  // Multi-Viewer Propagation
  // ========================================================================

  describe('Multi-Viewer Synchronization', () => {
    it('should propagate MCP updates to all subscribed viewers', () => {
      store.setNode(mockNode, viewerSource, true);

      // Create multiple viewer subscriptions
      const viewer1Updates: Node[] = [];
      const viewer2Updates: Node[] = [];
      const viewer3Updates: Node[] = [];

      store.subscribe(mockNode.id, (node) => viewer1Updates.push(node));
      store.subscribe(mockNode.id, (node) => viewer2Updates.push(node));
      store.subscribe(mockNode.id, (node) => viewer3Updates.push(node));

      // Simulate MCP server update
      const mcpUpdate: NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'AI agent updated this node' },
        source: { type: 'mcp-server', serverId: 'mcp-1' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('mcp-server', mcpUpdate);

      // All viewers should receive the update
      expect(viewer1Updates).toHaveLength(1);
      expect(viewer2Updates).toHaveLength(1);
      expect(viewer3Updates).toHaveLength(1);

      expect(viewer1Updates[0].content).toBe('AI agent updated this node');
      expect(viewer2Updates[0].content).toBe('AI agent updated this node');
      expect(viewer3Updates[0].content).toBe('AI agent updated this node');
    });

    it('should integrate with ReactiveNodeService viewer instances', () => {
      store.setNode(mockNode, viewerSource, true);

      // Create viewer subscriptions (simulating what ReactiveNodeService does)
      const viewerUpdates: Node[] = [];
      store.subscribe(mockNode.id, (node) => viewerUpdates.push(node));

      // Simulate MCP update
      store.updateNode(
        mockNode.id,
        { content: 'MCP updated via SharedNodeStore' },
        { type: 'mcp-server' },
        { skipPersistence: true }
      );

      // Viewer subscription should have been notified
      expect(viewerUpdates).toHaveLength(1);
      expect(viewerUpdates[0].content).toBe('MCP updated via SharedNodeStore');

      // The SharedNodeStore should have the update
      const storeNode = store.getNode(mockNode.id);
      expect(storeNode?.content).toBe('MCP updated via SharedNodeStore');
    });
  });

  // ========================================================================
  // Conflict Detection with MCP Updates
  // ========================================================================

  describe('MCP Update Conflicts', () => {
    it('should handle local edit and MCP update sequence', () => {
      store.setNode(mockNode, viewerSource, true);

      // Simulate local edit (not persisted yet)
      store.updateNode(mockNode.id, { content: 'Local edit in progress' }, viewerSource, {
        skipPersistence: true,
        skipConflictDetection: true
      });

      // Verify first update applied
      expect(store.getNode(mockNode.id)?.content).toBe('Local edit in progress');

      // Simulate MCP update
      store.updateNode(
        mockNode.id,
        { content: 'AI agent concurrent edit' },
        { type: 'mcp-server' },
        { skipPersistence: true, skipConflictDetection: true }
      );

      // The MCP update should win (applied last)
      const final = store.getNode(mockNode.id);
      expect(final?.content).toBe('AI agent concurrent edit');
    });
  });

  // ========================================================================
  // Performance and Batching
  // ========================================================================

  describe('MCP Update Performance', () => {
    it('should handle high-frequency MCP updates efficiently', () => {
      // Create multiple nodes
      const nodes: Node[] = [];
      for (let i = 0; i < 100; i++) {
        const node: Node = {
          ...mockNode,
          id: `node-${i}`,
          content: `Content ${i}`
        };
        nodes.push(node);
        store.setNode(node, viewerSource, true);
      }

      // Simulate high-frequency MCP updates
      const startTime = performance.now();

      for (let i = 0; i < 100; i++) {
        const mcpUpdate: NodeUpdate = {
          nodeId: `node-${i}`,
          changes: { content: `AI updated ${i}` },
          source: { type: 'mcp-server' },
          timestamp: Date.now()
        };

        store.handleExternalUpdate('mcp-server', mcpUpdate);
      }

      const duration = performance.now() - startTime;

      // Should complete within reasonable time (< 100ms for 100 updates)
      expect(duration).toBeLessThan(100);

      // Verify all updates applied
      for (let i = 0; i < 100; i++) {
        const updated = store.getNode(`node-${i}`);
        expect(updated?.content).toBe(`AI updated ${i}`);
      }
    });

    it('should track metrics for MCP updates', () => {
      // Create multiple nodes for independent updates
      const nodes: Node[] = [];
      for (let i = 0; i < 5; i++) {
        const node: Node = {
          ...mockNode,
          id: `metrics-node-${i}`,
          content: `Content ${i}`
        };
        nodes.push(node);
        store.setNode(node, viewerSource, true);
      }

      const initialMetrics = store.getMetrics();

      // Apply MCP updates to different nodes
      for (let i = 0; i < 5; i++) {
        const mcpUpdate: NodeUpdate = {
          nodeId: `metrics-node-${i}`,
          changes: { content: `MCP Update ${i}` },
          source: { type: 'mcp-server' },
          timestamp: Date.now()
        };

        store.handleExternalUpdate('mcp-server', mcpUpdate);
      }

      const finalMetrics = store.getMetrics();

      // Should have tracked the updates
      expect(finalMetrics.updateCount).toBe(initialMetrics.updateCount + 5);
      expect(finalMetrics.avgUpdateTime).toBeGreaterThan(0);
    });
  });

  // ========================================================================
  // Integration Readiness
  // ========================================================================

  describe('MCP Integration Readiness', () => {
    it('should provide clear integration point for MCP server', () => {
      store.setNode(mockNode, viewerSource, true);

      // Simulate what MCP integration would do
      const simulateMCPServerEvent = (update: NodeUpdate) => {
        store.handleExternalUpdate('mcp-server', update);
      };

      // Test the simulation
      simulateMCPServerEvent({
        nodeId: mockNode.id,
        changes: { content: 'MCP server integration test' },
        source: { type: 'mcp-server', serverId: 'future-mcp-server' },
        timestamp: Date.now()
      });

      const result = store.getNode(mockNode.id);
      expect(result?.content).toBe('MCP server integration test');
    });

    it('should handle MCP server reconnection with batch updates', () => {
      store.setNode(mockNode, viewerSource, true);

      // Simulate disconnection (no updates)
      const beforeDisconnect = store.getNode(mockNode.id);

      // Simulate reconnection with batch of updates (applied sequentially)
      const batchUpdates = [
        'Update during disconnect 1',
        'Update during disconnect 2',
        'Update during disconnect 3'
      ];

      for (const content of batchUpdates) {
        store.updateNode(
          mockNode.id,
          { content },
          { type: 'mcp-server' },
          { skipPersistence: true, skipConflictDetection: true }
        );
      }

      // Should have applied all updates (last one wins)
      const afterReconnect = store.getNode(mockNode.id);
      expect(afterReconnect?.content).toBe('Update during disconnect 3');
      expect(beforeDisconnect).not.toEqual(afterReconnect);
    });
  });
});
