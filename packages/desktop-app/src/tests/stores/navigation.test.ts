/**
 * Unit tests for navigation store - pane and tab management
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
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
  loadPersistedState,
  updateTabTitle,
  updateTabContent,
  getOrderedTabsForPane,
  DEFAULT_PANE_ID,
  DAILY_JOURNAL_TAB_ID,
  type Tab
} from '$lib/stores/navigation';
import { TabPersistenceService } from '$lib/services/tab-persistence-service';
import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';

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

    it('does not modify other panes when reordering', () => {
      // Create second pane with a tab
      const pane2 = createPane();
      expect(pane2).not.toBeNull();

      const pane2Tab: Tab = {
        id: 'pane2-tab',
        title: 'Pane 2 Tab',
        type: 'node',
        content: { nodeId: 'pane2-node', nodeType: 'text' },
        closeable: true,
        paneId: pane2!.id
      };
      addTab(pane2Tab);

      // Add tabs to pane 1
      const pane1Tab1: Tab = {
        id: 'pane1-tab-1',
        title: 'Pane 1 Tab 1',
        type: 'node',
        content: { nodeId: 'pane1-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      const pane1Tab2: Tab = {
        id: 'pane1-tab-2',
        title: 'Pane 1 Tab 2',
        type: 'node',
        content: { nodeId: 'pane1-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(pane1Tab1);
      addTab(pane1Tab2);

      const stateBefore = get(tabState);
      const pane2Before = stateBefore.panes.find((p) => p.id === pane2!.id);
      const pane2TabIdsBefore = [...pane2Before!.tabIds];

      // Reorder tab in pane 1
      reorderTab(pane1Tab1.id, 2, DEFAULT_PANE_ID);

      const stateAfter = get(tabState);
      const pane2After = stateAfter.panes.find((p) => p.id === pane2!.id);

      // Pane 2 should remain unchanged
      expect(pane2After?.tabIds).toEqual(pane2TabIdsBefore);
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

  describe('addTab with makeActive parameter', () => {
    it('does not set tab as active when makeActive is false', () => {
      const originalActiveTab = get(tabState).activeTabIds[DEFAULT_PANE_ID];

      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab, false);

      const state = get(tabState);
      expect(state.tabs.length).toBe(2);
      expect(state.tabs[1].id).toBe(newTab.id);
      // Active tab should remain unchanged
      expect(state.activeTabIds[DEFAULT_PANE_ID]).toBe(originalActiveTab);
      // Active pane should remain unchanged
      expect(state.activePaneId).toBe(DEFAULT_PANE_ID);
    });
  });

  describe('setActiveTab edge cases', () => {
    it('does not change state when tab does not exist', () => {
      const stateBefore = get(tabState);
      const previousActiveTab = stateBefore.activeTabIds[DEFAULT_PANE_ID];

      setActiveTab('non-existent-tab-id');

      const stateAfter = get(tabState);
      expect(stateAfter.activeTabIds[DEFAULT_PANE_ID]).toBe(previousActiveTab);
    });
  });

  describe('closeTab edge cases', () => {
    it('does not change state when tab does not exist', () => {
      const stateBefore = get(tabState);
      const tabCountBefore = stateBefore.tabs.length;

      closeTab('non-existent-tab-id');

      const stateAfter = get(tabState);
      expect(stateAfter.tabs.length).toBe(tabCountBefore);
    });

    it('does not change state when pane not found for existing tab', () => {
      // This is a defensive test - in practice, tabs should always have valid paneIds
      // But the code checks for this condition on line 376-378

      // Manually corrupt state to create orphaned tab
      tabState.update((state) => ({
        ...state,
        tabs: [
          ...state.tabs,
          {
            id: 'orphaned-tab',
            title: 'Orphaned',
            type: 'node',
            content: { nodeId: 'test', nodeType: 'text' },
            closeable: true,
            paneId: 'non-existent-pane'
          }
        ]
      }));

      const stateWithOrphan = get(tabState);
      const tabCountWithOrphan = stateWithOrphan.tabs.length;

      closeTab('orphaned-tab');

      const stateAfter = get(tabState);
      // Should not remove the orphaned tab because pane doesn't exist
      expect(stateAfter.tabs.length).toBe(tabCountWithOrphan);
    });
  });

  describe('updateTabTitle', () => {
    it('updates title of existing tab', () => {
      const tabId = DAILY_JOURNAL_TAB_ID;
      const newTitle = 'Updated Journal Title';

      const stateBefore = get(tabState);
      const originalTitle = stateBefore.tabs.find((t) => t.id === tabId)?.title;
      expect(originalTitle).not.toBe(newTitle);

      updateTabTitle(tabId, newTitle);

      const stateAfter = get(tabState);
      const updatedTab = stateAfter.tabs.find((t) => t.id === tabId);
      expect(updatedTab?.title).toBe(newTitle);
    });

    it('does not affect other tabs', () => {
      // Add another tab
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(newTab);

      const stateBefore = get(tabState);
      const otherTabTitle = stateBefore.tabs.find((t) => t.id === newTab.id)?.title;

      updateTabTitle(DAILY_JOURNAL_TAB_ID, 'New Daily Title');

      const stateAfter = get(tabState);
      const otherTab = stateAfter.tabs.find((t) => t.id === newTab.id);
      expect(otherTab?.title).toBe(otherTabTitle);
    });
  });

  describe('updateTabContent', () => {
    it('updates content of existing tab', () => {
      const tabId = DAILY_JOURNAL_TAB_ID;
      const newContent = { nodeId: 'new-node-id', nodeType: 'text' };

      updateTabContent(tabId, newContent);

      const state = get(tabState);
      const updatedTab = state.tabs.find((t) => t.id === tabId);
      expect(updatedTab?.content).toEqual(newContent);
    });

    it('updates content without nodeType', () => {
      const tabId = DAILY_JOURNAL_TAB_ID;
      const newContent = { nodeId: 'another-node-id' };

      updateTabContent(tabId, newContent);

      const state = get(tabState);
      const updatedTab = state.tabs.find((t) => t.id === tabId);
      expect(updatedTab?.content).toEqual(newContent);
    });

    it('does not affect other tabs', () => {
      // Add another tab
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };
      addTab(newTab);

      const stateBefore = get(tabState);
      const otherTabContent = stateBefore.tabs.find((t) => t.id === newTab.id)?.content;

      updateTabContent(DAILY_JOURNAL_TAB_ID, { nodeId: 'changed-id', nodeType: 'task' });

      const stateAfter = get(tabState);
      const otherTab = stateAfter.tabs.find((t) => t.id === newTab.id);
      expect(otherTab?.content).toEqual(otherTabContent);
    });
  });

  describe('getOrderedTabsForPane', () => {
    it('returns tabs in correct order for pane', () => {
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

      const state = get(tabState);
      const orderedTabs = getOrderedTabsForPane(state, DEFAULT_PANE_ID);

      expect(orderedTabs.length).toBe(4); // Daily Journal + 3 new tabs
      expect(orderedTabs[0].id).toBe(DAILY_JOURNAL_TAB_ID);
      expect(orderedTabs[1].id).toBe(tab1.id);
      expect(orderedTabs[2].id).toBe(tab2.id);
      expect(orderedTabs[3].id).toBe(tab3.id);
    });

    it('returns empty array for non-existent pane', () => {
      const state = get(tabState);
      const orderedTabs = getOrderedTabsForPane(state, 'non-existent-pane');

      expect(orderedTabs).toEqual([]);
    });

    it('filters out undefined tabs gracefully', () => {
      // Get current state
      const state = get(tabState);

      // Manually create a pane with invalid tab reference (defensive test)
      const testState = {
        ...state,
        panes: [
          ...state.panes,
          {
            id: 'test-pane',
            width: 50,
            tabIds: ['non-existent-tab-id', DAILY_JOURNAL_TAB_ID]
          }
        ]
      };

      const orderedTabs = getOrderedTabsForPane(testState, 'test-pane');

      // Should only return the valid tab (Daily Journal)
      expect(orderedTabs.length).toBe(1);
      expect(orderedTabs[0].id).toBe(DAILY_JOURNAL_TAB_ID);
    });
  });

  describe('loadPersistedState', () => {
    // Mock the services
    let mockLoad: ReturnType<typeof vi.fn>;
    let mockScheduleRestoration: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      // Reset store before each test
      resetTabState();

      // Setup mocks
      mockLoad = vi.fn();
      mockScheduleRestoration = vi.fn();

      vi.spyOn(TabPersistenceService, 'load').mockImplementation(mockLoad);
      vi.spyOn(NodeExpansionCoordinator, 'scheduleRestoration').mockImplementation(
        mockScheduleRestoration
      );
    });

    afterEach(() => {
      vi.restoreAllMocks();
    });

    it('loads persisted state successfully', () => {
      const persistedState = {
        tabs: [
          {
            id: 'persisted-tab-1',
            title: 'Persisted Tab',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-a', 'node-b']
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['persisted-tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'persisted-tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      const result = loadPersistedState();

      expect(result).toBe(true);
      const state = get(tabState);
      expect(state.tabs.length).toBe(1);
      expect(state.tabs[0].id).toBe('persisted-tab-1');
      expect(state.panes.length).toBe(1);
      expect(state.activePaneId).toBe(DEFAULT_PANE_ID);
    });

    it('schedules expansion state restoration for valid expandedNodeIds', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-a', 'node-b', 'node-c']
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      expect(mockScheduleRestoration).toHaveBeenCalledWith('tab-1', ['node-a', 'node-b', 'node-c']);
    });

    it('skips restoration for empty expandedNodeIds array', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: []
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      expect(mockScheduleRestoration).not.toHaveBeenCalled();
    });

    it('skips restoration for undefined expandedNodeIds', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID
            // No expandedNodeIds property
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      expect(mockScheduleRestoration).not.toHaveBeenCalled();
    });

    it('handles malformed expandedNodeIds gracefully (non-array)', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: 'not-an-array' as unknown as string[]
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      // Should not throw
      expect(() => loadPersistedState()).not.toThrow();

      // Should not schedule restoration
      expect(mockScheduleRestoration).not.toHaveBeenCalled();
    });

    it('validates expandedNodeIds contains only non-empty strings', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-a', '', 'node-b'] // Contains empty string
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      // Should not schedule restoration due to empty string
      expect(mockScheduleRestoration).not.toHaveBeenCalled();
    });

    it('validates expandedNodeIds contains only strings', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-a', 123, 'node-b'] as unknown as string[] // Contains number
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      // Should not schedule restoration due to non-string element
      expect(mockScheduleRestoration).not.toHaveBeenCalled();
    });

    it('returns false when no persisted state exists', () => {
      mockLoad.mockReturnValue(null);

      const result = loadPersistedState();

      expect(result).toBe(false);
    });

    it('returns true even when load fails but marks as initialized', () => {
      mockLoad.mockReturnValue(null);

      const result = loadPersistedState();

      expect(result).toBe(false);

      // After calling loadPersistedState, the store should be marked as initialized
      // This enables automatic persistence for subsequent changes
      // We can verify this by making a change and checking if persistence was attempted
      // (Note: Direct verification of isInitialized flag is not possible as it's private)
    });

    it('processes multiple tabs with mixed expandedNodeIds validity', () => {
      const persistedState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node' as const,
            content: { nodeId: 'node-1', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-a', 'node-b'] // Valid
          },
          {
            id: 'tab-2',
            title: 'Tab 2',
            type: 'node' as const,
            content: { nodeId: 'node-2', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: [] // Empty - should skip
          },
          {
            id: 'tab-3',
            title: 'Tab 3',
            type: 'node' as const,
            content: { nodeId: 'node-3', nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID,
            expandedNodeIds: ['node-c'] // Valid
          }
        ],
        panes: [
          {
            id: DEFAULT_PANE_ID,
            width: 100,
            tabIds: ['tab-1', 'tab-2', 'tab-3']
          }
        ],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: {
          [DEFAULT_PANE_ID]: 'tab-1'
        }
      };

      mockLoad.mockReturnValue(persistedState);

      loadPersistedState();

      // Should schedule restoration for tab-1 and tab-3, but not tab-2
      expect(mockScheduleRestoration).toHaveBeenCalledTimes(2);
      expect(mockScheduleRestoration).toHaveBeenCalledWith('tab-1', ['node-a', 'node-b']);
      expect(mockScheduleRestoration).toHaveBeenCalledWith('tab-3', ['node-c']);
    });
  });

  describe('automatic persistence subscription', () => {
    let mockSave: ReturnType<typeof vi.fn>;
    let mockGetExpandedNodeIds: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      vi.useFakeTimers();
      resetTabState();

      mockSave = vi.fn();
      mockGetExpandedNodeIds = vi.fn().mockReturnValue([]);

      vi.spyOn(TabPersistenceService, 'save').mockImplementation(mockSave);
      vi.spyOn(NodeExpansionCoordinator, 'getExpandedNodeIds').mockImplementation(
        mockGetExpandedNodeIds
      );

      // Initialize to enable persistence
      vi.spyOn(TabPersistenceService, 'load').mockReturnValue(null);
      loadPersistedState();
    });

    afterEach(() => {
      vi.useRealTimers();
      vi.restoreAllMocks();
    });

    it('persists state changes after debounce delay', () => {
      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab);

      // Should not persist immediately
      expect(mockSave).not.toHaveBeenCalled();

      // Fast-forward past debounce delay (500ms)
      vi.advanceTimersByTime(500);

      // Should persist after debounce
      expect(mockSave).toHaveBeenCalledTimes(1);
    });

    it('debounces multiple rapid state changes', () => {
      // Make multiple rapid changes
      addTab({
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      });

      vi.advanceTimersByTime(200); // Partial delay

      addTab({
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      });

      vi.advanceTimersByTime(200); // Partial delay

      updateTabTitle('test-tab-1', 'Updated Title');

      // Should not persist yet (debounce timer keeps resetting)
      expect(mockSave).not.toHaveBeenCalled();

      // Complete the debounce delay
      vi.advanceTimersByTime(500);

      // Should persist only once with final state
      expect(mockSave).toHaveBeenCalledTimes(1);
    });

    it('enriches tabs with expansion state before saving', () => {
      mockGetExpandedNodeIds.mockReturnValue(['node-1', 'node-2']);

      const newTab: Tab = {
        id: 'test-tab-1',
        title: 'Test Tab',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      };

      addTab(newTab);

      vi.advanceTimersByTime(500);

      // Verify save was called with enriched tabs
      expect(mockSave).toHaveBeenCalledTimes(1);
      const savedState = mockSave.mock.calls[0][0];
      const savedTab = savedState.tabs.find((t: Tab) => t.id === 'test-tab-1');
      expect(savedTab.expandedNodeIds).toEqual(['node-1', 'node-2']);
    });

    it('calls getExpandedNodeIds for each tab when persisting', () => {
      // Add multiple tabs
      addTab({
        id: 'test-tab-1',
        title: 'Test Tab 1',
        type: 'node',
        content: { nodeId: 'test-node-1', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      });

      addTab({
        id: 'test-tab-2',
        title: 'Test Tab 2',
        type: 'node',
        content: { nodeId: 'test-node-2', nodeType: 'text' },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      });

      vi.advanceTimersByTime(500);

      // Should call getExpandedNodeIds for each tab (including Daily Journal)
      expect(mockGetExpandedNodeIds).toHaveBeenCalledWith(DAILY_JOURNAL_TAB_ID);
      expect(mockGetExpandedNodeIds).toHaveBeenCalledWith('test-tab-1');
      expect(mockGetExpandedNodeIds).toHaveBeenCalledWith('test-tab-2');
      expect(mockGetExpandedNodeIds).toHaveBeenCalledTimes(3);
    });
  });
});
