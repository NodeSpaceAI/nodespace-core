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
}

export interface TabState {
  tabs: Tab[];
  activeTabId: string;
}

// Helper to get today's date in YYYY-MM-DD format
function getTodayDateId(): string {
  return formatDateISO(new Date());
}

// Stable ID for the Daily Journal tab (accessed via sidebar)
export const DAILY_JOURNAL_TAB_ID = 'daily-journal';

// Tab state store
const initialTabState: TabState = {
  tabs: [
    {
      id: DAILY_JOURNAL_TAB_ID, // Stable ID for sidebar navigation
      title: 'Daily Journal', // DateNodeViewer will update based on current date
      type: 'node',
      content: {
        nodeId: getTodayDateId(), // Starts with today, but user can navigate to any date
        nodeType: 'date'
      },
      closeable: false
    }
  ],
  activeTabId: DAILY_JOURNAL_TAB_ID
};

export const tabState = writable<TabState>(initialTabState);

// Helper functions
export function setActiveTab(tabId: string) {
  tabState.update((state) => ({
    ...state,
    activeTabId: tabId
  }));
}

export function closeTab(tabId: string) {
  tabState.update((state) => {
    const newTabs = state.tabs.filter((tab) => tab.id !== tabId);
    let newActiveTabId = state.activeTabId;

    // If we closed the active tab, switch to first remaining tab
    if (tabId === state.activeTabId && newTabs.length > 0) {
      newActiveTabId = newTabs[0].id;
    }

    return {
      tabs: newTabs,
      activeTabId: newActiveTabId
    };
  });
}

export function addTab(tab: Tab) {
  tabState.update((state) => ({
    ...state,
    tabs: [...state.tabs, tab],
    activeTabId: tab.id
  }));
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
