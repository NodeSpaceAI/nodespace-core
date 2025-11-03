/**
 * Unit tests for NavigationService - navigateToNodeInOtherPane method
 *
 * Tests the Cmd+Shift+Click behavior for opening nodes in the other pane
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { getNavigationService } from '$lib/services/navigation-service';
import { tabState, resetTabState, DEFAULT_PANE_ID } from '$lib/stores/navigation';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import type { Node } from '$lib/types';

describe('NavigationService - navigateToNodeInOtherPane', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    // Reset store to initial state
    resetTabState();

    // Get navigation service instance
    navService = getNavigationService();

    // Mock a test node in the store
    const testNode: Node = {
      id: 'test-node-1',
      nodeType: 'text',
      content: 'Test Node Content',
      version: 1,
      parentId: null,
      containerNodeId: null,
      beforeSiblingId: null,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(
      testNode,
      { type: 'external', source: 'test', description: 'test node' },
      true
    );
  });

  describe('Single pane behavior', () => {
    it('creates second pane when only 1 exists', async () => {
      // Initially we have 1 pane
      const initialState = get(tabState);
      expect(initialState.panes.length).toBe(1);
      expect(initialState.panes[0].id).toBe(DEFAULT_PANE_ID);

      // Navigate to node in other pane
      await navService.navigateToNodeInOtherPane('test-node-1');

      // Should now have 2 panes
      const state = get(tabState);
      expect(state.panes.length).toBe(2);
      expect(state.panes[0].width).toBe(50); // First pane resized
      expect(state.panes[1].width).toBe(50); // Second pane created
    });

    it('opens tab in new pane', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = get(tabState);
      const newPane = state.panes[1];

      // Check that a tab was added to the new pane
      const tabsInNewPane = state.tabs.filter((t) => t.paneId === newPane.id);
      expect(tabsInNewPane.length).toBe(1);
      expect(tabsInNewPane[0]?.content?.nodeId).toBe('test-node-1');
      expect(tabsInNewPane[0]?.content?.nodeType).toBe('text');
    });

    it('sets new pane as active', async () => {
      const initialState = get(tabState);
      expect(initialState.activePaneId).toBe(DEFAULT_PANE_ID);

      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = get(tabState);
      expect(state.activePaneId).not.toBe(DEFAULT_PANE_ID);
      expect(state.activePaneId).toBe(state.panes[1].id);
    });

    it('handles date nodes correctly', async () => {
      await navService.navigateToNodeInOtherPane('2025-12-25');

      const state = get(tabState);
      const newPane = state.panes[1];
      const tabsInNewPane = state.tabs.filter((t) => t.paneId === newPane.id);

      expect(tabsInNewPane.length).toBe(1);
      expect(tabsInNewPane[0]?.content?.nodeId).toBe('2025-12-25');
      expect(tabsInNewPane[0]?.content?.nodeType).toBe('date');
      // Title is formatted date string (e.g., "December 25, 2025" or similar)
      expect(tabsInNewPane[0]?.title).toBeTruthy();
      expect(tabsInNewPane[0]?.title?.length).toBeGreaterThan(0);
    });
  });

  describe('Two pane behavior', () => {
    beforeEach(async () => {
      // Create second pane by navigating first time
      await navService.navigateToNodeInOtherPane('test-node-1');

      // Switch back to first pane
      const state = get(tabState);
      const firstPaneId = state.panes[0].id;

      // Manually import and use setActivePane
      const { setActivePane } = await import('$lib/stores/navigation');
      setActivePane(firstPaneId);

      // Mock another test node
      const testNode2: Node = {
        id: 'test-node-2',
        nodeType: 'text',
        content: 'Test Node 2 Content',
        version: 1,
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        properties: {},
        createdAt: Date.now().toString(),
        modifiedAt: Date.now().toString()
      };
      sharedNodeStore.setNode(
        testNode2,
        { type: 'external', source: 'test', description: 'test node 2' },
        true
      );
    });

    it('opens in other pane when 2 panes exist', async () => {
      const beforeState = get(tabState);
      expect(beforeState.panes.length).toBe(2);

      const firstPaneId = beforeState.panes[0].id;
      const secondPaneId = beforeState.panes[1].id;

      // Active pane should be first pane
      expect(beforeState.activePaneId).toBe(firstPaneId);

      // Navigate to second node in other pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = get(tabState);

      // Should still have 2 panes (not 3)
      expect(afterState.panes.length).toBe(2);

      // New tab should be in the second pane (the "other" pane)
      const tabsInSecondPane = afterState.tabs.filter((t) => t.paneId === secondPaneId);
      const newTab = tabsInSecondPane.find((t) => t.content?.nodeId === 'test-node-2');

      expect(newTab).toBeDefined();
      expect(newTab?.content?.nodeType).toBe('text');
    });

    it('switches focus to other pane', async () => {
      const beforeState = get(tabState);
      const firstPaneId = beforeState.panes[0].id;
      const secondPaneId = beforeState.panes[1].id;

      // Active pane is first pane
      expect(beforeState.activePaneId).toBe(firstPaneId);

      // Navigate to node in other pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = get(tabState);

      // Active pane should now be second pane
      expect(afterState.activePaneId).toBe(secondPaneId);
    });

    it('prevents creating more than 2 panes', async () => {
      // Already have 2 panes from beforeEach
      const beforeState = get(tabState);
      expect(beforeState.panes.length).toBe(2);

      // Try to navigate again - should NOT create a third pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = get(tabState);
      expect(afterState.panes.length).toBe(2);
    });
  });

  describe('Error handling', () => {
    it('handles non-existent node gracefully', async () => {
      // Spy on console.error
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

      const beforeState = get(tabState);
      const initialPaneCount = beforeState.panes.length;

      // Try to navigate to non-existent node
      await navService.navigateToNodeInOtherPane('non-existent-node-uuid');

      const afterState = get(tabState);

      // Should not create new pane or tabs
      expect(afterState.panes.length).toBe(initialPaneCount);

      // Should have logged an error
      expect(consoleErrorSpy).toHaveBeenCalled();

      consoleErrorSpy.mockRestore();
    });

    it('handles invalid UUID format gracefully', async () => {
      const beforeState = get(tabState);
      const initialPaneCount = beforeState.panes.length;

      // Try to navigate to invalid UUID (should fail in resolveNodeTarget)
      await navService.navigateToNodeInOtherPane('invalid-uuid');

      const afterState = get(tabState);

      // Should not create new pane or tabs
      expect(afterState.panes.length).toBe(initialPaneCount);
    });
  });

  describe('Tab properties', () => {
    it('creates closeable tabs', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = get(tabState);
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.closeable).toBe(true);
    });

    it('generates correct tab titles', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = get(tabState);
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.title).toBe('Test Node Content');
    });

    it('sets correct tab type', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = get(tabState);
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.type).toBe('node');
    });
  });
});
