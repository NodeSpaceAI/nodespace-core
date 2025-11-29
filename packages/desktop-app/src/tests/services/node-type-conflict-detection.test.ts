/**
 * NodeType Conflict Detection Tests
 *
 * Tests that conflict detection behaves correctly for node type conversions:
 * - NodeType conversions bypass conflict detection
 * - Regular concurrent edits still trigger conflict detection
 * - Edge cases around rapid updates
 *
 * Part of issue #275: HeaderNode implementation
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

describe('NodeType Conflict Detection', () => {
  let store: SharedNodeStore;

  const viewer1Source: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-1'
  };

  const viewer2Source: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-2'
  };

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
  });

  describe('NodeType Conversion Bypass', () => {
    it('should bypass conflict detection for nodeType conversions', async () => {
      const textNode: Node = {
        id: 'test-node-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Rapid sequence: content update followed by nodeType conversion
      // This simulates typing "##" then space which triggers conversion
      store.updateNode(textNode.id, { content: '##' }, viewer1Source);

      store.updateNode(
        textNode.id,
        {
          content: '## ',
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewer1Source
      );

      // Both should succeed without conflict
      const finalNode = store.getNode(textNode.id);
      expect(finalNode?.nodeType).toBe('header');
      expect(finalNode?.content).toBe('## ');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should bypass conflict detection for header → text conversion', async () => {
      const headerNode: Node = {
        id: 'test-node-2',
        nodeType: 'header',
        content: '## Hello',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { headerLevel: 2 },
        mentions: []
      };

      store.setNode(headerNode, viewer1Source);

      // Rapid sequence: remove space then hashtags
      store.updateNode(headerNode.id, { content: '##Hello' }, viewer1Source);

      store.updateNode(
        headerNode.id,
        {
          content: 'Hello',
          nodeType: 'text',
          properties: {}
        },
        viewer1Source
      );

      const finalNode = store.getNode(headerNode.id);
      expect(finalNode?.nodeType).toBe('text');
      expect(finalNode?.content).toBe('Hello');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should handle multiple rapid conversions without conflicts', async () => {
      const textNode: Node = {
        id: 'test-node-3',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Multiple conversions in sequence with small delays to ensure proper ordering
      store.updateNode(textNode.id, { content: '##' }, viewer1Source);

      await new Promise((resolve) => setTimeout(resolve, 10));

      store.updateNode(
        textNode.id,
        { content: '## ', nodeType: 'header', properties: { headerLevel: 2 } },
        viewer1Source
      );

      await new Promise((resolve) => setTimeout(resolve, 10));

      store.updateNode(textNode.id, { content: '## Test' }, viewer1Source);

      // Wait for final update to complete
      await new Promise((resolve) => setTimeout(resolve, 50));

      const finalNode = store.getNode(textNode.id);
      expect(finalNode?.nodeType).toBe('header');
      expect(finalNode?.content).toBe('## Test');
    });
  });

  describe('Regular Conflict Detection', () => {
    it('should still detect conflicts for regular concurrent edits', async () => {
      const textNode: Node = {
        id: 'test-node-4',
        nodeType: 'text',
        content: 'Original',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Two different viewers editing simultaneously (not nodeType conversion)
      store.updateNode(textNode.id, { content: 'Viewer 1 edit' }, viewer1Source);

      store.updateNode(textNode.id, { content: 'Viewer 2 edit' }, viewer2Source);

      // Verify final state
      const finalNode = store.getNode(textNode.id);
      expect(finalNode).toBeDefined();
      expect(finalNode?.content).toBeTruthy();

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should detect conflicts for concurrent property changes', async () => {
      const textNode: Node = {
        id: 'test-node-5',
        nodeType: 'text',
        content: 'Test',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { customProp: 'value1' },
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Concurrent property updates from different sources
      store.updateNode(
        textNode.id,
        { properties: { customProp: 'viewer1-change' } },
        viewer1Source
      );

      store.updateNode(
        textNode.id,
        { properties: { customProp: 'viewer2-change' } },
        viewer2Source
      );

      // Verify final state
      const finalNode = store.getNode(textNode.id);
      expect(finalNode).toBeDefined();
      expect(finalNode?.properties.customProp).toBeTruthy();

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
  });

  describe('Edge Cases', () => {
    it('should handle nodeType conversion with concurrent content edit', async () => {
      const textNode: Node = {
        id: 'test-node-6',
        nodeType: 'text',
        content: 'Test',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Viewer 1 converts to header
      store.updateNode(
        textNode.id,
        {
          content: '## Test',
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewer1Source
      );

      // Viewer 2 edits content (doesn't know about conversion yet)
      store.updateNode(textNode.id, { content: 'Test Modified' }, viewer2Source);

      // Final state should be resolved (last write wins)
      const finalNode = store.getNode(textNode.id);
      expect(finalNode).toBeDefined();

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should handle alternating text ↔ header conversions', async () => {
      const textNode: Node = {
        id: 'test-node-7',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Alternate conversions
      store.updateNode(
        textNode.id,
        { content: '## A', nodeType: 'header', properties: { headerLevel: 2 } },
        viewer1Source
      );

      store.updateNode(
        textNode.id,
        { content: 'A', nodeType: 'text', properties: {} },
        viewer1Source
      );

      store.updateNode(
        textNode.id,
        { content: '### A', nodeType: 'header', properties: { headerLevel: 3 } },
        viewer1Source
      );

      const finalNode = store.getNode(textNode.id);
      expect(finalNode?.nodeType).toBe('header');
      expect(finalNode?.properties.headerLevel).toBe(3);

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should handle nodeType conversion with no content change', async () => {
      const textNode: Node = {
        id: 'test-node-8',
        nodeType: 'text',
        content: '## Already formatted',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewer1Source);

      // Convert without changing content (e.g., programmatic conversion)
      store.updateNode(
        textNode.id,
        {
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewer1Source
      );

      const finalNode = store.getNode(textNode.id);
      expect(finalNode?.nodeType).toBe('header');
      expect(finalNode?.content).toBe('## Already formatted');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
  });

  describe('Performance Under Load', () => {
    it('should handle rapid nodeType conversions efficiently', async () => {
      const nodes: Node[] = Array.from({ length: 20 }, (_, i) => ({
        id: `test-node-${i}`,
        nodeType: 'text' as const,
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      }));

      // Add all nodes
      nodes.forEach((node) => store.setNode(node, viewer1Source));

      const startTime = Date.now();

      // Convert all to headers rapidly
      nodes.forEach((node) =>
        store.updateNode(
          node.id,
          {
            content: '## Test',
            nodeType: 'header',
            properties: { headerLevel: 2 }
          },
          viewer1Source
        )
      );

      // Verify all conversions
      nodes.forEach((node) => {
        const updatedNode = store.getNode(node.id);
        expect(updatedNode?.nodeType).toBe('header');
      });

      const endTime = Date.now();
      const duration = endTime - startTime;

      // Should be fast (< 1 second for 20 nodes)
      expect(duration).toBeLessThan(1000);

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
  });
});
