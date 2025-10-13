/**
 * Integration tests for view switching scenarios (date navigation, parent changes)
 * Tests the behavior of initializeNodes() when switching between different view parents
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import type { ReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import type { Node } from '$lib/types/node';

describe('View Switching - Date Navigation', () => {
  let nodeService: ReactiveNodeService;

  beforeEach(() => {
    // Clear shared store before each test
    sharedNodeStore.__resetForTesting();

    // Create service with mock events
    nodeService = createReactiveNodeService({
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    });
  });

  it('should not create duplicate placeholder nodes when navigating between dates', () => {
    // Simulate date navigation: Today → Yesterday → Today
    const today = '2025-10-13';
    const yesterday = '2025-10-12';

    // Helper to create placeholder node
    const createPlaceholder = (parentId: string): Node => ({
      id: globalThis.crypto.randomUUID(),
      nodeType: 'text',
      content: '',
      parentId,
      containerNodeId: parentId,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    });

    // Step 1: Load today with placeholder
    const todayPlaceholder1 = createPlaceholder(today);
    nodeService.setViewParentId(today);
    nodeService.initializeNodes([todayPlaceholder1], {
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: 0
    });

    // Verify placeholder exists for today
    let todayNodesFirst = sharedNodeStore.getNodesForParent(today);
    expect(todayNodesFirst).toHaveLength(1);
    expect(todayNodesFirst[0].content).toBe('');

    // Step 2: Navigate to yesterday with placeholder
    const yesterdayPlaceholder = createPlaceholder(yesterday);
    nodeService.setViewParentId(yesterday);
    nodeService.initializeNodes([yesterdayPlaceholder], {
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: 0
    });

    // Step 3: Navigate back to today with NEW placeholder
    const todayPlaceholder2 = createPlaceholder(today);
    nodeService.setViewParentId(today);
    nodeService.initializeNodes([todayPlaceholder2], {
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: 0
    });

    // CRITICAL: Should only have ONE placeholder for today (old one cleaned up)
    const todayNodesSecond = sharedNodeStore.getNodesForParent(today);
    expect(todayNodesSecond).toHaveLength(1);
    expect(todayNodesSecond[0].content).toBe('');

    // Verify no "multiple first children" error would occur
    const beforeSiblingNulls = todayNodesSecond.filter((n) => n.beforeSiblingId === null);
    expect(beforeSiblingNulls).toHaveLength(1);
  });

  it('should preserve persisted nodes when switching views', () => {
    const today = '2025-10-13';
    const yesterday = '2025-10-12';

    // Create a persisted node for today (with content)
    const persistedNode: Node = {
      id: 'persisted-1',
      nodeType: 'text',
      content: 'Important note',
      parentId: today,
      containerNodeId: today,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    // Step 1: Load today with persisted node
    nodeService.setViewParentId(today);
    nodeService.initializeNodes([persistedNode], {
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0
    });

    const todayNodesFirst = sharedNodeStore.getNodesForParent(today);
    expect(todayNodesFirst).toHaveLength(1);
    expect(todayNodesFirst[0].content).toBe('Important note');

    // Step 2: Navigate to yesterday
    nodeService.setViewParentId(yesterday);
    nodeService.initializeNodes([], { expanded: true, autoFocus: true, inheritHeaderLevel: 0 });

    // Step 3: Navigate back to today with same persisted node
    nodeService.setViewParentId(today);
    nodeService.initializeNodes([persistedNode], {
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0
    });

    // CRITICAL: Should still have the persisted node (not deleted)
    const todayNodesSecond = sharedNodeStore.getNodesForParent(today);
    expect(todayNodesSecond).toHaveLength(1);
    expect(todayNodesSecond[0].id).toBe('persisted-1');
    expect(todayNodesSecond[0].content).toBe('Important note');
  });

  it('should handle rapid view switching without creating duplicate nodes', () => {
    const dates = ['2025-10-13', '2025-10-12', '2025-10-11'];

    // Helper to create placeholder node
    const createPlaceholder = (parentId: string): Node => ({
      id: globalThis.crypto.randomUUID(),
      nodeType: 'text',
      content: '',
      parentId,
      containerNodeId: parentId,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    });

    // Rapidly switch between dates
    for (let i = 0; i < 3; i++) {
      for (const date of dates) {
        const placeholder = createPlaceholder(date);
        nodeService.setViewParentId(date);
        nodeService.initializeNodes([placeholder], {
          expanded: true,
          autoFocus: true,
          inheritHeaderLevel: 0
        });

        // Each date should only have one placeholder
        const nodes = sharedNodeStore.getNodesForParent(date);
        expect(nodes.length).toBeLessThanOrEqual(1);
      }
    }

    // Final verification: each date should have exactly one placeholder
    for (const date of dates) {
      const nodes = sharedNodeStore.getNodesForParent(date);
      expect(nodes).toHaveLength(1);
      expect(nodes[0].content).toBe('');
      expect(nodes[0].nodeType).toBe('text');
    }
  });

  it('should clean up only unpersisted placeholders, not persisted empty nodes', () => {
    const today = '2025-10-13';

    // Create an empty persisted node (user explicitly saved empty content)
    const persistedEmptyNode: Node = {
      id: 'empty-persisted',
      nodeType: 'text',
      content: '',
      parentId: today,
      containerNodeId: today,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: { saved: true } // Mark as explicitly saved
    };

    // Load with persisted empty node
    nodeService.setViewParentId(today);
    nodeService.initializeNodes([persistedEmptyNode], {
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0
    });

    // Switch away and back
    nodeService.setViewParentId('2025-10-12');
    nodeService.initializeNodes([], { expanded: true, autoFocus: true, inheritHeaderLevel: 0 });

    nodeService.setViewParentId(today);
    nodeService.initializeNodes([persistedEmptyNode], {
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0
    });

    // The persisted empty node should still exist
    const todayNodes = sharedNodeStore.getNodesForParent(today);
    expect(todayNodes).toHaveLength(1);
    expect(todayNodes[0].id).toBe('empty-persisted');
  });
});

describe('View Switching - Multiple Parents', () => {
  let nodeService: ReactiveNodeService;

  beforeEach(() => {
    sharedNodeStore.__resetForTesting();
    nodeService = createReactiveNodeService({
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    });
  });

  it('should handle switching between different node types (date, task, document)', () => {
    const parents = ['2025-10-13', 'task-container-1', 'doc-container-1'];

    // Helper to create placeholder node
    const createPlaceholder = (parentId: string): Node => ({
      id: globalThis.crypto.randomUUID(),
      nodeType: 'text',
      content: '',
      parentId,
      containerNodeId: parentId,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    });

    // Switch between different parent types
    for (const parent of parents) {
      const placeholder = createPlaceholder(parent);
      nodeService.setViewParentId(parent);
      nodeService.initializeNodes([placeholder], {
        expanded: true,
        autoFocus: true,
        inheritHeaderLevel: 0
      });

      const nodes = sharedNodeStore.getNodesForParent(parent);
      expect(nodes).toHaveLength(1);
      expect(nodes[0].content).toBe('');
    }

    // Verify isolation: each parent has its own placeholder
    for (const parent of parents) {
      const nodes = sharedNodeStore.getNodesForParent(parent);
      expect(nodes).toHaveLength(1);
      expect(nodes[0].parentId).toBe(parent);
    }
  });

  it('should maintain sibling chain integrity when switching views', () => {
    const parent = '2025-10-13';

    // Create multiple persisted nodes with proper sibling chain
    const nodes: Node[] = [
      {
        id: 'node-1',
        nodeType: 'text',
        content: 'First',
        parentId: parent,
        containerNodeId: parent,
        beforeSiblingId: null, // First in chain
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
      },
      {
        id: 'node-2',
        nodeType: 'text',
        content: 'Second',
        parentId: parent,
        containerNodeId: parent,
        beforeSiblingId: 'node-1', // After node-1
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
      }
    ];

    // Load nodes
    nodeService.setViewParentId(parent);
    nodeService.initializeNodes(nodes, { expanded: true, autoFocus: false, inheritHeaderLevel: 0 });

    // Switch away and back
    nodeService.setViewParentId('other-parent');
    nodeService.initializeNodes([], { expanded: true, autoFocus: true, inheritHeaderLevel: 0 });

    nodeService.setViewParentId(parent);
    nodeService.initializeNodes(nodes, { expanded: true, autoFocus: false, inheritHeaderLevel: 0 });

    // Verify sibling chain integrity maintained
    const loadedNodes = sharedNodeStore.getNodesForParent(parent);
    expect(loadedNodes).toHaveLength(2);

    const firstNodes = loadedNodes.filter((n) => n.beforeSiblingId === null);
    expect(firstNodes).toHaveLength(1);
    expect(firstNodes[0].id).toBe('node-1');
  });
});
