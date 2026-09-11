import { formatDateISO } from '$lib/utils/date-formatting';
import { clearScrollPosition, clearPaneScrollPositions } from './scroll-state';
import { TabPersistenceService } from '$lib/services/tab-persistence-service';
import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('Navigation');

export interface Tab {
  id: string;
  title: string;
  type: 'node' | 'placeholder' | 'settings' | 'search' | 'conflicts';
  content?: {
    nodeId: string;
    nodeType?: string;
  };
  closeable: boolean;
  paneId: string; // Which pane this tab belongs to
  expandedNodeIds?: string[]; // Sparse array: only store expanded node IDs (collapsed is default)
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

/**
 * Title a newly created node-tab starts as when the caller has no better one
 * to offer yet — `NavigationService.focusOrOpenNode`'s default. The one place
 * this exact string matters is `computeTabTitle` (tab-title.ts): once the
 * node hydrates, a tab still showing exactly this placeholder gets replaced
 * with a generic "Untitled" fallback, but a tab opened with some OTHER
 * caller-supplied title (e.g. a related node's id, used as a stable label
 * when it has neither a title nor content) keeps that title instead — only
 * this literal placeholder is treated as "nothing real to show yet".
 */
export const LOADING_TAB_TITLE = 'Loading...';

const PERSISTENCE_DEBOUNCE_MS = 500;

// Tab state store — initial state
function createInitialTabState(): TabState {
  return {
    tabs: [
      {
        id: DAILY_JOURNAL_TAB_ID,
        title: 'Daily Journal',
        type: 'node',
        content: {
          nodeId: getTodayDateId(),
          nodeType: 'date',
        },
        closeable: true,
        paneId: DEFAULT_PANE_ID,
      },
    ],
    panes: [
      {
        id: DEFAULT_PANE_ID,
        width: 100, // Single pane starts at 100%
        tabIds: [DAILY_JOURNAL_TAB_ID],
      },
    ],
    activePaneId: DEFAULT_PANE_ID,
    activeTabIds: {
      [DEFAULT_PANE_ID]: DAILY_JOURNAL_TAB_ID,
    },
  };
}

class NavigationStore {
  state = $state<TabState>(createInitialTabState());

  // Track initialization state to prevent overwriting loaded state
  #isInitialized = false;

  // Debounce timer for persistence
  #persistenceTimer: ReturnType<typeof setTimeout> | undefined;

  /**
   * Push the current set of open tabs' document root node ids to
   * SharedNodeStore so it can evict cached nodes no open tab/pane reaches
   * any longer (see `SharedNodeStore.updateOpenDocumentRoots`). Called on
   * every tab-state mutation — open, close, content change, and session
   * load/reset — with the FULL current list, so a node whose only open tab
   * just closed becomes an eviction candidate there, and a node reopened
   * before its eviction timer fires gets that timer cancelled.
   *
   * Looked up dynamically (not a module-scope singleton reference) because
   * tests call `SharedNodeStore.resetInstance()` between runs — mirrors the
   * same pattern `reactive-node-service.svelte.ts` uses for the same reason.
   */
  #syncNodeReachability(): void {
    const rootNodeIds: string[] = [];
    for (const tab of this.state.tabs) {
      if (tab.type === 'node' && tab.content?.nodeId) {
        rootNodeIds.push(tab.content.nodeId);
      }
    }
    SharedNodeStore.getInstance().updateOpenDocumentRoots(rootNodeIds);
  }

  /**
   * Persist tab state (debounced). Only persists after initialization to avoid
   * overwriting loaded state during startup. Enriches tabs with expansion state
   * before saving.
   */
  #persist(): void {
    // Reachability sync runs unconditionally (not debounced, not gated on
    // #isInitialized like the disk save below): eviction timers are keyed
    // off the CURRENT open-tab set, so a node closed and reopened inside the
    // disk-persistence debounce window must still see both changes promptly.
    this.#syncNodeReachability();

    if (!this.#isInitialized) return;

    if (this.#persistenceTimer !== undefined) {
      clearTimeout(this.#persistenceTimer);
    }

    // Debounce persistence to avoid rapid-fire saves during interactions
    this.#persistenceTimer = setTimeout(() => {
      // Enrich tabs with expansion state before saving
      const enrichedTabs = this.state.tabs.map((tab) => ({
        ...tab,
        expandedNodeIds: NodeExpansionCoordinator.getExpandedNodeIds(tab.id),
      }));

      TabPersistenceService.save({
        ...this.state,
        tabs: enrichedTabs,
      });
    }, PERSISTENCE_DEBOUNCE_MS);
  }

  /**
   * Load persisted tab state from storage.
   * Should be called once on application startup.
   * @returns True if state was loaded successfully, false if no saved state exists or loading failed
   */
  loadPersistedState(): boolean {
    const persisted = TabPersistenceService.load();

    if (persisted) {
      this.state = {
        tabs: persisted.tabs,
        panes: persisted.panes,
        activePaneId: persisted.activePaneId,
        activeTabIds: persisted.activeTabIds,
      };

      // Schedule expansion state restoration for each tab
      // This will be applied when viewers register (deferred restoration pattern)
      for (const tab of persisted.tabs) {
        // Validate expandedNodeIds before scheduling restoration
        if (
          tab.expandedNodeIds &&
          Array.isArray(tab.expandedNodeIds) &&
          tab.expandedNodeIds.length > 0 &&
          tab.expandedNodeIds.every((id) => typeof id === 'string' && id.length > 0)
        ) {
          NodeExpansionCoordinator.scheduleRestoration(tab.id, tab.expandedNodeIds);
        } else if (tab.expandedNodeIds && !Array.isArray(tab.expandedNodeIds)) {
          // Log warning for malformed data but don't crash
          log.warn(
            `Invalid expandedNodeIds for tab ${tab.id}: expected array, got ${typeof tab.expandedNodeIds}`
          );
        }
      }
    }

    // Enable persistence after load attempt (whether successful or not)
    this.#isInitialized = true;

    // Establish initial reachability from the restored (or default) tab set.
    // Bypasses #persist() (this method intentionally doesn't debounce a save
    // of the state it just loaded), so sync explicitly rather than relying
    // on some later mutation to report it first.
    this.#syncNodeReachability();

    return !!persisted;
  }

  /** Test utility to reset store to initial state */
  resetTabState(): void {
    this.state = createInitialTabState();
    this.#syncNodeReachability();
  }

  /** Clear all tabs and panes (used during database hot-swap) */
  clearAllTabs(): void {
    this.state = {
      tabs: [],
      panes: [
        {
          id: DEFAULT_PANE_ID,
          width: 100,
          tabIds: [],
        },
      ],
      activePaneId: DEFAULT_PANE_ID,
      activeTabIds: {},
    };
    this.#persist();
  }

  // Pane Management

  /**
   * Creates a new pane with 50/50 split. Maximum 2 panes supported.
   * @returns The created pane or null if max panes reached
   */
  createPane(): Pane | null {
    const state = this.state;
    // Prevent creating more than 2 panes
    if (state.panes.length >= 2) {
      return null;
    }

    // Generate unique pane ID by finding the highest existing pane number and incrementing
    // This prevents duplicate IDs when panes are closed and recreated
    const existingPaneNumbers = state.panes
      .map((p) => {
        const match = p.id.match(/^pane-(\d+)$/);
        return match ? parseInt(match[1], 10) : 0;
      })
      .filter((n) => !isNaN(n));
    const maxPaneNumber = existingPaneNumbers.length > 0 ? Math.max(...existingPaneNumbers) : 0;
    const newPaneId = `pane-${maxPaneNumber + 1}`;

    // Create new pane with 50% width
    const newPane: Pane = {
      id: newPaneId,
      width: 50,
      tabIds: [],
    };

    // Update existing panes to 50% width
    const updatedPanes = state.panes.map((pane) => ({ ...pane, width: 50 }));

    this.state = { ...state, panes: [...updatedPanes, newPane] };
    this.#persist();

    return newPane;
  }

  /**
   * Closes a pane and expands remaining pane to 100%. Cannot close the last pane.
   */
  closePane(paneId: string): void {
    // Clean up scroll positions for all viewers in this pane
    clearPaneScrollPositions(paneId);

    const state = this.state;
    // Cannot close the last pane
    if (state.panes.length <= 1) {
      return;
    }

    // Remove the pane
    const remainingPanes = state.panes.filter((pane) => pane.id !== paneId);

    // Expand remaining pane to 100%
    const updatedPanes = remainingPanes.map((pane) => ({ ...pane, width: 100 }));

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

    this.state = {
      ...state,
      panes: updatedPanes,
      tabs: remainingTabs,
      activePaneId: newActivePaneId,
      activeTabIds: newActiveTabIds,
    };
    this.#persist();
  }

  /** Sets the active pane */
  setActivePane(paneId: string): void {
    const state = this.state;
    // Verify pane exists
    const paneExists = state.panes.some((pane) => pane.id === paneId);
    if (!paneExists) {
      return;
    }

    this.state = { ...state, activePaneId: paneId };
    this.#persist();
  }

  /** Resizes panes maintaining 100% total width */
  resizePane(paneId: string, newWidth: number): void {
    const state = this.state;
    // Only works with 2 panes
    if (state.panes.length !== 2) {
      return;
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

    this.state = { ...state, panes: updatedPanes };
    this.#persist();
  }

  // Tab Management

  /** Sets the active tab in the specified pane */
  setActiveTab(tabId: string, paneId?: string): void {
    const state = this.state;
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) {
      return;
    }

    const targetPaneId = paneId || tab.paneId;

    this.state = {
      ...state,
      activePaneId: targetPaneId,
      activeTabIds: {
        ...state.activeTabIds,
        [targetPaneId]: tabId,
      },
    };
    this.#persist();
  }

  /** Closes a tab and auto-closes the pane if it's the last tab */
  closeTab(tabId: string): void {
    const state = this.state;
    const tab = state.tabs.find((t) => t.id === tabId);
    if (!tab) {
      return;
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
      return;
    }

    // Check if this is the last tab in the last pane
    const tabsInPane = state.tabs.filter((t) => t.paneId === paneId);
    if (tabsInPane.length === 1 && state.panes.length === 1) {
      // Cannot close last tab in last pane
      return;
    }

    // Remove the tab
    const newTabs = state.tabs.filter((t) => t.id !== tabId);

    // Update pane's tab list
    const updatedPanes = state.panes.map((p) => {
      if (p.id === paneId) {
        return { ...p, tabIds: p.tabIds.filter((id) => id !== tabId) };
      }
      return p;
    });

    // If this was the last tab in the pane, close the pane
    const remainingTabsInPane = newTabs.filter((t) => t.paneId === paneId);
    if (remainingTabsInPane.length === 0 && state.panes.length > 1) {
      // Close the empty pane
      const remainingPanes = updatedPanes.filter((p) => p.id !== paneId);

      // Expand remaining pane to 100%
      const expandedPanes = remainingPanes.map((p) => ({ ...p, width: 100 }));

      // Update active pane if necessary
      let newActivePaneId = state.activePaneId;
      if (paneId === state.activePaneId && remainingPanes.length > 0) {
        newActivePaneId = remainingPanes[0].id;
      }

      // Update active tab IDs map
      const newActiveTabIds = { ...state.activeTabIds };
      delete newActiveTabIds[paneId];

      this.state = {
        ...state,
        panes: expandedPanes,
        tabs: newTabs,
        activePaneId: newActivePaneId,
        activeTabIds: newActiveTabIds,
      };
      this.#persist();
      return;
    }

    // Update active tab in this pane if we closed the active one
    const newActiveTabIds = { ...state.activeTabIds };
    if (tabId === state.activeTabIds[paneId]) {
      const firstRemainingTab = remainingTabsInPane[0];
      if (firstRemainingTab) {
        newActiveTabIds[paneId] = firstRemainingTab.id;
      }
    }

    this.state = {
      ...state,
      panes: updatedPanes,
      tabs: newTabs,
      activeTabIds: newActiveTabIds,
    };
    this.#persist();
  }

  /** Adds a new tab to the specified pane */
  addTab(tab: Tab, makeActive: boolean = true): void {
    const state = this.state;
    // Verify pane exists
    const paneExists = state.panes.some((pane) => pane.id === tab.paneId);
    if (!paneExists) {
      log.error(`Pane ${tab.paneId} does not exist`);
      return;
    }

    // Add tab to pane's tab list
    const updatedPanes = state.panes.map((pane) => {
      if (pane.id === tab.paneId) {
        return { ...pane, tabIds: [...pane.tabIds, tab.id] };
      }
      return pane;
    });

    // Only update active tab/pane if makeActive is true
    const newState: TabState = {
      ...state,
      tabs: [...state.tabs, tab],
      panes: updatedPanes,
    };

    if (makeActive) {
      newState.activePaneId = tab.paneId;
      newState.activeTabIds = {
        ...state.activeTabIds,
        [tab.paneId]: tab.id,
      };
    }

    this.state = newState;
    this.#persist();
  }

  updateTabContent(tabId: string, content: { nodeId: string; nodeType?: string }): void {
    this.state = {
      ...this.state,
      tabs: this.state.tabs.map((tab) => (tab.id === tabId ? { ...tab, content } : tab)),
    };
    this.#persist();
  }

  /** Reorder a tab within the same pane */
  reorderTab(tabId: string, newIndex: number, paneId: string): void {
    const state = this.state;
    const pane = state.panes.find((p) => p.id === paneId);
    if (!pane) {
      return;
    }

    const currentIndex = pane.tabIds.indexOf(tabId);
    if (currentIndex === -1) {
      return;
    }

    // Don't do anything if moving to same position
    if (currentIndex === newIndex) {
      return;
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

    this.state = { ...state, panes: updatedPanes };
    this.#persist();
  }

  /**
   * Move a tab from one pane to another.
   * If source pane becomes empty, it will be closed automatically.
   */
  moveTabBetweenPanes(
    tabId: string,
    sourcePaneId: string,
    targetPaneId: string,
    targetIndex: number
  ): void {
    const state = this.state;
    const sourcePane = state.panes.find((p) => p.id === sourcePaneId);
    const targetPane = state.panes.find((p) => p.id === targetPaneId);
    const tab = state.tabs.find((t) => t.id === tabId);

    if (!sourcePane || !targetPane || !tab) {
      return;
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
      updatedPanes = updatedPanes.map((p) => ({ ...p, width: 100 }));

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

      this.state = {
        ...state,
        tabs: updatedTabs,
        panes: updatedPanes,
        activePaneId: newActivePaneId,
        activeTabIds: newActiveTabIds,
      };
      this.#persist();
      return;
    }

    // Update active tab in source pane if we moved the active tab
    const newActiveTabIds = { ...state.activeTabIds };
    if (state.activeTabIds[sourcePaneId] === tabId) {
      // Set first remaining tab as active in source pane
      if (sourceTabIds.length > 0) {
        newActiveTabIds[sourcePaneId] = sourceTabIds[0];
      }
    }

    // Set moved tab as active in target pane
    newActiveTabIds[targetPaneId] = tabId;

    this.state = {
      ...state,
      tabs: updatedTabs,
      panes: updatedPanes,
      activePaneId: targetPaneId,
      activeTabIds: newActiveTabIds,
    };
    this.#persist();
  }
}

export const navigationStore = new NavigationStore();

// ---------------------------------------------------------------------------
// Thin free-function delegators — keep existing callers working unchanged.
// ---------------------------------------------------------------------------

export const loadPersistedState = (): boolean => navigationStore.loadPersistedState();
export const resetTabState = (): void => navigationStore.resetTabState();
export const clearAllTabs = (): void => navigationStore.clearAllTabs();
export const createPane = (): Pane | null => navigationStore.createPane();
export const closePane = (paneId: string): void => navigationStore.closePane(paneId);
export const setActivePane = (paneId: string): void => navigationStore.setActivePane(paneId);
export const resizePane = (paneId: string, newWidth: number): void =>
  navigationStore.resizePane(paneId, newWidth);
export const setActiveTab = (tabId: string, paneId?: string): void =>
  navigationStore.setActiveTab(tabId, paneId);
export const closeTab = (tabId: string): void => navigationStore.closeTab(tabId);
export const addTab = (tab: Tab, makeActive: boolean = true): void =>
  navigationStore.addTab(tab, makeActive);
export const updateTabContent = (
  tabId: string,
  content: { nodeId: string; nodeType?: string }
): void => navigationStore.updateTabContent(tabId, content);
export const reorderTab = (tabId: string, newIndex: number, paneId: string): void =>
  navigationStore.reorderTab(tabId, newIndex, paneId);
export const moveTabBetweenPanes = (
  tabId: string,
  sourcePaneId: string,
  targetPaneId: string,
  targetIndex: number
): void => navigationStore.moveTabBetweenPanes(tabId, sourcePaneId, targetPaneId, targetIndex);

/**
 * Get ordered tabs for a specific pane.
 * @param state - The current tab state
 * @param paneId - The pane ID to get tabs for
 * @returns Array of tabs in the order specified by the pane's tabIds array
 */
export function getOrderedTabsForPane(state: TabState, paneId: string): Tab[] {
  const pane = state.panes.find((p) => p.id === paneId);
  if (!pane) return [];

  // Single-pass: map tabIds to tabs, filter out undefined
  return pane.tabIds
    .map((tabId) => state.tabs.find((t) => t.id === tabId))
    .filter((t): t is Tab => t !== undefined);
}

// Re-export from shared utility
export { formatDateTitle as getDateTabTitle } from '$lib/utils/date-formatting';
