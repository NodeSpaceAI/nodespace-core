import { writable } from 'svelte/store';

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
  const today = new Date();
  const year = today.getFullYear();
  const month = String(today.getMonth() + 1).padStart(2, '0');
  const day = String(today.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
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

// Helper function to format date for tab title
export function getDateTabTitle(date: Date): string {
  const today = new Date();
  const tomorrow = new Date(today);
  tomorrow.setDate(tomorrow.getDate() + 1);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);

  // Normalize dates to compare only year, month, day (ignore time)
  const normalizeDate = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const targetDate = normalizeDate(date);
  const todayNormalized = normalizeDate(today);
  const tomorrowNormalized = normalizeDate(tomorrow);
  const yesterdayNormalized = normalizeDate(yesterday);

  if (targetDate.getTime() === todayNormalized.getTime()) {
    return 'Today';
  } else if (targetDate.getTime() === tomorrowNormalized.getTime()) {
    return 'Tomorrow';
  } else if (targetDate.getTime() === yesterdayNormalized.getTime()) {
    return 'Yesterday';
  } else {
    // Format as YYYY-MM-DD using local timezone (not UTC)
    const year = targetDate.getFullYear();
    const month = String(targetDate.getMonth() + 1).padStart(2, '0');
    const day = String(targetDate.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
  }
}
