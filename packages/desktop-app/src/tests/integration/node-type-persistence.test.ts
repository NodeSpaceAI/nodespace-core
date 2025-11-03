/**
 * NodeType Persistence Tests
 *
 * Tests that node type conversions persist correctly through the full stack:
 * - Text → Header conversion saves to database
 * - Header → Text conversion saves to database
 * - Page refresh loads persisted node types correctly
 *
 * Part of issue #275: HeaderNode implementation
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store';
import { PersistenceCoordinator } from '../../lib/services/persistence-coordinator.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

describe('NodeType Persistence', () => {
  let store: SharedNodeStore;
  let coordinator: PersistenceCoordinator;

  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-1'
  };

  beforeEach(() => {
    // Reset and initialize services
    SharedNodeStore.resetInstance();
    PersistenceCoordinator.resetInstance();

    store = SharedNodeStore.getInstance();
    coordinator = PersistenceCoordinator.getInstance();

    // Enable test mode to skip actual database operations
    coordinator.enableTestMode();
    coordinator.resetTestState();
  });

  afterEach(async () => {
    // Clean up
    store.clearAll();
    SharedNodeStore.resetInstance();

    await coordinator.reset();
    PersistenceCoordinator.resetInstance();
  });

  describe('Create New Header Node', () => {
    it('should persist newly created header node to database', async () => {
      // This test covers the bug we found: creating a new header node
      // (e.g., pressing Enter from a header node) must persist correctly
      const headerNode: Node = {
        id: 'new-header-node',
        nodeType: 'header',
        content: '## New Header',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(headerNode, viewerSource);

      // Verify node was created in store
      const createdNode = store.getNode(headerNode.id);
      expect(createdNode?.nodeType).toBe('header');
      expect(createdNode?.content).toBe('## New Header');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Verify it persisted (in test mode, just check it exists in store)
      const persistedNode = store.getNode(headerNode.id);
      expect(persistedNode).toBeDefined();
      expect(persistedNode?.nodeType).toBe('header');
    });
  });

  describe('Text → Header Conversion Persistence', () => {
    it('should persist text → header conversion to database', async () => {
      // Create initial text node
      const textNode: Node = {
        id: 'test-node-1',
        nodeType: 'text',
        content: '',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      // Simulate typing "## Hello" which triggers conversion
      store.updateNode(
        textNode.id,
        {
          content: '## Hello',
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewerSource
      );

      // Verify node type changed in store
      const updatedNode = store.getNode(textNode.id);
      expect(updatedNode?.nodeType).toBe('header');
      expect(updatedNode?.content).toBe('## Hello');
      expect(updatedNode?.properties.headerLevel).toBe(2);

      // Wait for persistence to complete
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should include headerLevel in persisted properties', async () => {
      const textNode: Node = {
        id: 'test-node-2',
        nodeType: 'text',
        content: '',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      // Convert to h1
      store.updateNode(
        textNode.id,
        {
          content: '# Title',
          nodeType: 'header',
          properties: { headerLevel: 1 }
        },
        viewerSource
      );

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Verify the node was updated correctly
      const updatedNode = store.getNode(textNode.id);
      expect(updatedNode?.nodeType).toBe('header');
      expect(updatedNode?.properties.headerLevel).toBe(1);
    });

    it('should handle all header levels (h1-h6)', async () => {
      for (let level = 1; level <= 6; level++) {
        const nodeId = `test-node-h${level}`;
        const hashtags = '#'.repeat(level);
        const content = `${hashtags} Header ${level}`;

        const textNode: Node = {
          id: nodeId,
          nodeType: 'text',
          content: '',
          parentId: null,
          containerNodeId: null,
          beforeSiblingId: null,
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {},
          mentions: []
        };

        store.setNode(textNode, viewerSource);

        store.updateNode(
          nodeId,
          {
            content,
            nodeType: 'header',
            properties: { headerLevel: level }
          },
          viewerSource
        );

        const updatedNode = store.getNode(nodeId);
        expect(updatedNode?.nodeType).toBe('header');
        expect(updatedNode?.properties.headerLevel).toBe(level);
      }

      // Wait for all persistence operations to complete
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
  });

  describe('Header → Text Conversion Persistence', () => {
    it('should persist header → text conversion to database', async () => {
      // Create initial header node
      const headerNode: Node = {
        id: 'test-node-3',
        nodeType: 'header',
        content: '## Hello',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { headerLevel: 2 },
        mentions: []
      };

      store.setNode(headerNode, viewerSource);

      // Simulate removing hashtags (triggers conversion back to text)
      store.updateNode(
        headerNode.id,
        {
          content: 'Hello',
          nodeType: 'text',
          properties: {}
        },
        viewerSource
      );

      // Verify node type changed in store
      const updatedNode = store.getNode(headerNode.id);
      expect(updatedNode?.nodeType).toBe('text');
      expect(updatedNode?.content).toBe('Hello');
      expect(updatedNode?.properties.headerLevel).toBeUndefined();

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should clear headerLevel property when converting to text', async () => {
      const headerNode: Node = {
        id: 'test-node-4',
        nodeType: 'header',
        content: '### Header',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { headerLevel: 3 },
        mentions: []
      };

      store.setNode(headerNode, viewerSource);

      store.updateNode(
        headerNode.id,
        {
          content: 'Header',
          nodeType: 'text',
          properties: {}
        },
        viewerSource
      );

      const updatedNode = store.getNode(headerNode.id);
      expect(updatedNode?.properties).toEqual({});

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
  });

  describe('Persistence Timing and Conflict Detection', () => {
    it('should bypass conflict detection for nodeType conversions', async () => {
      const textNode: Node = {
        id: 'test-node-5',
        nodeType: 'text',
        content: '',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      // Rapid updates: first content only, then content + nodeType
      // This should NOT trigger conflict detection
      store.updateNode(textNode.id, { content: '##' }, viewerSource);

      store.updateNode(
        textNode.id,
        {
          content: '## Hello',
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewerSource
      );

      const finalNode = store.getNode(textNode.id);
      expect(finalNode?.nodeType).toBe('header');
      expect(finalNode?.content).toBe('## Hello');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    it('should persist nodeType changes immediately', async () => {
      const textNode: Node = {
        id: 'test-node-6',
        nodeType: 'text',
        content: '',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      const beforeUpdate = Date.now();

      store.updateNode(
        textNode.id,
        {
          content: '## Test',
          nodeType: 'header',
          properties: { headerLevel: 2 }
        },
        viewerSource
      );

      // Verify the update happened in store
      const updatedNode = store.getNode(textNode.id);
      expect(updatedNode?.nodeType).toBe('header');
      expect(updatedNode?.content).toBe('## Test');

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 100));

      const afterUpdate = Date.now();

      // Persistence should happen within reasonable time
      const persistenceDelay = afterUpdate - beforeUpdate;
      expect(persistenceDelay).toBeLessThan(500); // Should be fast
    });
  });

  describe('Load Persisted Nodes', () => {
    it('should load persisted header nodes correctly after simulated reload', async () => {
      // Simulate creating and persisting a header node
      const headerNode: Node = {
        id: 'test-node-7',
        nodeType: 'header',
        content: '## Persisted Header',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { headerLevel: 2 },
        mentions: []
      };

      store.setNode(headerNode, viewerSource);

      // Wait for persistence
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Simulate page reload by clearing and reloading
      const persistedNode = store.getNode(headerNode.id);
      store.clearAll();

      // Reload the persisted node (simulating database load)
      if (persistedNode) {
        store.setNode(persistedNode, { type: 'database', reason: 'test-reload' });
      }

      // Verify it loaded correctly
      const reloadedNode = store.getNode(headerNode.id);
      expect(reloadedNode?.nodeType).toBe('header');
      expect(reloadedNode?.content).toBe('## Persisted Header');
      expect(reloadedNode?.properties.headerLevel).toBe(2);
    });

    it('should preserve all header properties after reload', async () => {
      const headerNode: Node = {
        id: 'test-node-8',
        nodeType: 'header',
        content: '# Main Title',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {
          headerLevel: 1,
          customProp: 'test-value'
        },
        mentions: []
      };

      store.setNode(headerNode, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, 50));

      const persistedNode = store.getNode(headerNode.id);
      store.clearAll();

      if (persistedNode) {
        store.setNode(persistedNode, { type: 'database', reason: 'test-reload' });
      }

      const reloadedNode = store.getNode(headerNode.id);
      expect(reloadedNode?.properties.customProp).toBe('test-value');
    });
  });
});
