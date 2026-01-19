import { writable } from 'svelte/store';
import { LayoutPersistenceService } from '$lib/services/layout-persistence-service';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('Layout');

export interface LayoutState {
  sidebarCollapsed: boolean;
  activePane: string;
  collectionsExpanded: boolean;
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
  activePane: 'today',
  collectionsExpanded: false
};

export const layoutState = writable<LayoutState>(initialLayoutState);

// Track initialization state to prevent overwriting loaded state
let isInitialized = false;

// Subscribe to state changes and persist automatically
// Note: LayoutPersistenceService.save() handles debouncing internally
layoutState.subscribe((state) => {
  // Only persist after initialization to avoid overwriting loaded state
  if (isInitialized) {
    LayoutPersistenceService.save(state);
  }
});

/**
 * Load persisted layout state from storage
 * Should be called once on application startup
 * Idempotent - safe to call multiple times (subsequent calls are no-ops)
 * @returns True if state was loaded successfully, false if no saved state exists or loading failed
 */
export function loadPersistedLayoutState(): boolean {
  // Guard against multiple initializations (e.g., component remounting)
  if (isInitialized) {
    log.warn('loadPersistedLayoutState called after initialization, ignoring');
    return false;
  }

  const persisted = LayoutPersistenceService.load();

  if (persisted) {
    // Restore the state
    layoutState.set({
      sidebarCollapsed: persisted.sidebarCollapsed,
      activePane: 'today', // Keep activePane at default for now (not persisted)
      collectionsExpanded: persisted.collectionsExpanded ?? false
    });
  }

  // Enable persistence after load attempt (whether successful or not)
  isInitialized = true;

  return !!persisted;
}

// Navigation items store - updated to match design system patterns.html
export const navigationItems = writable<NavigationItem[]>([
  {
    id: 'daily-journal',
    label: 'Daily Journal',
    icon: 'm3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z', // home icon
    active: false,
    type: 'link'
  },
  // Note: "Collections" is rendered inline in NavigationSidebar
  // using bits-ui Collapsible - inserted after Daily Journal
  {
    id: 'search',
    label: 'Search',
    icon: 'M11 11m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0M21 21l-4.35-4.35', // search icon
    active: false,
    type: 'link'
  },
  {
    id: 'favorites',
    label: 'Favorites',
    icon: 'M12 2l3.09 6.26L22 9.27l-5 4.87L18.18 21.02L12 17.77l-6.18 3.25L7 14.14l-5-4.87l6.91-1.01L12 2z', // star icon
    active: false,
    type: 'link'
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

export function setCollectionsExpanded(expanded: boolean) {
  layoutState.update((state) => ({
    ...state,
    collectionsExpanded: expanded
  }));
}

export function toggleCollectionsExpanded() {
  layoutState.update((state) => ({
    ...state,
    collectionsExpanded: !state.collectionsExpanded
  }));
}
