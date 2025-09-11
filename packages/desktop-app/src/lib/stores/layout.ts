import { writable } from 'svelte/store';

export interface LayoutState {
  sidebarCollapsed: boolean;
  activePane: string;
}

export interface NavigationItem {
  id: string;
  label: string;
  icon: string;
  active: boolean;
  type: 'link' | 'placeholder';
}

// Layout state store
const initialLayoutState: LayoutState = {
  sidebarCollapsed: false,
  activePane: 'today'
};

export const layoutState = writable<LayoutState>(initialLayoutState);

// Navigation items store - updated to match design system patterns.html
export const navigationItems = writable<NavigationItem[]>([
  {
    id: 'daily-journal',
    label: 'Daily Journal',
    icon: 'm3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z', // home icon
    active: true,
    type: 'link'
  },
  {
    id: 'dashboard',
    label: 'Dashboard',
    icon: 'M3 3h18v18H3V3zM9 15h6', // dashboard icon
    active: false,
    type: 'placeholder'
  },
  {
    id: 'search',
    label: 'Search',
    icon: 'M11 11m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0M21 21l-4.35-4.35', // search icon
    active: false,
    type: 'placeholder'
  },
  {
    id: 'favorites',
    label: 'Favorites',
    icon: 'M12 2l3.09 6.26L22 9.27l-5 4.87L18.18 21.02L12 17.77l-6.18 3.25L7 14.14l-5-4.87l6.91-1.01L12 2z', // star icon
    active: false,
    type: 'placeholder'
  },
  {
    id: 'library',
    label: 'Library',
    icon: 'M3 3h18v18H3V3zM9 15h6', // placeholder dashboard-style icon
    active: false,
    type: 'placeholder'
  }
]);

// Helper functions
export function toggleSidebar() {
  layoutState.update((state) => ({
    ...state,
    sidebarCollapsed: !state.sidebarCollapsed
  }));
}

export function setActivePane(paneId: string) {
  layoutState.update((state) => ({
    ...state,
    activePane: paneId
  }));
}
