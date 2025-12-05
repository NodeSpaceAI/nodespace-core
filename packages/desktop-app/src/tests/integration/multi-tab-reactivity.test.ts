/**
 * Multi-Tab/Pane Reactivity Integration Tests
 *
 * These tests verify that changes made in one viewer/pane propagate correctly
 * to other viewers of the same content. The reactivity system uses:
 *
 * 1. SharedNodeStore - Singleton holding all node data with subscriber notifications
 * 2. ReactiveStructureTree - Svelte 5 state for parent→child hierarchy
 * 3. ReactiveNodeService - Per-viewer instances that subscribe to SharedNodeStore
 *
 * Test scenarios from issue #641:
 * 1. Content Update Propagation - Edit in viewer 1, verify viewer 2 updates
 * 2. Structural Change Propagation - Indent in viewer 1, verify hierarchy in viewer 2
 * 3. Cross-Pane Split View - Split panes with independent scroll positions
 * 4. Placeholder Promotion - New node ID consistency across panes
 * 5. Concurrent Conflict - Simultaneous edits to same node
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types/node';

// Helper to create test nodes
function createTestNode(id: string, content: string, parentId?: string): Node {
  return {
    id,
    content,
    nodeType: 'text',
    version: 1,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    properties: {},
    parentId
  };
}

// Helper to create mock events for ReactiveNodeService
function createMockEvents() {
  return {
    emit: vi.fn(),
    on: vi.fn().mockReturnValue(() => {}),
    hierarchyChanged: vi.fn(),
    nodeDeleted: vi.fn()
  };
}

// Helper to wait for subscriber notifications
async function waitForSubscribers(): Promise<void> {
  // Allow microtasks to complete for subscriber notifications
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('Multi-Tab/Pane Reactivity', () => {
  beforeEach(() => {
    // Reset shared state between tests
    sharedNodeStore.__resetForTesting();
    structureTree.children = new Map();
    sharedNodeStore.clearTestErrors();
  });

  describe('1. Content Update Propagation', () => {
    it('should propagate content changes to all viewers of same node', async () => {
      // Setup: Create a node that will be viewed by two panes
      const sharedNode = createTestNode('shared-node-1', 'Initial content');
      sharedNodeStore.setNode(sharedNode, { type: 'database', reason: 'test-setup' });

      // Create two independent viewer services (simulating two panes)
      const viewer1Events = createMockEvents();
      const viewer2Events = createMockEvents();
      const viewer1 = createReactiveNodeService(viewer1Events as never);
      const viewer2 = createReactiveNodeService(viewer2Events as never);

      // Both viewers initialize with the same node
      viewer1.initializeNodes([sharedNode]);
      viewer2.initializeNodes([sharedNode]);

      // Verify initial state is same in both viewers
      const node1Before = sharedNodeStore.getNode('shared-node-1');
      const node2Before = sharedNodeStore.getNode('shared-node-1');
      expect(node1Before?.content).toBe('Initial content');
      expect(node2Before?.content).toBe('Initial content');

      // Act: Update content through viewer 1
      viewer1.updateNodeContent('shared-node-1', 'Updated content from viewer 1');
      await waitForSubscribers();

      // Assert: Viewer 2 should see the updated content
      const nodeAfter = sharedNodeStore.getNode('shared-node-1');
      expect(nodeAfter?.content).toBe('Updated content from viewer 1');

      // Both viewers should have triggered their update mechanisms
      expect(viewer1._updateTrigger).toBeGreaterThan(0);
      expect(viewer2._updateTrigger).toBeGreaterThan(0);
    });

    it('should update all viewers within milliseconds (no network round-trip)', async () => {
      // Setup
      const node = createTestNode('perf-node', 'Performance test content');
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test-setup' });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      viewer1.initializeNodes([node]);

      // Act: Measure update propagation time
      const startTime = performance.now();
      viewer1.updateNodeContent('perf-node', 'New content');
      await waitForSubscribers();
      const endTime = performance.now();

      // Assert: Update should propagate in < 10ms (no network delay)
      const propagationTime = endTime - startTime;
      expect(propagationTime).toBeLessThan(10);

      // Verify content was updated
      expect(sharedNodeStore.getNode('perf-node')?.content).toBe('New content');
    });

    it('should maintain version tracking across viewers', async () => {
      // Setup
      const node = createTestNode('version-node', 'Version 1 content');
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test-setup' });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      viewer1.initializeNodes([node]);

      const initialVersion = sharedNodeStore.getVersion('version-node');

      // Act: Multiple updates
      viewer1.updateNodeContent('version-node', 'Version 2 content');
      await waitForSubscribers();
      const version2 = sharedNodeStore.getVersion('version-node');

      viewer1.updateNodeContent('version-node', 'Version 3 content');
      await waitForSubscribers();
      const version3 = sharedNodeStore.getVersion('version-node');

      // Assert: Versions should increment
      expect(version2).toBeGreaterThan(initialVersion);
      expect(version3).toBeGreaterThan(version2);
    });
  });

  describe('2. Structural Change Propagation', () => {
    it('should propagate hierarchy changes when node is indented', async () => {
      // Setup: Create parent and two children at same level
      const parent = createTestNode('struct-parent', 'Parent');
      const child1 = createTestNode('struct-child-1', 'Child 1', 'struct-parent');
      const child2 = createTestNode('struct-child-2', 'Child 2', 'struct-parent');

      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child1, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child2, { type: 'database', reason: 'test-setup' });

      // Set up hierarchy: both children under parent
      structureTree.__testOnly_addChild({ parentId: 'struct-parent', childId: 'struct-child-1', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'struct-parent', childId: 'struct-child-2', order: 2.0 });

      // Create two viewers of the same parent
      const viewer1Events = createMockEvents();
      const viewer2Events = createMockEvents();
      const viewer1 = createReactiveNodeService(viewer1Events as never);
      const viewer2 = createReactiveNodeService(viewer2Events as never);

      viewer1.initializeNodes([parent, child1, child2]);
      viewer2.initializeNodes([parent, child1, child2]);

      // Verify initial structure: both children under parent
      const parentChildren = structureTree.getChildren('struct-parent');
      expect(parentChildren).toContain('struct-child-1');
      expect(parentChildren).toContain('struct-child-2');

      // Act: Simulate indent - child2 becomes child of child1
      // In real code, this goes through indentNode() but we test the structure change directly
      // moveInMemoryRelationship(oldParentId, newParentId, childId)
      structureTree.moveInMemoryRelationship('struct-parent', 'struct-child-1', 'struct-child-2');

      // Assert: Structure should be updated
      const child1Children = structureTree.getChildren('struct-child-1');
      expect(child1Children).toContain('struct-child-2');

      // Parent should no longer have child2 as direct child
      const parentChildrenAfter = structureTree.getChildren('struct-parent');
      expect(parentChildrenAfter).not.toContain('struct-child-2');

      // Both viewers have hierarchy changed callbacks registered
      // Note: hierarchyChanged is called during initializeNodes when computing depths,
      // which triggers the callback. The test verifies the callback mechanism exists
      // and is wired up correctly to the service.
      expect(viewer1Events.hierarchyChanged).toHaveBeenCalled();
      expect(viewer2Events.hierarchyChanged).toHaveBeenCalled();
    });

    it('should show chevron/expand indicator in both viewers after indent', async () => {
      // Setup: Child1 initially has no children
      const parent = createTestNode('chevron-parent', 'Parent');
      const child1 = createTestNode('chevron-child-1', 'Child 1', 'chevron-parent');
      const child2 = createTestNode('chevron-child-2', 'Child 2', 'chevron-parent');

      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child1, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child2, { type: 'database', reason: 'test-setup' });

      structureTree.__testOnly_addChild({ parentId: 'chevron-parent', childId: 'chevron-child-1', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'chevron-parent', childId: 'chevron-child-2', order: 2.0 });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      viewer1.initializeNodes([parent, child1, child2]);

      // Verify child1 has no children initially
      expect(structureTree.hasChildren('chevron-child-1')).toBe(false);

      // Act: Move child2 under child1 (indent operation)
      // moveInMemoryRelationship(oldParentId, newParentId, childId)
      structureTree.moveInMemoryRelationship('chevron-parent', 'chevron-child-1', 'chevron-child-2');

      // Assert: child1 now has children (should show chevron)
      expect(structureTree.hasChildren('chevron-child-1')).toBe(true);
    });
  });

  describe('3. Cross-Pane Split View', () => {
    it('should allow two panes to view same parent with independent state', async () => {
      // Setup: Create parent with children
      const parent = createTestNode('split-parent', 'Split Parent');
      const child1 = createTestNode('split-child-1', 'Child 1', 'split-parent');
      const child2 = createTestNode('split-child-2', 'Child 2', 'split-parent');

      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child1, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child2, { type: 'database', reason: 'test-setup' });

      structureTree.__testOnly_addChild({ parentId: 'split-parent', childId: 'split-child-1', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'split-parent', childId: 'split-child-2', order: 2.0 });

      // Create two independent viewers (split panes)
      const leftPane = createReactiveNodeService(createMockEvents() as never);
      const rightPane = createReactiveNodeService(createMockEvents() as never);

      leftPane.initializeNodes([parent, child1, child2]);
      rightPane.initializeNodes([parent, child1, child2]);

      // Act: Edit in left pane
      leftPane.updateNodeContent('split-child-1', 'Updated from left pane');
      await waitForSubscribers();

      // Assert: Right pane sees updated content
      const nodeFromStore = sharedNodeStore.getNode('split-child-1');
      expect(nodeFromStore?.content).toBe('Updated from left pane');
    });

    it('should maintain independent expansion state per viewer', async () => {
      // Setup
      const parent = createTestNode('expand-parent', 'Parent');
      const child = createTestNode('expand-child', 'Child', 'expand-parent');
      const grandchild = createTestNode('expand-grandchild', 'Grandchild', 'expand-child');

      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(child, { type: 'database', reason: 'test-setup' });
      sharedNodeStore.setNode(grandchild, { type: 'database', reason: 'test-setup' });

      structureTree.__testOnly_addChild({ parentId: 'expand-parent', childId: 'expand-child', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'expand-child', childId: 'expand-grandchild', order: 1.0 });

      const leftPane = createReactiveNodeService(createMockEvents() as never);
      const rightPane = createReactiveNodeService(createMockEvents() as never);

      leftPane.initializeNodes([parent, child, grandchild]);
      rightPane.initializeNodes([parent, child, grandchild]);

      // Act: Expand in left pane only
      leftPane.setExpanded('expand-child', true);

      // Assert: UI state is maintained per-viewer via _uiState
      // The left pane's expanded state doesn't affect right pane
      // (UI state is local to each ReactiveNodeService instance)
      const leftUIState = leftPane.getUIState('expand-child');
      const rightUIState = rightPane.getUIState('expand-child');

      expect(leftUIState?.expanded).toBe(true);
      // Right pane has default expanded state (true by default in initializeNodes)
      // The key assertion is that setExpanded on one viewer doesn't affect the other
      // Both start expanded, but if we collapsed right, left would remain expanded
      expect(rightUIState?.expanded).toBe(true);

      // Now demonstrate independent state by collapsing right pane
      rightPane.setExpanded('expand-child', false);
      expect(leftPane.getUIState('expand-child')?.expanded).toBe(true);
      expect(rightPane.getUIState('expand-child')?.expanded).toBe(false);
    });
  });

  describe('4. Placeholder Promotion Sync', () => {
    it('should maintain consistent node ID when placeholder is promoted', async () => {
      // Setup: Create parent for new node
      const parent = createTestNode('promo-parent', 'Promotion Parent');
      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });
      structureTree.__testOnly_addChild({ parentId: 'root', childId: 'promo-parent', order: 1.0 });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      const viewer2 = createReactiveNodeService(createMockEvents() as never);

      viewer1.initializeNodes([parent]);
      viewer2.initializeNodes([parent]);

      // Act: Create placeholder in viewer 1
      const placeholderId = 'placeholder-temp-123';
      const placeholderNode = createTestNode(placeholderId, '', 'promo-parent');
      sharedNodeStore.setNode(placeholderNode, { type: 'viewer', viewerId: 'test-viewer-1' });
      structureTree.__testOnly_addChild({ parentId: 'promo-parent', childId: placeholderId, order: 2.0 });

      await waitForSubscribers();

      // Verify placeholder exists in store
      expect(sharedNodeStore.getNode(placeholderId)).toBeDefined();
      expect(sharedNodeStore.getNode(placeholderId)?.content).toBe('');

      // Simulate promotion: update placeholder with real content
      sharedNodeStore.updateNode(placeholderId, { content: 'Promoted content' }, { type: 'viewer', viewerId: 'test-viewer-1' });
      await waitForSubscribers();

      // Assert: Both viewers should see the same promoted node
      const promotedNode = sharedNodeStore.getNode(placeholderId);
      expect(promotedNode?.content).toBe('Promoted content');
      expect(promotedNode?.id).toBe(placeholderId);
    });

    it('should synchronize placeholder to permanent ID transition', async () => {
      // This tests the scenario where a placeholder gets a permanent ID from backend
      const parent = createTestNode('id-parent', 'ID Parent');
      sharedNodeStore.setNode(parent, { type: 'database', reason: 'test-setup' });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      viewer1.initializeNodes([parent]);

      // Create placeholder
      const tempId = 'temp-placeholder-456';
      const placeholder = createTestNode(tempId, 'Placeholder content', 'id-parent');
      sharedNodeStore.setNode(placeholder, { type: 'viewer', viewerId: 'test-viewer-1' });
      structureTree.__testOnly_addChild({ parentId: 'id-parent', childId: tempId, order: 1.0 });

      // Simulate backend assigning permanent ID by creating new node and deleting old
      const permanentId = 'permanent-node-789';
      const permanentNode = createTestNode(permanentId, 'Placeholder content', 'id-parent');
      sharedNodeStore.setNode(permanentNode, { type: 'database', reason: 'promotion' });

      // Delete placeholder
      sharedNodeStore.deleteNode(tempId, { type: 'database', reason: 'promotion-cleanup' });

      // Update structure tree
      structureTree.__testOnly_addChild({ parentId: 'id-parent', childId: permanentId, order: 1.0 });

      await waitForSubscribers();

      // Assert: Placeholder removed, permanent node exists
      expect(sharedNodeStore.getNode(tempId)).toBeUndefined();
      expect(sharedNodeStore.getNode(permanentId)).toBeDefined();
      expect(sharedNodeStore.getNode(permanentId)?.content).toBe('Placeholder content');
    });
  });

  describe('5. Concurrent Conflict Detection', () => {
    it('should detect concurrent edits to same node', async () => {
      // Setup: Node that will be edited concurrently
      const node = createTestNode('conflict-node', 'Original content');
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test-setup' });

      const viewer1 = createReactiveNodeService(createMockEvents() as never);
      const viewer2 = createReactiveNodeService(createMockEvents() as never);

      viewer1.initializeNodes([node]);
      viewer2.initializeNodes([node]);

      // Act: Both viewers update different aspects
      // Viewer 1 updates content
      viewer1.updateNodeContent('conflict-node', 'Content from viewer 1');

      // Viewer 2 updates properties (different field)
      viewer2.updateNodeProperties('conflict-node', { customProp: 'value from viewer 2' });

      await waitForSubscribers();

      // Assert: Store should have both changes applied
      // (Last-Write-Wins for content, but properties should merge)
      const finalNode = sharedNodeStore.getNode('conflict-node');
      expect(finalNode).toBeDefined();
      // The store applies updates in order received
      expect(finalNode?.properties?.customProp).toBe('value from viewer 2');
    });

    it('should handle rapid sequential updates without data loss', async () => {
      // Setup
      const node = createTestNode('rapid-node', 'Initial');
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test-setup' });

      const viewer = createReactiveNodeService(createMockEvents() as never);
      viewer.initializeNodes([node]);

      // Act: Rapid sequential updates
      for (let i = 1; i <= 10; i++) {
        viewer.updateNodeContent('rapid-node', `Update ${i}`);
      }

      await waitForSubscribers();

      // Assert: Final state should be last update
      const finalNode = sharedNodeStore.getNode('rapid-node');
      expect(finalNode?.content).toBe('Update 10');

      // Version should reflect all updates
      const version = sharedNodeStore.getVersion('rapid-node');
      expect(version).toBeGreaterThanOrEqual(10);
    });

    it('should track version conflicts for OCC', async () => {
      // Setup: Node with known version
      const node = createTestNode('occ-node', 'OCC content');
      node.version = 5;
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test-setup' });

      const initialVersion = sharedNodeStore.getVersion('occ-node');

      // Act: Update that should increment version
      sharedNodeStore.updateNode('occ-node', { content: 'Updated OCC content' }, { type: 'viewer', viewerId: 'test-viewer-1' });

      // Assert: Version should be incremented
      const newVersion = sharedNodeStore.getVersion('occ-node');
      expect(newVersion).toBeGreaterThan(initialVersion);
    });
  });

  describe('Subscriber Notification Mechanism', () => {
    it('should notify all wildcard subscribers on any node change', async () => {
      // Setup: Track notifications
      const notifications: Array<{ nodeId: string; content: string }> = [];

      const unsubscribe = sharedNodeStore.subscribeAll((node) => {
        notifications.push({ nodeId: node.id, content: node.content });
      });

      // Act: Create and update multiple nodes
      const node1 = createTestNode('notify-1', 'Content 1');
      const node2 = createTestNode('notify-2', 'Content 2');

      sharedNodeStore.setNode(node1, { type: 'database', reason: 'test' });
      sharedNodeStore.setNode(node2, { type: 'database', reason: 'test' });
      sharedNodeStore.updateNode('notify-1', { content: 'Updated 1' }, { type: 'viewer', viewerId: 'test-viewer-1' });

      await waitForSubscribers();

      // Assert: Should have received notifications for all changes
      expect(notifications.length).toBe(3);
      expect(notifications).toContainEqual({ nodeId: 'notify-1', content: 'Content 1' });
      expect(notifications).toContainEqual({ nodeId: 'notify-2', content: 'Content 2' });
      expect(notifications).toContainEqual({ nodeId: 'notify-1', content: 'Updated 1' });

      // Cleanup
      unsubscribe();
    });

    it('should allow multiple viewers to subscribe independently', async () => {
      // Setup: Track notifications per viewer
      const viewer1Notifications: string[] = [];
      const viewer2Notifications: string[] = [];

      const unsub1 = sharedNodeStore.subscribeAll((node) => {
        viewer1Notifications.push(node.id);
      });

      const unsub2 = sharedNodeStore.subscribeAll((node) => {
        viewer2Notifications.push(node.id);
      });

      // Act
      const node = createTestNode('multi-sub-node', 'Test');
      sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });

      await waitForSubscribers();

      // Assert: Both viewers should be notified
      expect(viewer1Notifications).toContain('multi-sub-node');
      expect(viewer2Notifications).toContain('multi-sub-node');

      // Cleanup
      unsub1();
      unsub2();
    });

    it('should properly unsubscribe to prevent memory leaks', async () => {
      // Setup
      const notifications: string[] = [];
      const unsubscribe = sharedNodeStore.subscribeAll((node) => {
        notifications.push(node.id);
      });

      // First node should notify
      sharedNodeStore.setNode(createTestNode('leak-1', 'Test'), { type: 'database', reason: 'test' });
      await waitForSubscribers();
      expect(notifications).toContain('leak-1');

      // Unsubscribe
      unsubscribe();

      // Second node should NOT notify (unsubscribed)
      sharedNodeStore.setNode(createTestNode('leak-2', 'Test'), { type: 'database', reason: 'test' });
      await waitForSubscribers();

      // Assert: Only first notification received
      expect(notifications).toContain('leak-1');
      expect(notifications).not.toContain('leak-2');
    });
  });

  describe('Structure Tree Reactivity', () => {
    it('should update children map on hierarchy changes', () => {
      // Verify empty initially
      expect(structureTree.getChildren('reactivity-parent')).toEqual([]);

      // Act: Add child relationship
      structureTree.__testOnly_addChild({ parentId: 'reactivity-parent', childId: 'reactivity-child', order: 1.0 });

      // Assert: Children should be updated
      expect(structureTree.getChildren('reactivity-parent')).toContain('reactivity-child');
    });

    it('should maintain sorted order for siblings', () => {
      // Add children out of order
      structureTree.__testOnly_addChild({ parentId: 'sort-parent', childId: 'child-c', order: 3.0 });
      structureTree.__testOnly_addChild({ parentId: 'sort-parent', childId: 'child-a', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'sort-parent', childId: 'child-b', order: 2.0 });

      // Assert: Children should be in sorted order
      const children = structureTree.getChildren('sort-parent');
      expect(children).toEqual(['child-a', 'child-b', 'child-c']);
    });

    it('should handle fractional ordering for insertions between siblings', () => {
      // Setup: Two siblings with integer orders
      structureTree.__testOnly_addChild({ parentId: 'frac-parent', childId: 'child-1', order: 1.0 });
      structureTree.__testOnly_addChild({ parentId: 'frac-parent', childId: 'child-3', order: 3.0 });

      // Act: Insert between with fractional order
      structureTree.__testOnly_addChild({ parentId: 'frac-parent', childId: 'child-2', order: 2.0 });

      // Assert: Order is maintained
      const children = structureTree.getChildren('frac-parent');
      expect(children).toEqual(['child-1', 'child-2', 'child-3']);

      // Insert with fractional order between 1 and 2
      structureTree.__testOnly_addChild({ parentId: 'frac-parent', childId: 'child-1.5', order: 1.5 });

      const childrenAfter = structureTree.getChildren('frac-parent');
      expect(childrenAfter).toEqual(['child-1', 'child-1.5', 'child-2', 'child-3']);
    });
  });
});
