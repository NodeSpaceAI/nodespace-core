/**
 * Unit tests for layout store - sidebar state and navigation management
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import {
  layoutState,
  navigationItems,
  toggleSidebar,
  setActivePane,
  type NavigationItem
} from '$lib/stores/layout';
import { LayoutPersistenceService } from '$lib/services/layout-persistence-service';

// Mock the LayoutPersistenceService
vi.mock('$lib/services/layout-persistence-service', () => ({
  LayoutPersistenceService: {
    save: vi.fn(),
    load: vi.fn(),
    clear: vi.fn(),
    flush: vi.fn(),
    saveNow: vi.fn()
  }
}));

// Mock the logger to avoid console noise in tests
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

describe('Layout Store - Layout State Management', () => {
  beforeEach(() => {
    // Clear all mocks before each test
    vi.clearAllMocks();

    // Reset the layoutState to initial state
    layoutState.set({
      sidebarCollapsed: false,
      activePane: 'today',
      collectionsExpanded: false
    });

    // Reset the module state by requiring a fresh import
    // This ensures isInitialized is reset between tests
    vi.resetModules();
  });

  describe('Initial State', () => {
    it('has correct initial layout state', () => {
      const state = get(layoutState);

      expect(state.sidebarCollapsed).toBe(false);
      expect(state.activePane).toBe('today');
    });

    it('has correct initial navigation items', () => {
      const items = get(navigationItems);

      // Note: Collections section is rendered separately in NavigationSidebar, not in this store
      // Items: daily-journal, search, favorites (Dashboard removed)
      expect(items).toHaveLength(3);
      expect(items[0].id).toBe('daily-journal');
      expect(items[0].active).toBe(false); // No default active state - nav items just navigate
      expect(items[0].type).toBe('link');
    });

    it('navigation items have required properties', () => {
      const items = get(navigationItems);

      items.forEach((item) => {
        expect(item).toHaveProperty('id');
        expect(item).toHaveProperty('label');
        expect(item).toHaveProperty('icon');
        expect(item).toHaveProperty('active');
        expect(item).toHaveProperty('type');
        expect(typeof item.id).toBe('string');
        expect(typeof item.label).toBe('string');
        expect(typeof item.icon).toBe('string');
        expect(typeof item.active).toBe('boolean');
        expect(['link', 'placeholder']).toContain(item.type);
      });
    });

    it('has no active navigation items initially', () => {
      // Navigation items don't have default active state - they just navigate to destinations
      const items = get(navigationItems);
      const activeItems = items.filter((item) => item.active);

      expect(activeItems).toHaveLength(0);
    });

    it('all navigation items are of type link initially', () => {
      const items = get(navigationItems);

      items.forEach((item) => {
        expect(item.type).toBe('link');
      });
    });
  });

  describe('toggleSidebar', () => {
    it('toggles sidebar from collapsed to expanded', () => {
      // Start with collapsed state
      layoutState.set({
        sidebarCollapsed: true,
        activePane: 'today',
        collectionsExpanded: false
      });

      toggleSidebar();

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(false);
    });

    it('toggles sidebar from expanded to collapsed', () => {
      // Start with expanded state
      layoutState.set({
        sidebarCollapsed: false,
        activePane: 'today',
        collectionsExpanded: false
      });

      toggleSidebar();

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(true);
    });

    it('preserves activePane when toggling', () => {
      layoutState.set({
        sidebarCollapsed: false,
        activePane: 'custom-pane',
        collectionsExpanded: false
      });

      toggleSidebar();

      const state = get(layoutState);
      expect(state.activePane).toBe('custom-pane');
    });

    it('can be toggled multiple times', () => {
      const initialState = get(layoutState);
      const initialCollapsed = initialState.sidebarCollapsed;

      toggleSidebar();
      expect(get(layoutState).sidebarCollapsed).toBe(!initialCollapsed);

      toggleSidebar();
      expect(get(layoutState).sidebarCollapsed).toBe(initialCollapsed);

      toggleSidebar();
      expect(get(layoutState).sidebarCollapsed).toBe(!initialCollapsed);
    });
  });

  describe('setActivePane', () => {
    it('sets active pane to new value', () => {
      setActivePane('dashboard');

      const state = get(layoutState);
      expect(state.activePane).toBe('dashboard');
    });

    it('preserves sidebarCollapsed when setting active pane', () => {
      layoutState.set({
        sidebarCollapsed: true,
        activePane: 'today',
        collectionsExpanded: false
      });

      setActivePane('search');

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(true);
      expect(state.activePane).toBe('search');
    });

    it('can set active pane to empty string', () => {
      setActivePane('');

      const state = get(layoutState);
      expect(state.activePane).toBe('');
    });

    it('can set active pane multiple times', () => {
      setActivePane('dashboard');
      expect(get(layoutState).activePane).toBe('dashboard');

      setActivePane('search');
      expect(get(layoutState).activePane).toBe('search');

      setActivePane('favorites');
      expect(get(layoutState).activePane).toBe('favorites');
    });

    it('accepts any string value for pane ID', () => {
      const customPaneIds = ['custom-pane-1', 'node-123', 'special-view', '42'];

      customPaneIds.forEach((paneId) => {
        setActivePane(paneId);
        expect(get(layoutState).activePane).toBe(paneId);
      });
    });
  });

  describe('Store Reactivity', () => {
    it('notifies subscribers when layoutState changes', () => {
      const subscriber = vi.fn();
      const unsubscribe = layoutState.subscribe(subscriber);

      // Initial call on subscribe
      expect(subscriber).toHaveBeenCalledTimes(1);

      toggleSidebar();

      // Should be called again after change
      expect(subscriber).toHaveBeenCalledTimes(2);

      unsubscribe();
    });

    it('notifies subscribers when navigationItems changes', () => {
      const subscriber = vi.fn();
      const unsubscribe = navigationItems.subscribe(subscriber);

      // Initial call on subscribe
      expect(subscriber).toHaveBeenCalledTimes(1);

      navigationItems.set([
        {
          id: 'test',
          label: 'Test',
          icon: 'test-icon',
          active: true,
          type: 'link'
        }
      ]);

      // Should be called again after change
      expect(subscriber).toHaveBeenCalledTimes(2);

      unsubscribe();
    });

    it('multiple subscribers receive updates', () => {
      const subscriber1 = vi.fn();
      const subscriber2 = vi.fn();

      const unsubscribe1 = layoutState.subscribe(subscriber1);
      const unsubscribe2 = layoutState.subscribe(subscriber2);

      toggleSidebar();

      expect(subscriber1).toHaveBeenCalledTimes(2); // Initial + update
      expect(subscriber2).toHaveBeenCalledTimes(2); // Initial + update

      unsubscribe1();
      unsubscribe2();
    });
  });

  describe('Navigation Items Store', () => {
    it('can update navigation items', () => {
      const newItems: NavigationItem[] = [
        {
          id: 'custom-1',
          label: 'Custom 1',
          icon: 'icon-1',
          active: true,
          type: 'link'
        },
        {
          id: 'custom-2',
          label: 'Custom 2',
          icon: 'icon-2',
          active: false,
          type: 'placeholder'
        }
      ];

      navigationItems.set(newItems);

      const items = get(navigationItems);
      expect(items).toEqual(newItems);
      expect(items).toHaveLength(2);
    });

    it('can update individual navigation item', () => {
      // First set the items back to initial state to ensure they exist
      const initialItems: NavigationItem[] = [
        {
          id: 'daily-journal',
          label: 'Daily Journal',
          icon: 'm3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
          active: true,
          type: 'link'
        },
        {
          id: 'dashboard',
          label: 'Dashboard',
          icon: 'M3 3h18v18H3V3zM9 15h6',
          active: false,
          type: 'link'
        }
      ];
      navigationItems.set(initialItems);

      navigationItems.update((items) => {
        return items.map((item) =>
          item.id === 'daily-journal' ? { ...item, active: false } : item
        );
      });

      const items = get(navigationItems);
      const dailyJournal = items.find((item) => item.id === 'daily-journal');

      expect(dailyJournal?.active).toBe(false);
    });

    it('can set multiple items as active', () => {
      // First set the items to a known state
      const testItems: NavigationItem[] = [
        {
          id: 'item-1',
          label: 'Item 1',
          icon: 'icon-1',
          active: false,
          type: 'link'
        },
        {
          id: 'item-2',
          label: 'Item 2',
          icon: 'icon-2',
          active: false,
          type: 'link'
        },
        {
          id: 'item-3',
          label: 'Item 3',
          icon: 'icon-3',
          active: false,
          type: 'link'
        },
        {
          id: 'item-4',
          label: 'Item 4',
          icon: 'icon-4',
          active: false,
          type: 'link'
        },
        {
          id: 'item-5',
          label: 'Item 5',
          icon: 'icon-5',
          active: false,
          type: 'link'
        }
      ];
      navigationItems.set(testItems);

      navigationItems.update((items) => {
        return items.map((item) => ({ ...item, active: true }));
      });

      const items = get(navigationItems);
      const activeItems = items.filter((item) => item.active);

      expect(activeItems).toHaveLength(5);
    });

    it('can clear all navigation items', () => {
      navigationItems.set([]);

      const items = get(navigationItems);
      expect(items).toHaveLength(0);
    });
  });
});

describe('Layout Store - Persistence Integration', () => {
  beforeEach(async () => {
    // Clear all mocks before each test
    vi.clearAllMocks();

    // Reset modules to ensure fresh import with reset isInitialized flag
    await vi.resetModules();

    // Import fresh instances after reset
    const freshModule = await import('$lib/stores/layout');

    // Reset to initial state
    freshModule.layoutState.set({
      sidebarCollapsed: false,
      activePane: 'today',
      collectionsExpanded: false
    });
  });

  afterEach(() => {
    // Clean up after tests
    vi.clearAllMocks();
  });

  describe('loadPersistedLayoutState', () => {
    it('loads persisted state successfully', async () => {
      // Mock the persistence service to return saved state
      const persistedState = {
        version: 1,
        sidebarCollapsed: true
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      // Get fresh import
      const { loadPersistedLayoutState, layoutState } = await import('$lib/stores/layout');

      const result = loadPersistedLayoutState();

      expect(result).toBe(true);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(true);
      expect(state.activePane).toBe('today'); // activePane not persisted
    });

    it('returns false when no persisted state exists', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout');

      const result = loadPersistedLayoutState();

      expect(result).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);
    });

    it('prevents multiple initializations', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: true
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout');

      // First call should load
      const result1 = loadPersistedLayoutState();
      expect(result1).toBe(true);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);

      // Second call should be ignored
      const result2 = loadPersistedLayoutState();
      expect(result2).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1); // Not called again
    });

    it('enables persistence after initialization', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: true
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout');

      // Load persisted state (initializes)
      loadPersistedLayoutState();

      // Clear the mock to track new calls
      vi.clearAllMocks();

      // Now changes should trigger persistence
      toggleSidebar();

      // Wait for subscription to fire
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('does not persist changes before initialization', async () => {
      const { toggleSidebar } = await import('$lib/stores/layout');

      // Make changes before initialization
      toggleSidebar();

      // Wait for any potential subscription
      await new Promise((resolve) => setTimeout(resolve, 0));

      // Should NOT have called save
      expect(LayoutPersistenceService.save).not.toHaveBeenCalled();
    });

    it('persists state after initialization with no saved state', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout');

      // Initialize (with no saved state)
      const result = loadPersistedLayoutState();
      expect(result).toBe(false);

      // Clear mocks after initialization
      vi.clearAllMocks();

      // Make changes after initialization
      toggleSidebar();

      // Wait for subscription to fire
      await new Promise((resolve) => setTimeout(resolve, 0));

      // Should persist changes
      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('preserves default activePane when loading state', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: true
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, layoutState } = await import('$lib/stores/layout');

      loadPersistedLayoutState();

      const state = get(layoutState);
      expect(state.activePane).toBe('today');
    });

    it('keeps sidebarCollapsed false when no state loaded', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, layoutState } = await import('$lib/stores/layout');

      loadPersistedLayoutState();

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(false);
    });
  });

  describe('Automatic Persistence on State Changes', () => {
    it('persists state when sidebar is toggled after init', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: false
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();

      // Wait for subscription
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
      expect(LayoutPersistenceService.save).toHaveBeenCalledWith(
        expect.objectContaining({
          sidebarCollapsed: true,
          activePane: 'today'
        })
      );
    });

    it('persists state when active pane is changed after init', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: false
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, setActivePane } = await import('$lib/stores/layout');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      setActivePane('dashboard');

      // Wait for subscription
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
      expect(LayoutPersistenceService.save).toHaveBeenCalledWith(
        expect.objectContaining({
          sidebarCollapsed: false,
          activePane: 'dashboard'
        })
      );
    });

    it('persists correct state on multiple changes', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: false
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar, setActivePane } =
        await import('$lib/stores/layout');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();
      await new Promise((resolve) => setTimeout(resolve, 0));

      setActivePane('search');
      await new Promise((resolve) => setTimeout(resolve, 0));

      toggleSidebar();
      await new Promise((resolve) => setTimeout(resolve, 0));

      // Should have been called multiple times
      expect(LayoutPersistenceService.save).toHaveBeenCalledTimes(3);

      // Last call should have final state
      const lastCall = vi.mocked(LayoutPersistenceService.save).mock.calls[2][0];
      expect(lastCall).toEqual(
        expect.objectContaining({
          sidebarCollapsed: false,
          activePane: 'search'
        })
      );
    });
  });

  describe('Edge Cases', () => {
    it('handles undefined persisted state', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, layoutState } = await import('$lib/stores/layout');

      const result = loadPersistedLayoutState();

      expect(result).toBe(false);
      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(false);
      expect(state.activePane).toBe('today');
    });

    it('state changes work correctly after failed initialization', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, toggleSidebar, layoutState } =
        await import('$lib/stores/layout');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(true);

      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('concurrent state changes are handled correctly', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: false
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar, setActivePane, layoutState } =
        await import('$lib/stores/layout');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      // Make multiple changes without waiting
      toggleSidebar();
      setActivePane('dashboard');
      toggleSidebar();

      await new Promise((resolve) => setTimeout(resolve, 0));

      const state = get(layoutState);
      expect(state.sidebarCollapsed).toBe(false); // Toggled twice
      expect(state.activePane).toBe('dashboard');

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('initialization is idempotent with same result', async () => {
      const persistedState = {
        version: 1,
        sidebarCollapsed: true
      };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout');

      const result1 = loadPersistedLayoutState();
      const result2 = loadPersistedLayoutState();
      const result3 = loadPersistedLayoutState();

      expect(result1).toBe(true);
      expect(result2).toBe(false);
      expect(result3).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);
    });
  });
});
