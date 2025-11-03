import { writable } from 'svelte/store';
import { formatDateISO } from '$lib/utils/date-formatting';

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

// Re-export from shared utility for backward compatibility
export { formatDateTitle as getDateTabTitle } from '$lib/utils/date-formatting';
