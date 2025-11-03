import { writable } from 'svelte/store';
import { formatDateISO } from '$lib/utils/date-formatting';
import { clearScrollPosition, clearPaneScrollPositions } from './scroll-state';
import { TabPersistenceService } from '$lib/services/tab-persistence-service';

export interface Tab {
  id: string;
  title: string;
  type: 'node' | 'placeholder';
  content?: {
    nodeId: string;
    nodeType?: string;
  };
  closeable: boolean;
  paneId: string; // Which pane this tab belongs to
}

export interface Pane {
  id: string;
  width: number; // Percentage width (0-100)
  tabIds: string[]; // Array of tab IDs in this pane
}

export interface TabState {
  tabs: Tab[];
  panes: Pane[];
  activePaneId: string; // Currently focused pane
  activeTabIds: Record<string, string>; // Map of paneId -> activeTabId
}

// Helper to get today's date in YYYY-MM-DD format
function getTodayDateId(): string {
  return formatDateISO(new Date());
}

// Stable IDs for panes and tabs
export const DAILY_JOURNAL_TAB_ID = 'daily-journal';
export const DEFAULT_PANE_ID = 'pane-1';

// Tab state store
const initialTabState: TabState = {
  tabs: [
    {
      id: DAILY_JOURNAL_TAB_ID,
      title: 'Daily Journal',
      type: 'node',
      content: {
        nodeId: getTodayDateId(),
        nodeType: 'date'
      },
      closeable: true,
      paneId: DEFAULT_PANE_ID
    }
  ],
  panes: [
    {
      id: DEFAULT_PANE_ID,
      width: 100, // Single pane starts at 100%
      tabIds: [DAILY_JOURNAL_TAB_ID]
    }
  ],
  activePaneId: DEFAULT_PANE_ID,
  activeTabIds: {
    [DEFAULT_PANE_ID]: DAILY_JOURNAL_TAB_ID
  }
};

export const tabState = writable<TabState>(initialTabState);

// Track initialization state to prevent overwriting loaded state
let isInitialized = false;

// Subscribe to state changes and persist automatically
tabState.subscribe((state) => {
  // Only persist after initialization to avoid overwriting loaded state
  if (isInitialized) {
    TabPersistenceService.save(state);
  }
});

/**
 * Load persisted tab state from storage
 * Should be called once on application startup
 * @returns True if state was loaded successfully, false if no saved state exists or loading failed
 */
export function loadPersistedState(): boolean {
  const persisted = TabPersistenceService.load();

  if (persisted) {
    // Restore the state
    tabState.set({
      tabs: persisted.tabs,
      panes: persisted.panes,
      activePaneId: persisted.activePaneId,
      activeTabIds: persisted.activeTabIds
    });
  }

  // Enable persistence after load attempt (whether successful or not)
  isInitialized = true;

  return !!persisted;
}

// Test utility to reset store to initial state
export function resetTabState() {
  tabState.set({
    tabs: [
      {
        id: DAILY_JOURNAL_TAB_ID,
        title: 'Daily Journal',
        type: 'node',
        content: {
          nodeId: getTodayDateId(),
          nodeType: 'date'
        },
        closeable: true,
        paneId: DEFAULT_PANE_ID
      }
    ],
    panes: [
      {
        id: DEFAULT_PANE_ID,
        width: 100,
        tabIds: [DAILY_JOURNAL_TAB_ID]
      }
    ],
    activePaneId: DEFAULT_PANE_ID,
    activeTabIds: {
      [DEFAULT_PANE_ID]: DAILY_JOURNAL_TAB_ID
    }
  });
}

// Pane Management Functions

/**
 * Creates a new pane with 50/50 split
 * Maximum 2 panes supported
 * @returns The created pane or null if max panes reached
 */
export function createPane(): Pane | null {
  let createdPane: Pane | null = null;

  tabState.update((state) => {
    // Prevent creating more than 2 panes
    if (state.panes.length >= 2) {
      return state;
    }

    // Create new pane with 50% width
    const newPane: Pane = {
      id: `pane-${state.panes.length + 1}`,
      width: 50,
      tabIds: []
    };

    // Update existing panes to 50% width
    const updatedPanes = state.panes.map((pane) => ({
      ...pane,
      width: 50
    }));

    createdPane = newPane;

    return {
      ...state,
      panes: [...updatedPanes, newPane]
    };
  });

  return createdPane;
}

/**
 * Closes a pane and expands remaining pane to 100%
 * Cannot close the last pane
 * @param paneId - The pane ID to close
 */
export function closePane(paneId: string) {
  // Clean up scroll positions for all viewers in this pane
  clearPaneScrollPositions(paneId);

  tabState.update((state) => {
    // Cannot close the last pane
    if (state.panes.length <= 1) {
      return state;
    }

    // Remove the pane
    const remainingPanes = state.panes.filter((pane) => pane.id !== paneId);

    // Expand remaining pane to 100%
    const updatedPanes = remainingPanes.map((pane) => ({
      ...pane,
      width: 100
    }));

    // Remove all tabs belonging to this pane
    const remainingTabs = state.tabs.filter((tab) => tab.paneId !== paneId);

    // Update active pane if necessary
    let newActivePaneId = state.activePaneId;
    if (paneId === state.activePaneId && remainingPanes.length > 0) {
      newActivePaneId = remainingPanes[0].id;
    }

    // Update active tab IDs map
    const newActiveTabIds = { ...state.activeTabIds };
    delete newActiveTabIds[paneId];

    return {
      ...state,
      panes: updatedPanes,
      tabs: remainingTabs,
      activePaneId: newActivePaneId,
      activeTabIds: newActiveTabIds
    };
  });
}

/**
 * Sets the active pane
 * @param paneId - The pane ID to set as active
 */
export function setActivePane(paneId: string) {
  tabState.update((state) => {
    // Verify pane exists
    const paneExists = state.panes.some((pane) => pane.id === paneId);
    if (!paneExists) {
      return state;
    }

    return {
      ...state,
      activePaneId: paneId
    };
  });
}

/**
 * Resizes panes maintaining 100% total width
 * @param paneId - The pane ID to resize
 * @param newWidth - New width percentage (0-100)
 */
export function resizePane(paneId: string, newWidth: number) {
  tabState.update((state) => {
    // Only works with 2 panes
    if (state.panes.length !== 2) {
      return state;
    }

    // Enforce minimum 200px (approximate percentage based on typical viewport)
    const minWidth = 20; // ~200px at 1000px viewport width
    const clampedWidth = Math.max(minWidth, Math.min(100 - minWidth, newWidth));

    const updatedPanes = state.panes.map((pane) => {
      if (pane.id === paneId) {
        return { ...pane, width: clampedWidth };
      } else {
        // Other pane gets remaining width
        return { ...pane, width: 100 - clampedWidth };
      }
    });

    return {
      ...state,
      panes: updatedPanes
    };
  });
}

// Tab Management Functions

/**
 * Sets the active tab in the specified pane
 * @param tabId - The tab ID to set as active
 * @param paneId - The pane ID containing the tab
 */
export function setActiveTab(tabId: string, paneId?: string) {
  tabState.update((state) => {
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) {
      return state;
    }

    const targetPaneId = paneId || tab.paneId;

    return {
      ...state,
      activePaneId: targetPaneId,
      activeTabIds: {
        ...state.activeTabIds,
        [targetPaneId]: tabId
      }
    };
  });
}

/**
 * Closes a tab and auto-closes the pane if it's the last tab
 * @param tabId - The tab ID to close
 */
export function closeTab(tabId: string) {
  tabState.update((state) => {
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) {
      return state;
    }

    const paneId = tab.paneId;

    // Clean up scroll positions for all panes that had this tab
    // Since tabs can appear in multiple panes (split view), clean up all combinations
    state.panes.forEach((pane) => {
      const viewerId = `${tabId}-${pane.id}`;
      clearScrollPosition(viewerId);
    });
    const pane = state.panes.find((p) => p.id === paneId);
    if (!pane) {
      return state;
    }

    // Check if this is the last tab in the last pane
    const tabsInPane = state.tabs.filter((t) => t.paneId === paneId);
    if (tabsInPane.length === 1 && state.panes.length === 1) {
      // Cannot close last tab in last pane
      return state;
    }

    // Remove the tab
    const newTabs = state.tabs.filter((t) => t.id !== tabId);

    // Update pane's tab list
    const updatedPanes = state.panes.map((p) => {
      if (p.id === paneId) {
        return {
          ...p,
          tabIds: p.tabIds.filter((id) => id !== tabId)
        };
      }
      return p;
    });

    // If this was the last tab in the pane, close the pane
    const remainingTabsInPane = newTabs.filter((t) => t.paneId === paneId);
    if (remainingTabsInPane.length === 0 && state.panes.length > 1) {
      // Close the empty pane
      const remainingPanes = updatedPanes.filter((p) => p.id !== paneId);

      // Expand remaining pane to 100%
      const expandedPanes = remainingPanes.map((p) => ({
        ...p,
        width: 100
      }));

      // Update active pane if necessary
      let newActivePaneId = state.activePaneId;
      if (paneId === state.activePaneId && remainingPanes.length > 0) {
        newActivePaneId = remainingPanes[0].id;
      }

      // Update active tab IDs map
      const newActiveTabIds = { ...state.activeTabIds };
      delete newActiveTabIds[paneId];

      return {
        ...state,
        panes: expandedPanes,
        tabs: newTabs,
        activePaneId: newActivePaneId,
        activeTabIds: newActiveTabIds
      };
    }

    // Update active tab in this pane if we closed the active one
    let newActiveTabIds = { ...state.activeTabIds };
    if (tabId === state.activeTabIds[paneId]) {
      const firstRemainingTab = remainingTabsInPane[0];
      if (firstRemainingTab) {
        newActiveTabIds[paneId] = firstRemainingTab.id;
      }
    }

    return {
      ...state,
      panes: updatedPanes,
      tabs: newTabs,
      activeTabIds: newActiveTabIds
    };
  });
}

/**
 * Adds a new tab to the specified pane
 * @param tab - The tab to add
 */
export function addTab(tab: Tab) {
  tabState.update((state) => {
    // Verify pane exists
    const paneExists = state.panes.some((pane) => pane.id === tab.paneId);
    if (!paneExists) {
      console.error(`Pane ${tab.paneId} does not exist`);
      return state;
    }

    // Add tab to pane's tab list
    const updatedPanes = state.panes.map((pane) => {
      if (pane.id === tab.paneId) {
        return {
          ...pane,
          tabIds: [...pane.tabIds, tab.id]
        };
      }
      return pane;
    });

    return {
      ...state,
      tabs: [...state.tabs, tab],
      panes: updatedPanes,
      activePaneId: tab.paneId,
      activeTabIds: {
        ...state.activeTabIds,
        [tab.paneId]: tab.id
      }
    };
  });
}

export function updateTabTitle(tabId: string, newTitle: string) {
  tabState.update((state) => ({
    ...state,
    tabs: state.tabs.map((tab) => (tab.id === tabId ? { ...tab, title: newTitle } : tab))
  }));
}

export function updateTabContent(tabId: string, content: { nodeId: string; nodeType?: string }) {
  tabState.update((state) => ({
    ...state,
    tabs: state.tabs.map((tab) => (tab.id === tabId ? { ...tab, content } : tab))
  }));
}

/**
 * Get ordered tabs for a specific pane
 * @param state - The current tab state
 * @param paneId - The pane ID to get tabs for
 * @returns Array of tabs in the order specified by the pane's tabIds array
 *
 * @remarks
 * This function is optimized to use a single-pass approach for ordering tabs.
 * It maps through the pane's tabIds array and looks up each tab, filtering out
 * any undefined results (which shouldn't occur in normal operation).
 */
export function getOrderedTabsForPane(state: TabState, paneId: string): Tab[] {
  const pane = state.panes.find((p) => p.id === paneId);
  if (!pane) return [];

  // Single-pass: map tabIds to tabs, filter out undefined
  return pane.tabIds
    .map((tabId) => state.tabs.find((t) => t.id === tabId))
    .filter((t): t is Tab => t !== undefined);
}

/**
 * Reorder a tab within the same pane
 * @param tabId - The tab ID to reorder
 * @param newIndex - The new position index in the pane's tab list
 * @param paneId - The pane ID containing the tab
 */
export function reorderTab(tabId: string, newIndex: number, paneId: string): void {
  tabState.update((state) => {
    const pane = state.panes.find((p) => p.id === paneId);
    if (!pane) {
      return state;
    }

    const currentIndex = pane.tabIds.indexOf(tabId);
    if (currentIndex === -1) {
      return state;
    }

    // Don't do anything if moving to same position
    if (currentIndex === newIndex) {
      return state;
    }

    // Create new tabIds array with reordered tabs
    const newTabIds = [...pane.tabIds];
    newTabIds.splice(currentIndex, 1); // Remove from current position
    newTabIds.splice(newIndex, 0, tabId); // Insert at new position

    // Update pane with new tabIds order
    const updatedPanes = state.panes.map((p) => {
      if (p.id === paneId) {
        return { ...p, tabIds: newTabIds };
      }
      return p;
    });

    return {
      ...state,
      panes: updatedPanes
    };
  });
}

/**
 * Move a tab from one pane to another
 * If source pane becomes empty, it will be closed automatically
 * @param tabId - The tab ID to move
 * @param sourcePaneId - The source pane ID
 * @param targetPaneId - The target pane ID
 * @param targetIndex - The position index in target pane's tab list
 */
export function moveTabBetweenPanes(
  tabId: string,
  sourcePaneId: string,
  targetPaneId: string,
  targetIndex: number
): void {
  tabState.update((state) => {
    const sourcePane = state.panes.find((p) => p.id === sourcePaneId);
    const targetPane = state.panes.find((p) => p.id === targetPaneId);
    const tab = state.tabs.find((t) => t.id === tabId);

    if (!sourcePane || !targetPane || !tab) {
      return state;
    }

    // Update tab's paneId
    const updatedTab = { ...tab, paneId: targetPaneId };

    // Update tabs array
    const updatedTabs = state.tabs.map((t) => (t.id === tabId ? updatedTab : t));

    // Remove tab from source pane's tabIds
    const sourceTabIds = sourcePane.tabIds.filter((id) => id !== tabId);

    // Add tab to target pane's tabIds at specified index
    const targetTabIds = [...targetPane.tabIds];
    targetTabIds.splice(targetIndex, 0, tabId);

    // Update panes
    let updatedPanes = state.panes.map((p) => {
      if (p.id === sourcePaneId) {
        return { ...p, tabIds: sourceTabIds };
      }
      if (p.id === targetPaneId) {
        return { ...p, tabIds: targetTabIds };
      }
      return p;
    });

    // Check if source pane is now empty
    if (sourceTabIds.length === 0 && state.panes.length > 1) {
      // Close source pane
      updatedPanes = updatedPanes.filter((p) => p.id !== sourcePaneId);

      // Expand remaining pane to 100%
      updatedPanes = updatedPanes.map((p) => ({
        ...p,
        width: 100
      }));

      // Update active pane if necessary
      let newActivePaneId = state.activePaneId;
      if (sourcePaneId === state.activePaneId) {
        newActivePaneId = targetPaneId;
      }

      // Update active tab IDs map
      const newActiveTabIds = { ...state.activeTabIds };
      delete newActiveTabIds[sourcePaneId];

      // Set moved tab as active in target pane
      newActiveTabIds[targetPaneId] = tabId;

      return {
        ...state,
        tabs: updatedTabs,
        panes: updatedPanes,
        activePaneId: newActivePaneId,
        activeTabIds: newActiveTabIds
      };
    }

    // Update active tab in source pane if we moved the active tab
    let newActiveTabIds = { ...state.activeTabIds };
    if (state.activeTabIds[sourcePaneId] === tabId) {
      // Set first remaining tab as active in source pane
      if (sourceTabIds.length > 0) {
        newActiveTabIds[sourcePaneId] = sourceTabIds[0];
      }
    }

    // Set moved tab as active in target pane
    newActiveTabIds[targetPaneId] = tabId;

    return {
      ...state,
      tabs: updatedTabs,
      panes: updatedPanes,
      activePaneId: targetPaneId,
      activeTabIds: newActiveTabIds
    };
  });
}

// Re-export from shared utility for backward compatibility
export { formatDateTitle as getDateTabTitle } from '$lib/utils/date-formatting';
