/**
 * Unit tests for navigation store - pane and tab management
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  tabState,
  createPane,
  closePane,
  setActivePane,
  resizePane,
  setActiveTab,
  closeTab,
  addTab,
  resetTabState,
  DEFAULT_PANE_ID,
  DAILY_JOURNAL_TAB_ID,
  type Tab
} from '$lib/stores/navigation';

describe('Navigation Store - Pane Management', () => {
  beforeEach(() => {
    // Reset store to initial state
    resetTabState();
  });

  describe('createPane', () => {
    it('creates second pane at 50/50 split', () => {
      const newPane = createPane();

      expect(newPane).not.toBeNull();
      expect(newPane?.width).toBe(50);

      const state = get(tabState);
      expect(state.panes.length).toBe(2);
      expect(state.panes[0].width).toBe(50); // First pane resized to 50%
      expect(state.panes[1].width).toBe(50); // Second pane at 50%
    });

    it('prevents creating more than 2 panes', () => {
      // Create first pane (now we have 2)
      const firstPane = createPane();
      expect(firstPane).not.toBeNull();

      // Try to create third pane
      const thirdPane = createPane();
      expect(thirdPane).toBeNull();

      const state = get(tabState);
      expect(state.panes.length).toBe(2);
    });

    it('generates sequential pane IDs', () => {
      const newPane = createPane();
      expect(newPane?.id).toBe('pane-2');
    });
  });

  describe('closePane', () => {
    it('closes empty pane and expands remaining to 100%', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Close the second pane
      closePane(newPane!.id);

      const state = get(tabState);
      expect(state.panes.length).toBe(1);
      expect(state.panes[0].width).toBe(100);
    });

    it('prevents closing the last pane', () => {
      const state = get(tabState);
      const lastPaneId = state.panes[0].id;

      closePane(lastPaneId);

      const newState = get(tabState);
      expect(newState.panes.length).toBe(1);
      expect(newState.panes[0].id).toBe(lastPaneId);
    });

    it('removes all tabs belonging to closed pane', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add a tab to the second pane
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(newTab);

      const stateBefore = get(tabState);
      expect(stateBefore.tabs.length).toBe(2); // Daily Journal + new tab

      // Close the second pane
      closePane(newPane!.id);

      const stateAfter = get(tabState);
      expect(stateAfter.tabs.length).toBe(1);
      expect(stateAfter.tabs[0].id).toBe(DAILY_JOURNAL_TAB_ID);
    });

    it('updates active pane if closed pane was active', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Set second pane as active
      setActivePane(newPane!.id);

      const stateBefore = get(tabState);
      expect(stateBefore.activePaneId).toBe(newPane!.id);

      // Close the active pane
      closePane(newPane!.id);

      const stateAfter = get(tabState);
      expect(stateAfter.activePaneId).toBe(DEFAULT_PANE_ID);
    });
  });

  describe('setActivePane', () => {
    it('sets active pane when pane exists', () => {
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      setActivePane(newPane!.id);

      const state = get(tabState);
      expect(state.activePaneId).toBe(newPane!.id);
    });

    it('ignores invalid pane ID', () => {
      const stateBefore = get(tabState);
      const previousActivePaneId = stateBefore.activePaneId;

      setActivePane('non-existent-pane');

      const stateAfter = get(tabState);
      expect(stateAfter.activePaneId).toBe(previousActivePaneId);
    });
  });

  describe('resizePane', () => {
    it('resizes panes maintaining total 100%', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Resize first pane to 60%
      resizePane(DEFAULT_PANE_ID, 60);

      const state = get(tabState);
      expect(state.panes[0].width).toBe(60);
      expect(state.panes[1].width).toBe(40);
    });

    it('enforces minimum width of 20%', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Try to resize to less than minimum
      resizePane(DEFAULT_PANE_ID, 10);

      const state = get(tabState);
      expect(state.panes[0].width).toBe(20); // Clamped to minimum
      expect(state.panes[1].width).toBe(80);
    });

    it('enforces maximum width of 80%', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Try to resize to more than maximum
      resizePane(DEFAULT_PANE_ID, 90);

      const state = get(tabState);
      expect(state.panes[0].width).toBe(80); // Clamped to maximum
      expect(state.panes[1].width).toBe(20);
    });

    it('does nothing with single pane', () => {
      resizePane(DEFAULT_PANE_ID, 60);

      const state = get(tabState);
      expect(state.panes[0].width).toBe(100); // Unchanged
    });
  });
});

describe('Navigation Store - Tab Management', () => {
  beforeEach(() => {
    // Reset store to initial state
    resetTabState();
  });

  describe('setActiveTab', () => {
    it('sets active tab and activates its pane', () => {
      const tabId = DAILY_JOURNAL_TAB_ID;
      const paneId = DEFAULT_PANE_ID;

      setActiveTab(tabId);

      const newState = get(tabState);
      expect(newState.activePaneId).toBe(paneId);
      expect(newState.activeTabIds[paneId]).toBe(tabId);
    });

    it('handles paneId parameter override', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add tab to second pane
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(newTab);

      setActiveTab(newTab.id, newPane!.id);

      const state = get(tabState);
      expect(state.activePaneId).toBe(newPane!.id);
      expect(state.activeTabIds[newPane!.id]).toBe(newTab.id);
    });
  });

  describe('closeTab', () => {
    it('disables close on last tab in last pane', () => {
      const stateBefore = get(tabState);
      expect(stateBefore.tabs.length).toBe(1);

      closeTab(DAILY_JOURNAL_TAB_ID);

      const stateAfter = get(tabState);
      expect(stateAfter.tabs.length).toBe(1);
      expect(stateAfter.tabs[0].id).toBe(DAILY_JOURNAL_TAB_ID);
    });

    it('auto-closes pane when last tab closed', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add tab to second pane
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(newTab);

      const stateBefore = get(tabState);
      expect(stateBefore.panes.length).toBe(2);
      expect(stateBefore.tabs.length).toBe(2);

      // Close the only tab in second pane
      closeTab(newTab.id);

      const stateAfter = get(tabState);
      expect(stateAfter.panes.length).toBe(1);
      expect(stateAfter.tabs.length).toBe(1);
      expect(stateAfter.panes[0].width).toBe(100);
    });

    it('switches to first remaining tab when active tab closed', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add two tabs to second pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);
      addTab(tab2);

      // Set tab2 as active
      setActiveTab(tab2.id);

      const stateBefore = get(tabState);
      expect(stateBefore.activeTabIds[newPane!.id]).toBe(tab2.id);

      // Close active tab
      closeTab(tab2.id);

      const stateAfter = get(tabState);
      expect(stateAfter.activeTabIds[newPane!.id]).toBe(tab1.id);
    });

    it('updates pane tabIds list when tab closed', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add two tabs to second pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);
      addTab(tab2);

      const stateBefore = get(tabState);
      const pane = stateBefore.panes.find((p) => p.id === newPane!.id);
      expect(pane?.tabIds.length).toBe(2);

      // Close one tab
      closeTab(tab1.id);

      const stateAfter = get(tabState);
      const updatedPane = stateAfter.panes.find((p) => p.id === newPane!.id);
      expect(updatedPane?.tabIds.length).toBe(1);
      expect(updatedPane?.tabIds[0]).toBe(tab2.id);
    });
  });

  describe('addTab', () => {
    it('adds tab to specified pane', () => {
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab);

      const state = get(tabState);
      expect(state.tabs.length).toBe(2);
      expect(state.tabs[1].id).toBe(newTab.id);
    });

    it('updates pane tabIds list when tab added', () => {
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab);

      const state = get(tabState);
      const pane = state.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(pane?.tabIds.length).toBe(2);
      expect(pane?.tabIds[1]).toBe(newTab.id);
    });

    it('sets new tab as active', () => {
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab);

      const state = get(tabState);
      expect(state.activePaneId).toBe(DEFAULT_PANE_ID);
      expect(state.activeTabIds[DEFAULT_PANE_ID]).toBe(newTab.id);
    });

    it('rejects tab with non-existent paneId', () => {
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: 'non-existent-pane'
      };

      const stateBefore = get(tabState);
      const tabCountBefore = stateBefore.tabs.length;

      addTab(newTab);

      const stateAfter = get(tabState);
      expect(stateAfter.tabs.length).toBe(tabCountBefore);
    });
  });
});
