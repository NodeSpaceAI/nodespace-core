import { LayoutPersistenceService } from '$lib/services/layout-persistence-service';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('Layout');

export interface LayoutState {
  sidebarCollapsed: boolean;
  activePane: string;
  collectionsExpanded: boolean;
  schemaTypesExpanded: boolean;
  aiChatsExpanded: boolean;
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
  collectionsExpanded: false,
  schemaTypesExpanded: false,
  aiChatsExpanded: false,
};

const initialNavigationItems: NavigationItem[] = [
  {
    id: 'daily-journal',
    label: 'Daily Journal',
    icon: 'm3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z', // home icon
    active: false,
    type: 'link',
  },
  // Note: "Collections" is rendered inline in NavigationSidebar
  // using bits-ui Collapsible - inserted after Daily Journal
  //
  // "AI Chat" nav item is temporarily removed. The old item opened a single
  // ephemeral chat tab backed by the now-deleted chatStore singleton (pre-ADR-034).
  // Every conversation is now an `ai-chat` node, so this item will be reintroduced
  // as an expandable list of recent ai-chat nodes (like Collections / Schema Types)
  // in a follow-up. Until then it is omitted rather than left inert.
  {
    id: 'search',
    label: 'Search',
    icon: 'M11 11m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0M21 21l-4.35-4.35', // search icon
    active: false,
    type: 'link',
  },
  {
    id: 'favorites',
    label: 'Favorites',
    icon: 'M12 2l3.09 6.26L22 9.27l-5 4.87L18.18 21.02L12 17.77l-6.18 3.25L7 14.14l-5-4.87l6.91-1.01L12 2z', // star icon
    active: false,
    type: 'link',
  },
  {
    id: 'conflicts',
    label: 'Conflicts',
    // triangle-alert icon
    icon: 'M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0zM12 9v4 M12 17h.01',
    active: false,
    type: 'link',
  },
  {
    id: 'settings',
    label: 'Settings',
    // gear icon (inner circle + cog outline as one path)
    icon: 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z',
    active: false,
    type: 'link',
  },
];

class LayoutStore {
  state = $state<LayoutState>({ ...initialLayoutState });
  navigationItems = $state<NavigationItem[]>(initialNavigationItems);

  // Track initialization state to prevent overwriting loaded state
  #isInitialized = false;

  /**
   * Persist the current layout state. LayoutPersistenceService.save() handles
   * debouncing internally. Only persists after initialization to avoid
   * overwriting loaded state during startup.
   */
  #persist(): void {
    if (this.#isInitialized) {
      LayoutPersistenceService.save(this.state);
    }
  }

  /**
   * Load persisted layout state from storage.
   * Should be called once on application startup.
   * Idempotent - safe to call multiple times (subsequent calls are no-ops).
   * @returns True if state was loaded successfully, false if no saved state exists or loading failed
   */
  loadPersistedLayoutState(): boolean {
    // Guard against multiple initializations (e.g., component remounting)
    if (this.#isInitialized) {
      log.warn('loadPersistedLayoutState called after initialization, ignoring');
      return false;
    }

    const persisted = LayoutPersistenceService.load();

    if (persisted) {
      this.state = {
        sidebarCollapsed: persisted.sidebarCollapsed,
        activePane: 'today', // Keep activePane at default for now (not persisted)
        collectionsExpanded: persisted.collectionsExpanded ?? false,
        schemaTypesExpanded: persisted.schemaTypesExpanded ?? false,
        aiChatsExpanded: persisted.aiChatsExpanded ?? false,
      };
    }

    // Enable persistence after load attempt (whether successful or not)
    this.#isInitialized = true;

    return !!persisted;
  }

  toggleSidebar(): void {
    this.state = { ...this.state, sidebarCollapsed: !this.state.sidebarCollapsed };
    this.#persist();
  }

  setActivePane(paneId: string): void {
    this.state = { ...this.state, activePane: paneId };
    this.#persist();
  }

  setCollectionsExpanded(expanded: boolean): void {
    this.state = { ...this.state, collectionsExpanded: expanded };
    this.#persist();
  }

  toggleCollectionsExpanded(): void {
    this.state = { ...this.state, collectionsExpanded: !this.state.collectionsExpanded };
    this.#persist();
  }

  setSchemaTypesExpanded(expanded: boolean): void {
    this.state = { ...this.state, schemaTypesExpanded: expanded };
    this.#persist();
  }

  setAiChatsExpanded(expanded: boolean): void {
    this.state = { ...this.state, aiChatsExpanded: expanded };
    this.#persist();
  }

  /** Mark a single navigation item active (and all others inactive). */
  setActiveNavItem(itemId: string): void {
    this.navigationItems = this.navigationItems.map((item) => ({
      ...item,
      active: item.id === itemId,
    }));
  }
}

export const layoutStore = new LayoutStore();

// Thin delegators keep existing callers working unchanged.
export const loadPersistedLayoutState = (): boolean => layoutStore.loadPersistedLayoutState();
export const toggleSidebar = (): void => layoutStore.toggleSidebar();
export const setActivePane = (paneId: string): void => layoutStore.setActivePane(paneId);
export const setCollectionsExpanded = (expanded: boolean): void =>
  layoutStore.setCollectionsExpanded(expanded);
export const toggleCollectionsExpanded = (): void => layoutStore.toggleCollectionsExpanded();
export const setSchemaTypesExpanded = (expanded: boolean): void =>
  layoutStore.setSchemaTypesExpanded(expanded);
export const setAiChatsExpanded = (expanded: boolean): void =>
  layoutStore.setAiChatsExpanded(expanded);
