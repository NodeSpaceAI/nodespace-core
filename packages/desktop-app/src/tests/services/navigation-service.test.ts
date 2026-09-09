/**
 * Unit tests for NavigationService
 *
 * Tests navigation functionality including:
 * - Node target resolution (resolveNodeTarget)
 * - Tab title generation (generateTabTitle)
 * - Regular navigation (navigateToNode)
 * - New tab creation (navigateToNode with openInNewTab)
 * - Other pane navigation (navigateToNodeInOtherPane)
 * - Focus-or-open tab reuse (focusOrOpenNode, focusNodeTab)
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { getNavigationService, NavigationService } from '$lib/services/navigation-service';
import {
  navigationStore,
  resetTabState,
  createPane,
  addTab,
  setActivePane,
  DEFAULT_PANE_ID
} from '$lib/stores/navigation.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types';

// Full isolation from sibling test files: navigation-service resolves targets by
// reading BOTH the singleton sharedNodeStore (getNode) and the singleton
// structureTree (getParent). Neither is per-file, so nodes/parent-edges another
// file leaves behind can skew the ancestor walk under vitest's `forks` pool —
// the source of the intermittent "resolves to date ancestor" flakes. Clear both
// before every test so each case starts from an empty graph. Runs before the
// per-describe beforeEach hooks that set up each test's own fixtures.
beforeEach(() => {
  sharedNodeStore.clearAll();
  structureTree.clear();
});

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
    const mockGetNode = vi.fn().mockResolvedValue({
      id: 'backend-node',
      nodeType: 'text',
      content: 'Backend Content',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    });

    vi.doMock('$lib/services/backend-adapter', () => ({
      backendAdapter: { getNode: mockGetNode }
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

    vi.doUnmock('$lib/services/backend-adapter');
  });

  it('returns null when node not found in backend', async () => {
    const mockGetNode = vi.fn().mockResolvedValue(null);

    vi.doMock('$lib/services/backend-adapter', () => ({
      backendAdapter: { getNode: mockGetNode }
    }));

    const target = await navService.resolveNodeTarget('non-existent');

    expect(target).toBeNull();

    vi.doUnmock('$lib/services/backend-adapter');
  });

  it('returns null when backend fetch fails', async () => {
    const mockGetNode = vi.fn().mockRejectedValue(new Error('Backend error'));

    vi.doMock('$lib/services/backend-adapter', () => ({
      backendAdapter: { getNode: mockGetNode }
    }));

    const target = await navService.resolveNodeTarget('error-node');

    expect(target).toBeNull();

    vi.doUnmock('$lib/services/backend-adapter');
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

    // `task` renders inline, so the plugin-name fallback does not apply — it lands on
    // the `<type> Node` fallback rather than the registry label "Task Node".
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

    // `task` renders inline, so the plugin-name fallback does not apply — it lands on
    // the `<type> Node` fallback rather than the registry label "Task Node".
    expect(target?.title).toBe('task Node');
  });

  it('generates fallback title for an untitled text node (still the raw <type> Node form)', async () => {
    // Pins the original reason node-type-predicates' gate exists: `text`'s plugin name
    // ("Text Node") is a registry label, not a title — must not regress to showing it.
    const textNode: Node = {
      id: 'text-untitled',
      nodeType: 'text',
      content: '',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(textNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('text-untitled');

    expect(target?.title).toBe('text Node');
  });

  it('generates fallback title for an untitled header node (still the raw <type> Node form)', async () => {
    const headerNode: Node = {
      id: 'header-untitled',
      nodeType: 'header',
      content: '',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(headerNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('header-untitled');

    expect(target?.title).toBe('header Node');
  });

  it('generates "Untitled Person" for an untitled person node, not "person Node"', async () => {
    // The bug this fixes: person has an inline node component (rendersAsEntityRow is
    // false) but its plugin name "Person" is a real entity noun — the gate used to
    // exclude it from the plugin-name branch purely because it renders inline.
    const personNode: Node = {
      id: 'person-untitled',
      nodeType: 'person',
      content: '',
      title: '', // unresolved title_template ("{first_name} {last_name}" with both empty)
      version: 1,
      properties: { person: { first_name: '', last_name: '' } },
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(personNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('person-untitled');

    expect(target?.title).toBe('Untitled Person');
  });

  it('ignores stale content on an untitled title_template-driven person, rather than showing it', async () => {
    // person is title_template-driven: an unresolved title must not fall back to `content`
    // (e.g. leftover content from before a type conversion) — it must reach the
    // "Untitled Person" fallback instead, matching resolveTitleOrContent's rule.
    const personNode: Node = {
      id: 'person-stale-content',
      nodeType: 'person',
      content: 'leftover text from before conversion',
      title: ' ', // template resolved to whitespace only — still unresolved
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(personNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('person-stale-content');

    expect(target?.title).toBe('Untitled Person');
  });

  it('generates "Untitled AI Chat" for a brand-new ai-chat with no messages yet', async () => {
    // ai-chat is already an entity row (no inline component), so it already reached the
    // plugin-name branch before this fix — this pins that the "Untitled " prefix now
    // generically applies there too, not just to person.
    const chatNode: Node = {
      id: 'chat-untitled',
      nodeType: 'ai-chat',
      content: '',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    sharedNodeStore.setNode(chatNode, { type: 'database', reason: 'test-setup' }, true);

    const target = await navService.resolveNodeTarget('chat-untitled');

    expect(target?.title).toBe('Untitled AI Chat');
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
    const initialState = navigationStore.state;
    const activeTabId = initialState.activeTabIds[initialState.activePaneId];

    await navService.navigateToNode('nav-node-1', false);

    const state = navigationStore.state;
    const activeTab = state.tabs.find((t) => t.id === activeTabId);

    expect(activeTab?.content?.nodeId).toBe('nav-node-1');
    expect(activeTab?.content?.nodeType).toBe('text');
  });

  it('creates new tab when openInNewTab is true (Cmd+Click)', async () => {
    const initialState = navigationStore.state;
    const initialTabCount = initialState.tabs.length;

    await navService.navigateToNode('nav-node-1', true);

    const state = navigationStore.state;
    expect(state.tabs.length).toBe(initialTabCount + 1);

    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('text');
    expect(newTab?.closeable).toBe(true);
  });

  it('creates new tab in specified pane', async () => {
    // Create a second pane first
    const { createPane } = await import('$lib/stores/navigation.svelte');
    const newPane = createPane();
    expect(newPane).toBeDefined();

    const initialState = navigationStore.state;
    const initialTabCount = initialState.tabs.length;

    // Navigate with sourcePaneId specified
    await navService.navigateToNode('nav-node-1', true, newPane!.id);

    const state = navigationStore.state;
    expect(state.tabs.length).toBe(initialTabCount + 1);

    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
    expect(newTab?.paneId).toBe(newPane!.id);
  });

  it('creates new tab in active pane when sourcePaneId not provided', async () => {
    const initialState = navigationStore.state;
    const activePaneId = initialState.activePaneId;

    await navService.navigateToNode('nav-node-1', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');

    expect(newTab?.paneId).toBe(activePaneId);
  });

  it('makes new tab active by default', async () => {
    await navService.navigateToNode('nav-node-1', true);

    const state = navigationStore.state;
    const activePaneId = state.activePaneId;
    const activeTabId = state.activeTabIds[activePaneId];

    const activeTab = state.tabs.find((t) => t.id === activeTabId);
    expect(activeTab?.content?.nodeId).toBe('nav-node-1');
  });

  it('does not make new tab active when makeTabActive is false', async () => {
    const initialState = navigationStore.state;
    const initialActiveTabId = initialState.activeTabIds[initialState.activePaneId];

    await navService.navigateToNode('nav-node-1', true, undefined, false);

    const state = navigationStore.state;
    const currentActiveTabId = state.activeTabIds[state.activePaneId];

    // Active tab should not have changed
    expect(currentActiveTabId).toBe(initialActiveTabId);

    // But new tab should exist
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'nav-node-1');
    expect(newTab).toBeDefined();
  });

  it('handles non-existent node gracefully', async () => {
    const initialState = navigationStore.state;
    const initialTabCount = initialState.tabs.length;

    await navService.navigateToNode('non-existent-node', true);

    const state = navigationStore.state;
    // No new tab should be created
    expect(state.tabs.length).toBe(initialTabCount);
  });

  it('sets correct tab title from node content', async () => {
    await navService.navigateToNode('nav-node-1', true);

    const state = navigationStore.state;
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
      const initialState = navigationStore.state;
      expect(initialState.panes.length).toBe(1);
      expect(initialState.panes[0].id).toBe(DEFAULT_PANE_ID);

      // Navigate to node in other pane
      await navService.navigateToNodeInOtherPane('test-node-1');

      // Should now have 2 panes
      const state = navigationStore.state;
      expect(state.panes.length).toBe(2);
      expect(state.panes[0].width).toBe(50); // First pane resized
      expect(state.panes[1].width).toBe(50); // Second pane created
    });

    it('opens tab in new pane', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = navigationStore.state;
      const newPane = state.panes[1];

      // Check that a tab was added to the new pane
      const tabsInNewPane = state.tabs.filter((t) => t.paneId === newPane.id);
      expect(tabsInNewPane.length).toBe(1);
      expect(tabsInNewPane[0]?.content?.nodeId).toBe('test-node-1');
      expect(tabsInNewPane[0]?.content?.nodeType).toBe('text');
    });

    it('sets new pane as active', async () => {
      const initialState = navigationStore.state;
      expect(initialState.activePaneId).toBe(DEFAULT_PANE_ID);

      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = navigationStore.state;
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

      const state = navigationStore.state;
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
      const state = navigationStore.state;
      const firstPaneId = state.panes[0].id;

      // Manually import and use setActivePane
      const { setActivePane } = await import('$lib/stores/navigation.svelte');
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
      const beforeState = navigationStore.state;
      expect(beforeState.panes.length).toBe(2);

      const firstPaneId = beforeState.panes[0].id;
      const secondPaneId = beforeState.panes[1].id;

      // Active pane should be first pane
      expect(beforeState.activePaneId).toBe(firstPaneId);

      // Navigate to second node in other pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = navigationStore.state;

      // Should still have 2 panes (not 3)
      expect(afterState.panes.length).toBe(2);

      // New tab should be in the second pane (the "other" pane)
      const tabsInSecondPane = afterState.tabs.filter((t) => t.paneId === secondPaneId);
      const newTab = tabsInSecondPane.find((t) => t.content?.nodeId === 'test-node-2');

      expect(newTab).toBeDefined();
      expect(newTab?.content?.nodeType).toBe('text');
    });

    it('switches focus to other pane', async () => {
      const beforeState = navigationStore.state;
      const firstPaneId = beforeState.panes[0].id;
      const secondPaneId = beforeState.panes[1].id;

      // Active pane is first pane
      expect(beforeState.activePaneId).toBe(firstPaneId);

      // Navigate to node in other pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = navigationStore.state;

      // Active pane should now be second pane
      expect(afterState.activePaneId).toBe(secondPaneId);
    });

    it('prevents creating more than 2 panes', async () => {
      // Already have 2 panes from beforeEach
      const beforeState = navigationStore.state;
      expect(beforeState.panes.length).toBe(2);

      // Try to navigate again - should NOT create a third pane
      await navService.navigateToNodeInOtherPane('test-node-2');

      const afterState = navigationStore.state;
      expect(afterState.panes.length).toBe(2);
    });
  });

  describe('Error handling', () => {
    it('handles non-existent node gracefully', async () => {
      const beforeState = navigationStore.state;
      const initialPaneCount = beforeState.panes.length;

      // Try to navigate to non-existent node
      await navService.navigateToNodeInOtherPane('non-existent-node-uuid');

      const afterState = navigationStore.state;

      // Should not create new pane or tabs (navigation fails gracefully)
      expect(afterState.panes.length).toBe(initialPaneCount);
    });

    it('handles invalid UUID format gracefully', async () => {
      const beforeState = navigationStore.state;
      const initialPaneCount = beforeState.panes.length;

      // Try to navigate to invalid UUID (should fail in resolveNodeTarget)
      await navService.navigateToNodeInOtherPane('invalid-uuid');

      const afterState = navigationStore.state;

      // Should not create new pane or tabs
      expect(afterState.panes.length).toBe(initialPaneCount);
    });

    it('respects explicit sourcePaneId parameter', async () => {
      // This tests that when an explicit sourcePaneId is provided,
      // the service correctly identifies the "other" pane
      // First, create second pane
      await navService.navigateToNodeInOtherPane('test-node-1');

      const beforeState = navigationStore.state;
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

      const afterState = navigationStore.state;

      // Tab should be created in the second pane (the "other" one from first)
      const newTab = afterState.tabs.find((t) => t.content?.nodeId === 'test-node-3');
      expect(newTab).toBeDefined();
      expect(newTab?.paneId).toBe(secondPaneId);
    });
  });

  describe('Tab properties', () => {
    it('creates closeable tabs', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = navigationStore.state;
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.closeable).toBe(true);
    });

    it('generates correct tab titles', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = navigationStore.state;
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.title).toBe('Test Node Content');
    });

    it('sets correct tab type', async () => {
      await navService.navigateToNodeInOtherPane('test-node-1');

      const state = navigationStore.state;
      const newTab = state.tabs.find((t) => t.content?.nodeId === 'test-node-1');

      expect(newTab?.type).toBe('node');
    });
  });
});

describe('NavigationService - Entity node navigation', () => {
  let navService: ReturnType<typeof getNavigationService>;

  function makeNode(id: string, nodeType: string, content: string = ''): Node {
    return {
      id,
      nodeType,
      content,
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
  }

  beforeEach(() => {
    resetTabState();
    structureTree.clear();
    navService = getNavigationService();
  });

  it('task node nested under date node opens as task (not parent date)', async () => {
    // Setup: date node -> task node (task has its own viewer)
    const dateNode = makeNode('2025-06-15', 'date', '2025-06-15');
    const taskNode = makeNode('task-under-date', 'task', 'Buy groceries');

    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(taskNode, { type: 'database', reason: 'test' }, true);

    // Set up parent-child relationship: date -> task
    structureTree.addChild({ parentId: '2025-06-15', childId: 'task-under-date', order: 1 });

    // Navigate to the task node in a new tab
    await navService.navigateToNode('task-under-date', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'task-under-date');

    // Should open the task node itself, NOT the parent date node
    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('task');
    expect(newTab?.content?.nodeId).toBe('task-under-date');
  });

  it('task node nested under date opens correctly in other pane', async () => {
    const dateNode = makeNode('2025-06-15', 'date', '2025-06-15');
    const taskNode = makeNode('task-other-pane', 'task', 'Write report');

    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(taskNode, { type: 'database', reason: 'test' }, true);

    structureTree.addChild({ parentId: '2025-06-15', childId: 'task-other-pane', order: 1 });

    await navService.navigateToNodeInOtherPane('task-other-pane');

    const state = navigationStore.state;
    const newPane = state.panes[1];
    const tabsInNewPane = state.tabs.filter((t) => t.paneId === newPane.id);

    expect(tabsInNewPane.length).toBe(1);
    expect(tabsInNewPane[0]?.content?.nodeId).toBe('task-other-pane');
    expect(tabsInNewPane[0]?.content?.nodeType).toBe('task');
  });

  it('project node under date opens as project (not parent date)', async () => {
    // `project` is a core type with no registered viewer or node component — it renders as
    // a read-only entity row, so it must open directly rather than resolving to its parent.
    const dateNode = makeNode('2025-06-15', 'date', '2025-06-15');
    const projectNode = makeNode('project-under-date', 'project', 'Website redesign');

    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(projectNode, { type: 'database', reason: 'test' }, true);

    structureTree.addChild({ parentId: '2025-06-15', childId: 'project-under-date', order: 1 });

    await navService.navigateToNode('project-under-date', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === 'project-under-date');

    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('project');
  });

  it('primitive text node under date still resolves to date ancestor', async () => {
    const dateNode = makeNode('2025-06-15', 'date', '2025-06-15');
    const textNode = makeNode('text-child', 'text', 'Some note text');

    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(textNode, { type: 'database', reason: 'test' }, true);

    structureTree.addChild({ parentId: '2025-06-15', childId: 'text-child', order: 1 });

    await navService.navigateToNode('text-child', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === '2025-06-15');

    // Primitive text node should resolve to parent date node
    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('date');
  });

  it('date node nested under another node still opens as date', async () => {
    // date nodes have their own viewer, so they should not walk up
    const parentText = makeNode('parent-text', 'text', 'Parent');
    const dateNode = makeNode('2025-07-01', 'date', '2025-07-01');

    sharedNodeStore.setNode(parentText, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);

    structureTree.addChild({ parentId: 'parent-text', childId: '2025-07-01', order: 1 });

    await navService.navigateToNode('2025-07-01', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === '2025-07-01');

    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('date');
  });

  it('deeply nested primitive walks up to nearest viewer-owning ancestor', async () => {
    // Structure: date -> text -> header (both text and header are primitives)
    const dateNode = makeNode('2025-06-15', 'date', '2025-06-15');
    const textNode = makeNode('mid-text', 'text', 'Middle text');
    const headerNode = makeNode('deep-header', 'header', '# Title');

    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(textNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(headerNode, { type: 'database', reason: 'test' }, true);

    structureTree.addChild({ parentId: '2025-06-15', childId: 'mid-text', order: 1 });
    structureTree.addChild({ parentId: 'mid-text', childId: 'deep-header', order: 1 });

    await navService.navigateToNode('deep-header', true);

    const state = navigationStore.state;
    const newTab = state.tabs.find((t) => t.content?.nodeId === '2025-06-15');

    // Should walk all the way up to the date node (nearest viewer-owning ancestor)
    expect(newTab).toBeDefined();
    expect(newTab?.content?.nodeType).toBe('date');
  });
});

describe('NavigationService - focusOrOpenNode', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    resetTabState();
    navService = getNavigationService();
  });

  function nodeTabs(nodeId: string) {
    return navigationStore.state.tabs.filter((tab) => tab.content?.nodeId === nodeId);
  }

  it('opens a tab when the node is not already open', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    const tabs = nodeTabs('node-a');
    expect(tabs).toHaveLength(1);
    expect(tabs[0].content?.nodeType).toBe('text');
    expect(tabs[0].type).toBe('node');
    expect(tabs[0].closeable).toBe(true);
  });

  it('makes the newly opened tab active', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    const state = navigationStore.state;
    const tab = nodeTabs('node-a')[0];
    expect(state.activeTabIds[tab.paneId]).toBe(tab.id);
  });

  it('defaults the title to a placeholder the viewer replaces on mount', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    expect(nodeTabs('node-a')[0].title).toBe('Loading...');
  });

  it('uses an explicit title when the caller already knows it', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text', title: 'Quarterly plan' });

    expect(nodeTabs('node-a')[0].title).toBe('Quarterly plan');
  });

  it('reuses the existing tab instead of opening a duplicate', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text' });
    const firstTabId = nodeTabs('node-a')[0].id;

    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    const tabs = nodeTabs('node-a');
    expect(tabs).toHaveLength(1);
    expect(tabs[0].id).toBe(firstTabId);
  });

  it('does not overwrite the existing tab title when reusing it', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text', title: 'Real title' });

    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    expect(nodeTabs('node-a')[0].title).toBe('Real title');
  });

  it('focuses an existing tab that lives in another pane, switching panes', () => {
    // Open the node in a second pane, then leave the first pane active.
    const secondPane = createPane();
    expect(secondPane).not.toBeNull();
    addTab({
      id: 'tab-in-other-pane',
      title: 'Elsewhere',
      type: 'node',
      content: { nodeId: 'node-a', nodeType: 'text' },
      closeable: true,
      paneId: secondPane!.id
    });
    // Put focus back on the first pane, so the match below has to switch panes.
    setActivePane(DEFAULT_PANE_ID);
    expect(navigationStore.state.activePaneId).toBe(DEFAULT_PANE_ID);

    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    const state = navigationStore.state;
    expect(nodeTabs('node-a')).toHaveLength(1);
    expect(state.activePaneId).toBe(secondPane!.id);
    expect(state.activeTabIds[secondPane!.id]).toBe('tab-in-other-pane');
  });

  it('opens into the active pane', () => {
    const secondPane = createPane();
    expect(secondPane).not.toBeNull();

    navService.focusOrOpenNode('node-a', { nodeType: 'text' });

    expect(nodeTabs('node-a')[0].paneId).toBe(navigationStore.state.activePaneId);
  });

  it('does not resolve to a navigation ancestor — it opens the node given', () => {
    // navigateToNode would walk a primitive up to its viewer-owning ancestor;
    // focusOrOpenNode deliberately does not.
    const dateNode: Node = {
      id: '2026-01-05',
      nodeType: 'date',
      content: '',
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };
    const child: Node = { ...dateNode, id: 'child-text', nodeType: 'text', content: 'Child' };
    sharedNodeStore.setNode(dateNode, { type: 'database', reason: 'test' }, true);
    sharedNodeStore.setNode(child, { type: 'database', reason: 'test' }, true);
    structureTree.addChild({ parentId: '2026-01-05', childId: 'child-text', order: 1 });

    navService.focusOrOpenNode('child-text', { nodeType: 'text' });

    expect(nodeTabs('child-text')).toHaveLength(1);
    expect(nodeTabs('2026-01-05')).toHaveLength(0);
  });

  describe('nodeType routing overrides', () => {
    it('opens a schema id under the tab nodeType that routes to its viewer', () => {
      // The sidebar opens schemas as 'query' so the tab reaches QueryNodeViewer.
      navService.focusOrOpenNode('schema-1', { nodeType: 'query' });

      expect(nodeTabs('schema-1')[0].content?.nodeType).toBe('query');
    });
  });

  describe('matchNodeType', () => {
    it('ignores nodeType by default, so any tab showing the id is reused', () => {
      navService.focusOrOpenNode('node-a', { nodeType: 'text' });

      navService.focusOrOpenNode('node-a', { nodeType: 'query' });

      expect(nodeTabs('node-a')).toHaveLength(1);
    });

    it('opens a new tab when matchNodeType is set and the open tab has another type', () => {
      // A date id is not a real node id: another tab may carry the same string
      // without being the daily journal.
      navService.focusOrOpenNode('2026-01-05', { nodeType: 'text' });

      navService.focusOrOpenNode('2026-01-05', { nodeType: 'date', matchNodeType: true });

      const tabs = nodeTabs('2026-01-05');
      expect(tabs).toHaveLength(2);
      expect(tabs.some((tab) => tab.content?.nodeType === 'date')).toBe(true);
    });

    it('reuses the tab when matchNodeType is set and the type matches', () => {
      navService.focusOrOpenNode('2026-01-05', { nodeType: 'date', matchNodeType: true });

      navService.focusOrOpenNode('2026-01-05', { nodeType: 'date', matchNodeType: true });

      expect(nodeTabs('2026-01-05')).toHaveLength(1);
    });
  });
});

describe('NavigationService - focusNodeTab', () => {
  let navService: ReturnType<typeof getNavigationService>;

  beforeEach(() => {
    resetTabState();
    navService = getNavigationService();
  });

  it('returns false and opens nothing when the node is not open', () => {
    const before = navigationStore.state.tabs.length;

    expect(navService.focusNodeTab('node-a')).toBe(false);
    expect(navigationStore.state.tabs).toHaveLength(before);
  });

  it('returns true and focuses the tab when the node is open', () => {
    navService.focusOrOpenNode('node-a', { nodeType: 'text' });
    const tab = navigationStore.state.tabs.find((t) => t.content?.nodeId === 'node-a');
    navService.focusOrOpenNode('node-b', { nodeType: 'text' });

    expect(navService.focusNodeTab('node-a')).toBe(true);
    expect(navigationStore.state.activeTabIds[tab!.paneId]).toBe(tab!.id);
  });

  it('matches on node id alone, regardless of the tab nodeType', () => {
    navService.focusOrOpenNode('schema-1', { nodeType: 'query' });

    expect(navService.focusNodeTab('schema-1')).toBe(true);
  });
});
