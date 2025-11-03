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
  reorderTab,
  moveTabBetweenPanes,
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

  describe('reorderTab', () => {
    beforeEach(() => {
      resetTabState();
    });

    it('reorders tab within same pane', () => {
      // Add three tabs to default pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const tab3: Tab = {
        id: 'test-tab-3',
        title: 'Test Tab 3',
        type: 'node',
        content: { nodeId: 'test-node-3', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(tab1);
      addTab(tab2);
      addTab(tab3);

      // Verify initial order: Daily Journal, tab1, tab2, tab3
      const stateBefore = get(tabState);
      const pane = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(pane?.tabIds).toEqual([DAILY_JOURNAL_TAB_ID, tab1.id, tab2.id, tab3.id]);

      // Move tab1 from index 1 to index 3 (after tab3)
      reorderTab(tab1.id, 3, DEFAULT_PANE_ID);

      const stateAfter = get(tabState);
      const updatedPane = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(updatedPane?.tabIds).toEqual([DAILY_JOURNAL_TAB_ID, tab2.id, tab3.id, tab1.id]);
    });

    it('handles invalid pane ID gracefully', () => {
      const stateBefore = get(tabState);
      const pane = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const tabIdsBefor = pane?.tabIds;

      reorderTab(DAILY_JOURNAL_TAB_ID, 0, 'non-existent-pane');

      const stateAfter = get(tabState);
      const updatedPane = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(updatedPane?.tabIds).toEqual(tabIdsBefor);
    });

    it('handles invalid tab ID gracefully', () => {
      const stateBefore = get(tabState);
      const pane = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const tabIdsBefore = pane?.tabIds;

      reorderTab('non-existent-tab', 0, DEFAULT_PANE_ID);

      const stateAfter = get(tabState);
      const updatedPane = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(updatedPane?.tabIds).toEqual(tabIdsBefore);
    });

    it('no-ops when moving to same position', () => {
      // Add a tab
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);

      const stateBefore = get(tabState);
      const pane = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const tabIdsBefore = [...pane!.tabIds];

      // Move tab1 to its current position (index 1)
      reorderTab(tab1.id, 1, DEFAULT_PANE_ID);

      const stateAfter = get(tabState);
      const updatedPane = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(updatedPane?.tabIds).toEqual(tabIdsBefore);
    });

    it('handles boundary indices correctly (move to start)', () => {
      // Add three tabs
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);
      addTab(tab2);

      // Move tab2 to index 0 (first position)
      reorderTab(tab2.id, 0, DEFAULT_PANE_ID);

      const state = get(tabState);
      const pane = state.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(pane?.tabIds).toEqual([tab2.id, DAILY_JOURNAL_TAB_ID, tab1.id]);
    });

    it('handles boundary indices correctly (move to end)', () => {
      // Add three tabs
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);
      addTab(tab2);

      // Move Daily Journal to last position
      reorderTab(DAILY_JOURNAL_TAB_ID, 2, DEFAULT_PANE_ID);

      const state = get(tabState);
      const pane = state.panes.find((p) => p.id === DEFAULT_PANE_ID);
      expect(pane?.tabIds).toEqual([tab1.id, tab2.id, DAILY_JOURNAL_TAB_ID]);
    });

    it('maintains tab reference integrity', () => {
      // Add a tab
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);

      const stateBefore = get(tabState);
      const tabBefore = stateBefore.tabs.find((t) => t.id === tab1.id);

      // Reorder tab
      reorderTab(tab1.id, 0, DEFAULT_PANE_ID);

      const stateAfter = get(tabState);
      const tabAfter = stateAfter.tabs.find((t) => t.id === tab1.id);

      // Tab object should remain the same (just order changed)
      expect(tabAfter).toEqual(tabBefore);
    });
  });

  describe('moveTabBetweenPanes', () => {
    beforeEach(() => {
      resetTabState();
    });

    it('moves tab between panes', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add a tab to first pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);

      const stateBefore = get(tabState);
      const pane1Before = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const pane2Before = stateBefore.panes.find((p) => p.id === newPane!.id);
      expect(pane1Before?.tabIds).toContain(tab1.id);
      expect(pane2Before?.tabIds).not.toContain(tab1.id);

      // Move tab from pane1 to pane2
      moveTabBetweenPanes(tab1.id, DEFAULT_PANE_ID, newPane!.id, 0);

      const stateAfter = get(tabState);
      const pane1After = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const pane2After = stateAfter.panes.find((p) => p.id === newPane!.id);

      expect(pane1After?.tabIds).not.toContain(tab1.id);
      expect(pane2After?.tabIds).toContain(tab1.id);
      expect(pane2After?.tabIds[0]).toBe(tab1.id);

      // Verify tab's paneId was updated
      const movedTab = stateAfter.tabs.find((t) => t.id === tab1.id);
      expect(movedTab?.paneId).toBe(newPane!.id);
    });

    it('closes source pane when last tab moved', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add a tab to second pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);

      const stateBefore = get(tabState);
      expect(stateBefore.panes.length).toBe(2);

      // Move the only tab from pane2 to pane1
      moveTabBetweenPanes(tab1.id, newPane!.id, DEFAULT_PANE_ID, 1);

      const stateAfter = get(tabState);
      expect(stateAfter.panes.length).toBe(1);
      expect(stateAfter.panes[0].id).toBe(DEFAULT_PANE_ID);
      expect(stateAfter.panes[0].width).toBe(100);
    });

    it('updates active tab correctly in both panes', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add two tabs to first pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);
      addTab(tab2);

      // Set tab2 as active in pane1
      setActiveTab(tab2.id, DEFAULT_PANE_ID);

      const stateBefore = get(tabState);
      expect(stateBefore.activeTabIds[DEFAULT_PANE_ID]).toBe(tab2.id);

      // Get the first remaining tab ID before moving
      const pane = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const remainingTabIds = pane!.tabIds.filter((id) => id !== tab2.id);
      const firstRemainingTabId = remainingTabIds[0];

      // Move active tab (tab2) from pane1 to pane2
      moveTabBetweenPanes(tab2.id, DEFAULT_PANE_ID, newPane!.id, 0);

      const stateAfter = get(tabState);
      // Source pane should have first remaining tab as active now
      expect(stateAfter.activeTabIds[DEFAULT_PANE_ID]).toBe(firstRemainingTabId);
      // Target pane should have moved tab as active
      expect(stateAfter.activeTabIds[newPane!.id]).toBe(tab2.id);
    });

    it('sets moved tab as active in target pane', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add a tab to pane2
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);

      // Add a tab to pane1
      const tab2: Tab = {
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab2);

      // Move tab2 from pane1 to pane2
      moveTabBetweenPanes(tab2.id, DEFAULT_PANE_ID, newPane!.id, 1);

      const state = get(tabState);
      expect(state.activePaneId).toBe(newPane!.id);
      expect(state.activeTabIds[newPane!.id]).toBe(tab2.id);
    });

    it('handles moving to non-existent pane', () => {
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab1);

      const stateBefore = get(tabState);
      const tabsBefore = stateBefore.tabs.length;
      const panesBefore = stateBefore.panes.length;

      // Try to move to non-existent pane
      moveTabBetweenPanes(tab1.id, DEFAULT_PANE_ID, 'non-existent-pane', 0);

      const stateAfter = get(tabState);
      expect(stateAfter.tabs.length).toBe(tabsBefore);
      expect(stateAfter.panes.length).toBe(panesBefore);

      // Tab should still be in original pane
      const tab = stateAfter.tabs.find((t) => t.id === tab1.id);
      expect(tab?.paneId).toBe(DEFAULT_PANE_ID);
    });

    it('handles moving non-existent tab', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      const stateBefore = get(tabState);
      const pane1Before = stateBefore.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const pane2Before = stateBefore.panes.find((p) => p.id === newPane!.id);
      const tabIds1Before = [...pane1Before!.tabIds];
      const tabIds2Before = [...pane2Before!.tabIds];

      // Try to move non-existent tab
      moveTabBetweenPanes('non-existent-tab', DEFAULT_PANE_ID, newPane!.id, 0);

      const stateAfter = get(tabState);
      const pane1After = stateAfter.panes.find((p) => p.id === DEFAULT_PANE_ID);
      const pane2After = stateAfter.panes.find((p) => p.id === newPane!.id);

      // Nothing should have changed
      expect(pane1After?.tabIds).toEqual(tabIds1Before);
      expect(pane2After?.tabIds).toEqual(tabIds2Before);
    });

    it('maintains pane width correctly after auto-close', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Resize panes to 60/40 split
      resizePane(DEFAULT_PANE_ID, 60);

      const stateBefore = get(tabState);
      expect(stateBefore.panes[0].width).toBe(60);
      expect(stateBefore.panes[1].width).toBe(40);

      // Add a tab to second pane
      const tab1: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);

      // Move the only tab from pane2 to pane1 (should close pane2)
      moveTabBetweenPanes(tab1.id, newPane!.id, DEFAULT_PANE_ID, 1);

      const stateAfter = get(tabState);
      expect(stateAfter.panes.length).toBe(1);
      // Remaining pane should be expanded to 100%
      expect(stateAfter.panes[0].width).toBe(100);
    });

    it('inserts tab at correct position in target pane', () => {
      // Create second pane
      const newPane = createPane();
      expect(newPane).not.toBeNull();

      // Add three tabs to pane2
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
      const tab3: Tab = {
        id: 'test-tab-3',
        title: 'Test Tab 3',
        type: 'node',
        content: { nodeId: 'test-node-3', nodeType: 'text' },
        closeable: true,
        paneId: newPane!.id
      };
      addTab(tab1);
      addTab(tab2);
      addTab(tab3);

      // Add a tab to pane1
      const tab4: Tab = {
        id: 'test-tab-4',
        title: 'Test Tab 4',
        type: 'node',
        content: { nodeId: 'test-node-4', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(tab4);

      // Move tab4 from pane1 to pane2 at index 1 (between tab1 and tab2)
      moveTabBetweenPanes(tab4.id, DEFAULT_PANE_ID, newPane!.id, 1);

      const state = get(tabState);
      const pane2 = state.panes.find((p) => p.id === newPane!.id);
      expect(pane2?.tabIds).toEqual([tab1.id, tab4.id, tab2.id, tab3.id]);
    });
  });
});
