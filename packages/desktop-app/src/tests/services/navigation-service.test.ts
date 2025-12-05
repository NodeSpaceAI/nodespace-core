/**
 * Unit tests for NavigationService
 *
 * Tests navigation functionality including:
 * - Node target resolution (resolveNodeTarget)
 * - Tab title generation (generateTabTitle)
 * - Regular navigation (navigateToNode)
 * - New tab creation (navigateToNode with openInNewTab)
 * - Other pane navigation (navigateToNodeInOtherPane)
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { getNavigationService, NavigationService } from '$lib/services/navigation-service';
import { tabState, resetTabState, DEFAULT_PANE_ID } from '$lib/stores/navigation';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types';

describe('NavigationService - Singleton Pattern', () => {
  it('getInstance returns the same instance', () => {
    const instance1 = NavigationService.getInstance();
    const instance2 = NavigationService.getInstance();

    expect(instance1).toBe(instance2);
  });

  it('getNavigationService returns the singleton instance', () => {
    const instance1 = getNavigationService();
    const instance2 = getNavigationService();
    const instance3 = NavigationService.getInstance();

    expect(instance1).toBe(instance2);
    expect(instance1).toBe(instance3);
  });
});

describe('NavigationService - resolveNodeTarget', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    resetTabState();
    navService = getNavigationService();
  });

  it('resolves node from store (synchronous path)', async () => {
    const testNode: Node = {
      id: 'test-node-1',
      nodeType: 'text',
      content: 'Test Content',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('test-node-1');

    expect(target).toEqual({
      nodeId: 'test-node-1',
      nodeType: 'text',
      title: 'Test Content'
    });
  });

  it('fetches node from backend when not in store', async () => {
    // Mock the tauri-commands module
    const mockGetNode = vi.fn().mockResolvedValue({
      id: 'backend-node',
      nodeType: 'text',
      content: 'Backend Content',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    });

    vi.doMock('$lib/services/tauri-commands', () => ({
      getNode: mockGetNode
    }));

    const target = await navService.resolveNodeTarget('backend-node');

    expect(target).toEqual({
      nodeId: 'backend-node',
      nodeType: 'text',
      title: 'Backend Content'
    });

    // Verify node was added to store
    const nodeInStore = sharedNodeStore.getNode('backend-node');
    expect(nodeInStore).toBeDefined();
    expect(nodeInStore?.content).toBe('Backend Content');

    vi.doUnmock('$lib/services/tauri-commands');
  });

  it('returns null when node not found in backend', async () => {
    const mockGetNode = vi.fn().mockResolvedValue(null);

    vi.doMock('$lib/services/tauri-commands', () => ({
      getNode: mockGetNode
    }));

    const target = await navService.resolveNodeTarget('non-existent');

    expect(target).toBeNull();

    vi.doUnmock('$lib/services/tauri-commands');
  });

  it('returns null when backend fetch fails', async () => {
    const mockGetNode = vi.fn().mockRejectedValue(new Error('Backend error'));

    vi.doMock('$lib/services/tauri-commands', () => ({
      getNode: mockGetNode
    }));

    const target = await navService.resolveNodeTarget('error-node');

    expect(target).toBeNull();

    vi.doUnmock('$lib/services/tauri-commands');
  });

  it('handles date nodes correctly', async () => {
    const dateNode: Node = {
      id: '2025-12-25',
      nodeType: 'date',
      content: '2025-12-25',
      version: 1,
      properties: { date: '2025-12-25' },
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('2025-12-25');

    expect(target).toBeDefined();
    expect(target?.nodeId).toBe('2025-12-25');
    expect(target?.nodeType).toBe('date');
    expect(target?.title).toBeTruthy();
    expect(target?.title.length).toBeGreaterThan(0);
  });
});

describe('NavigationService - generateTabTitle (via resolveNodeTarget)', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    resetTabState();
    navService = getNavigationService();
  });

  it('generates title from text node content', async () => {
    const textNode: Node = {
      id: 'text-1',
      nodeType: 'text',
      content: 'This is a long piece of content that should be truncated if too long',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(textNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('text-1');

    expect(target?.title).toBeTruthy();
    expect(target?.title.length).toBeGreaterThan(0);
  });

  it('generates title from date node properties', async () => {
    const dateNode: Node = {
      id: 'date-1',
      nodeType: 'date',
      content: '2025-01-15',
      version: 1,
      properties: { date: '2025-01-15' },
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('date-1');

    expect(target?.title).toBeTruthy();
    // Should be formatted date (e.g., "January 15, 2025")
    expect(target?.title).not.toBe('date-1');
  });

  it('generates title for date node with numeric timestamp', async () => {
    const dateNode: Node = {
      id: 'date-2',
      nodeType: 'date',
      content: '2025-01-15',
      version: 1,
      properties: { date: Date.now() },
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('date-2');

    expect(target?.title).toBeTruthy();
    expect(target?.title.length).toBeGreaterThan(0);
  });

  it('generates title for date node without date property', async () => {
    const dateNode: Node = {
      id: 'date-3',
      nodeType: 'date',
      content: '2025-01-15',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('date-3');

    expect(target?.title).toBeTruthy();
    // Should use current timestamp fallback
    expect(target?.title.length).toBeGreaterThan(0);
  });

  it('generates fallback title for node without content', async () => {
    const taskNode: Node = {
      id: 'task-1',
      nodeType: 'task',
      content: '',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(taskNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('task-1');

    expect(target?.title).toBe('task Node');
  });

  it('generates fallback title for node with non-string content', async () => {
    const taskNode: Node = {
      id: 'task-2',
      nodeType: 'task',
      content: null as unknown as string,
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(taskNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('task-2');

    expect(target?.title).toBe('task Node');
  });
});

describe('NavigationService - navigateToNode', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    resetTabState();
    navService = getNavigationService();

    // Setup test node
    const testNode: Node = {
      id: 'nav-node-1',
      nodeType: 'text',
      content: 'Navigation Test Node',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test-setup' }, true);
  });

  it('navigates to node in current tab (regular click)', async () => {
    const initialState = get(tabState);
    const activeTabId = initialState.activeTabIds[initialState.activePaneId];

    await navService.navigateToNode('nav-node-1', false);

    const state = get(tabState);
    const activeTab = state.tabs.find((t) => t.id === activeTabId);

    expect(activeTab?.content?.nodeId).toBe('nav-node-1');
    expect(activeTab?.content?.nodeType).toBe('text');
  });

  it('creates new tab when openInNewTab is true (Cmd+Click)', async () => {
    const initialState = get(tabState);
    const initialTabCount = initialState.tabs.length;

    await navService.navigateToNode('nav-node-1', true);

    const state = get(tabState);
    expect(state.tabs.length).toBe(initialTabCount + 1);

    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('text');
    expect(newTab?.closeable).toBe(true);
  });

  it('creates new tab in specified pane', async () => {
    // Create a second pane first
    const { createPane } = await import('$lib/stores/navigation');
    const newPane = createPane();
    expect(newPane).toBeDefined();

    const initialState = get(tabState);
    const initialTabCount = initialState.tabs.length;

    // Navigate with sourcePaneId specified
    await navService.navigateToNode('nav-node-1', true, newPane!.id);

    const state = get(tabState);
    expect(state.tabs.length).toBe(initialTabCount + 1);

    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
    expect(newTab?.paneId).toBe(newPane!.id);
  });

  it('creates new tab in active pane when sourcePaneId not provided', async () => {
    const initialState = get(tabState);
    const activePaneId = initialState.activePaneId;

    await navService.navigateToNode('nav-node-1', true);

    const state = get(tabState);
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');

    expect(newTab?.paneId).toBe(activePaneId);
  });

  it('makes new tab active by default', async () => {
    await navService.navigateToNode('nav-node-1', true);

    const state = get(tabState);
    const activePaneId = state.activePaneId;
    const activeTabId = state.activeTabIds[activePaneId];

    const activeTab = state.tabs.find((t) => t.id === activeTabId);
    expect(activeTab?.content?.nodeId).toBe('nav-node-1');
  });

  it('does not make new tab active when makeTabActive is false', async () => {
    const initialState = get(tabState);
    const initialActiveTabId = initialState.activeTabIds[initialState.activePaneId];

    await navService.navigateToNode('nav-node-1', true, undefined, false);

    const state = get(tabState);
    const currentActiveTabId = state.activeTabIds[state.activePaneId];

    // Active tab should not have changed
    expect(currentActiveTabId).toBe(initialActiveTabId);

    // But new tab should exist
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
  });

  it('handles non-existent node gracefully', async () => {
    const initialState = get(tabState);
    const initialTabCount = initialState.tabs.length;

    await navService.navigateToNode('non-existent-node', true);

    const state = get(tabState);
    // No new tab should be created
    expect(state.tabs.length).toBe(initialTabCount);
  });

  it('sets correct tab title from node content', async () => {
    await navService.navigateToNode('nav-node-1', true);

    const state = get(tabState);
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');

    expect(newTab?.title).toBe('Navigation Test Node');
  });
});

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
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test-setup' }, true);
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
      // Mock a date node in the store (backend would return this for YYYY-MM-DD IDs)
      const dateNode: Node = {
        id: '2025-12-25',
        nodeType: 'date',
        content: '2025-12-25', // Date nodes have content matching ID
        version: 1,
          properties: {},
        createdAt: Date.now().toString(),
        modifiedAt: Date.now().toString()
      };
      sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test-setup' }, true);

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
          properties: {},
        createdAt: Date.now().toString(),
        modifiedAt: Date.now().toString()
      };
      sharedNodeStore.setNode(testNode2, { type: 'database', reason: 'test-setup' }, true);
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
      const beforeState = get(tabState);
      const initialPaneCount = beforeState.panes.length;

      // Try to navigate to non-existent node
      await navService.navigateToNodeInOtherPane('non-existent-node-uuid');

      const afterState = get(tabState);

      // Should not create new pane or tabs (navigation fails gracefully)
      expect(afterState.panes.length).toBe(initialPaneCount);
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

    it('respects explicit sourcePaneId parameter', async () => {
      // This tests that when an explicit sourcePaneId is provided,
      // the service correctly identifies the "other" pane
      // First, create second pane
      await navService.navigateToNodeInOtherPane('test-node-1');

      const beforeState = get(tabState);
      expect(beforeState.panes.length).toBe(2);

      const firstPaneId = beforeState.panes[0].id;
      const secondPaneId = beforeState.panes[1].id;

      // Add another node for testing
      const testNode2: Node = {
        id: 'test-node-3',
        nodeType: 'text',
        content: 'Test Node 3',
        version: 1,
        properties: {},
        createdAt: Date.now().toString(),
        modifiedAt: Date.now().toString()
      };
      sharedNodeStore.setNode(testNode2, { type: 'database', reason: 'test-setup' }, true);

      // Navigate from first pane (explicit source)
      await navService.navigateToNodeInOtherPane('test-node-3', firstPaneId);

      const afterState = get(tabState);

      // Tab should be created in the second pane (the "other" one from first)
      const newTab = afterState.tabs.find((t) => t.content?.nodeId === 'test-node-3');
      expect(newTab).toBeDefined();
      expect(newTab?.paneId).toBe(secondPaneId);
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
