import { describe, it, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import type { Node } from '$lib/types/node';

describe('Split-Pane Content Isolation', () => {
  beforeEach(() => {
    // Reset shared node store to clear state from previous tests
    sharedNodeStore.__resetForTesting();

    // Clear any test errors from previous tests
    sharedNodeStore.clearTestErrors();
  });

  it('should isolate children between two viewers with different parents', () => {
    // Setup: Create two parents with different children
    const parentA: Node = {
      id: 'parent-a',
      content: 'Parent A',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    const parentB: Node = {
      id: 'parent-b',
      content: 'Parent B',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    const childA1: Node = {
      id: 'child-a1',
      content: 'Child of A',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    const childB1: Node = {
      id: 'child-b1',
      content: 'Child of B',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    sharedNodeStore.setNode(parentA, { type: 'database', reason: 'test-setup' });
    sharedNodeStore.setNode(parentB, { type: 'database', reason: 'test-setup' });
    sharedNodeStore.setNode(childA1, { type: 'database', reason: 'test-setup' });
    sharedNodeStore.setNode(childB1, { type: 'database', reason: 'test-setup' });

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    // Create service (simulating nodeManager)
    const mockEvents = { emit: () => {}, on: () => () => {}, hierarchyChanged: () => {} };
    const service = createReactiveNodeService(mockEvents as never);

    // Execute: Get visible nodes for each parent (simulating two panes)
    const nodesForA = service.visibleNodes('parent-a');
    const nodesForB = service.visibleNodes('parent-b');

    // Assert: Each pane shows only its parent's children
    expect(nodesForA.length).toBe(1);
    expect(nodesForB.length).toBe(1);
    expect(nodesForA[0].id).toBe('child-a1');
    expect(nodesForB[0].id).toBe('child-b1');
    expect(nodesForA[0].id).not.toBe(nodesForB[0].id);
  });

  it('should allow two viewers to query same parent without conflict', () => {
    // Setup
    const parent: Node = {
      id: 'shared-parent',
      content: 'Shared Parent',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    const child1: Node = {
      id: 'child-1',
      content: 'Child 1',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
    sharedNodeStore.setNode(child1, { type: 'database', reason: 'test-setup' });

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    const mockEvents = { emit: () => {}, on: () => () => {}, hierarchyChanged: () => {} };
    const service = createReactiveNodeService(mockEvents as never);

    // Execute: Two viewers query same parent
    const viewer1Nodes = service.visibleNodes('shared-parent');
    const viewer2Nodes = service.visibleNodes('shared-parent');

    // Assert: Both get same data (correct behavior)
    expect(viewer1Nodes.length).toBe(1);
    expect(viewer2Nodes.length).toBe(1);
    expect(viewer1Nodes[0].id).toBe('child-1');
    expect(viewer2Nodes[0].id).toBe('child-1');
  });

  it('should handle null parentId (root nodes)', () => {
    // Setup
    const rootNode: Node = {
      id: 'root-1',
      content: 'Root Node',
      nodeType: 'text',
      version: 1,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {}
    };

    sharedNodeStore.setNode(rootNode, { type: 'database', reason: 'test-setup' });

    const mockEvents = { emit: () => {}, on: () => () => {}, hierarchyChanged: () => {} };
    const service = createReactiveNodeService(mockEvents as never);

    // Initialize service with root node (simulates loadChildrenForParent)
    service.initializeNodes([rootNode]);

    // Execute
    const rootNodes = service.visibleNodes(null);

    // Assert
    expect(rootNodes.length).toBeGreaterThanOrEqual(1);
    expect(rootNodes.some((n) => n.id === 'root-1')).toBe(true);
  });

  it('should return empty array for non-existent parent (e.g., virtual date node with no children yet)', () => {
    const mockEvents = { emit: () => {}, on: () => () => {}, hierarchyChanged: () => {} };
    const service = createReactiveNodeService(mockEvents as never);

    // Execute: Query non-existent parent (e.g., virtual date node that hasn't been created yet)
    // This is valid behavior - virtual date nodes don't exist until first child is created
    const nodes = service.visibleNodes('2025-11-02');

    // Assert: Returns empty array (no children yet, parent may not exist)
    expect(nodes).toEqual([]);
  });
});
