/**
 * Unit tests for layout store - sidebar state and navigation management
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { layoutStore, toggleSidebar, setActivePane, type NavigationItem } from '$lib/stores/layout.svelte';
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

    // Reset the layout state to initial state
    layoutStore.state = {
      sidebarCollapsed: false,
      activePane: 'today',
      collectionsExpanded: false,
      schemaTypesExpanded: false,
      aiChatsExpanded: false
    };
  });

  describe('Initial State', () => {
    it('has correct initial layout state', () => {
      const state = layoutStore.state;

      expect(state.sidebarCollapsed).toBe(false);
      expect(state.activePane).toBe('today');
    });

    it('has correct initial navigation items', () => {
      const items = layoutStore.navigationItems;

      // Note: Collections section is rendered separately in NavigationSidebar, not in this store
      // Items: daily-journal, search, favorites, conflicts, settings
      // (agent-sessions removed per ADR-034 — PTY is provider mode 2d of the ai-chat node;
      //  "AI Chat" item temporarily removed pending its rework into an expandable list of
      //  recent ai-chat nodes; "conflicts" added for the ADR-068 Conflicts view)
      expect(items).toHaveLength(5);
      expect(items[0].id).toBe('daily-journal');
      expect(items[0].active).toBe(false); // No default active state - nav items just navigate
      expect(items[0].type).toBe('link');
      expect(items[items.length - 1].id).toBe('settings');
    });

    it('navigation items have required properties', () => {
      const items = layoutStore.navigationItems;

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
      const items = layoutStore.navigationItems;
      const activeItems = items.filter((item) => item.active);

      expect(activeItems).toHaveLength(0);
    });

    it('all navigation items are of type link initially', () => {
      const items = layoutStore.navigationItems;

      items.forEach((item) => {
        expect(item.type).toBe('link');
      });
    });
  });

  describe('toggleSidebar', () => {
    it('toggles sidebar from collapsed to expanded', () => {
      // Start with collapsed state
      layoutStore.state = {
        sidebarCollapsed: true,
        activePane: 'today',
        collectionsExpanded: false,
        schemaTypesExpanded: false,
        aiChatsExpanded: false
      };

      toggleSidebar();

      expect(layoutStore.state.sidebarCollapsed).toBe(false);
    });

    it('toggles sidebar from expanded to collapsed', () => {
      layoutStore.state = {
        sidebarCollapsed: false,
        activePane: 'today',
        collectionsExpanded: false,
        schemaTypesExpanded: false,
        aiChatsExpanded: false
      };

      toggleSidebar();

      expect(layoutStore.state.sidebarCollapsed).toBe(true);
    });

    it('preserves activePane when toggling', () => {
      layoutStore.state = {
        sidebarCollapsed: false,
        activePane: 'custom-pane',
        collectionsExpanded: false,
        schemaTypesExpanded: false,
        aiChatsExpanded: false
      };

      toggleSidebar();

      expect(layoutStore.state.activePane).toBe('custom-pane');
    });

    it('can be toggled multiple times', () => {
      const initialCollapsed = layoutStore.state.sidebarCollapsed;

      toggleSidebar();
      expect(layoutStore.state.sidebarCollapsed).toBe(!initialCollapsed);

      toggleSidebar();
      expect(layoutStore.state.sidebarCollapsed).toBe(initialCollapsed);

      toggleSidebar();
      expect(layoutStore.state.sidebarCollapsed).toBe(!initialCollapsed);
    });
  });

  describe('setActivePane', () => {
    it('sets active pane to new value', () => {
      setActivePane('dashboard');

      expect(layoutStore.state.activePane).toBe('dashboard');
    });

    it('preserves sidebarCollapsed when setting active pane', () => {
      layoutStore.state = {
        sidebarCollapsed: true,
        activePane: 'today',
        collectionsExpanded: false,
        schemaTypesExpanded: false,
        aiChatsExpanded: false
      };

      setActivePane('search');

      expect(layoutStore.state.sidebarCollapsed).toBe(true);
      expect(layoutStore.state.activePane).toBe('search');
    });

    it('can set active pane to empty string', () => {
      setActivePane('');

      expect(layoutStore.state.activePane).toBe('');
    });

    it('can set active pane multiple times', () => {
      setActivePane('dashboard');
      expect(layoutStore.state.activePane).toBe('dashboard');

      setActivePane('search');
      expect(layoutStore.state.activePane).toBe('search');

      setActivePane('favorites');
      expect(layoutStore.state.activePane).toBe('favorites');
    });

    it('accepts any string value for pane ID', () => {
      const customPaneIds = ['custom-pane-1', 'node-123', 'special-view', '42'];

      customPaneIds.forEach((paneId) => {
        setActivePane(paneId);
        expect(layoutStore.state.activePane).toBe(paneId);
      });
    });
  });

  describe('Navigation Items', () => {
    it('can replace navigation items', () => {
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

      layoutStore.navigationItems = newItems;

      const items = layoutStore.navigationItems;
      expect(items).toEqual(newItems);
      expect(items).toHaveLength(2);
    });

    it('setActiveNavItem marks a single item active', () => {
      layoutStore.navigationItems = [
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

      layoutStore.setActiveNavItem('dashboard');

      const items = layoutStore.navigationItems;
      expect(items.find((item) => item.id === 'daily-journal')?.active).toBe(false);
      expect(items.find((item) => item.id === 'dashboard')?.active).toBe(true);
    });

    it('can clear all navigation items', () => {
      layoutStore.navigationItems = [];

      expect(layoutStore.navigationItems).toHaveLength(0);
    });
  });
});

describe('Layout Store - Persistence Integration', () => {
  beforeEach(async () => {
    // Clear all mocks before each test
    vi.clearAllMocks();

    // Reset modules to ensure fresh import with reset isInitialized flag
    await vi.resetModules();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe('loadPersistedLayoutState', () => {
    it('loads persisted state successfully', async () => {
      const persistedState = { version: 1, sidebarCollapsed: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');

      const result = loadPersistedLayoutState();

      expect(result).toBe(true);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);
      expect(layoutStore.state.sidebarCollapsed).toBe(true);
      expect(layoutStore.state.activePane).toBe('today'); // activePane not persisted
    });

    it('returns false when no persisted state exists', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout.svelte');

      const result = loadPersistedLayoutState();

      expect(result).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);
    });

    it('prevents multiple initializations', async () => {
      const persistedState = { version: 1, sidebarCollapsed: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout.svelte');

      const result1 = loadPersistedLayoutState();
      expect(result1).toBe(true);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);

      // Second call should be ignored
      const result2 = loadPersistedLayoutState();
      expect(result2).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1); // Not called again
    });

    it('enables persistence after initialization', async () => {
      const persistedState = { version: 1, sidebarCollapsed: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('does not persist changes before initialization', async () => {
      const { toggleSidebar } = await import('$lib/stores/layout.svelte');

      // Make changes before initialization
      toggleSidebar();

      // Should NOT have called save
      expect(LayoutPersistenceService.save).not.toHaveBeenCalled();
    });

    it('persists state after initialization with no saved state', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout.svelte');

      const result = loadPersistedLayoutState();
      expect(result).toBe(false);

      vi.clearAllMocks();

      toggleSidebar();

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('preserves default activePane when loading state', async () => {
      const persistedState = { version: 1, sidebarCollapsed: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();

      expect(layoutStore.state.activePane).toBe('today');
    });

    it('keeps sidebarCollapsed false when no state loaded', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();

      expect(layoutStore.state.sidebarCollapsed).toBe(false);
    });
  });

  describe('Automatic Persistence on State Changes', () => {
    it('persists state when sidebar is toggled after init', async () => {
      const persistedState = { version: 1, sidebarCollapsed: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar } = await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
      expect(LayoutPersistenceService.save).toHaveBeenCalledWith(
        expect.objectContaining({
          sidebarCollapsed: true,
          activePane: 'today'
        })
      );
    });

    it('persists state when active pane is changed after init', async () => {
      const persistedState = { version: 1, sidebarCollapsed: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, setActivePane } = await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      setActivePane('dashboard');

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
      expect(LayoutPersistenceService.save).toHaveBeenCalledWith(
        expect.objectContaining({
          sidebarCollapsed: false,
          activePane: 'dashboard'
        })
      );
    });

    it('persists correct state on multiple changes', async () => {
      const persistedState = { version: 1, sidebarCollapsed: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar, setActivePane } =
        await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();
      setActivePane('search');
      toggleSidebar();

      // Should have been called for each change
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

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');

      const result = loadPersistedLayoutState();

      expect(result).toBe(false);
      expect(layoutStore.state.sidebarCollapsed).toBe(false);
      expect(layoutStore.state.activePane).toBe('today');
    });

    it('state changes work correctly after failed initialization', async () => {
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(null);

      const { loadPersistedLayoutState, toggleSidebar, layoutStore } =
        await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      toggleSidebar();

      expect(layoutStore.state.sidebarCollapsed).toBe(true);
      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('concurrent state changes are handled correctly', async () => {
      const persistedState = { version: 1, sidebarCollapsed: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, toggleSidebar, setActivePane, layoutStore } =
        await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      // Make multiple changes without waiting
      toggleSidebar();
      setActivePane('dashboard');
      toggleSidebar();

      expect(layoutStore.state.sidebarCollapsed).toBe(false); // Toggled twice
      expect(layoutStore.state.activePane).toBe('dashboard');

      expect(LayoutPersistenceService.save).toHaveBeenCalled();
    });

    it('initialization is idempotent with same result', async () => {
      const persistedState = { version: 1, sidebarCollapsed: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState } = await import('$lib/stores/layout.svelte');

      const result1 = loadPersistedLayoutState();
      const result2 = loadPersistedLayoutState();
      const result3 = loadPersistedLayoutState();

      expect(result1).toBe(true);
      expect(result2).toBe(false);
      expect(result3).toBe(false);
      expect(LayoutPersistenceService.load).toHaveBeenCalledTimes(1);
    });
  });

  describe('setCollectionsExpanded', () => {
    it('should set collectionsExpanded to true', async () => {
      const { setCollectionsExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setCollectionsExpanded(true);
      expect(layoutStore.state.collectionsExpanded).toBe(true);
    });

    it('should set collectionsExpanded to false', async () => {
      const { setCollectionsExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setCollectionsExpanded(false);
      expect(layoutStore.state.collectionsExpanded).toBe(false);
    });
  });

  describe('toggleCollectionsExpanded', () => {
    it('should toggle collectionsExpanded state', async () => {
      const { setCollectionsExpanded, toggleCollectionsExpanded, layoutStore } =
        await import('$lib/stores/layout.svelte');
      setCollectionsExpanded(false);
      toggleCollectionsExpanded();
      expect(layoutStore.state.collectionsExpanded).toBe(true);

      toggleCollectionsExpanded();
      expect(layoutStore.state.collectionsExpanded).toBe(false);
    });
  });

  describe('setSchemaTypesExpanded', () => {
    it('should set schemaTypesExpanded to true', async () => {
      const { setSchemaTypesExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setSchemaTypesExpanded(true);
      expect(layoutStore.state.schemaTypesExpanded).toBe(true);
    });

    it('should set schemaTypesExpanded to false', async () => {
      const { setSchemaTypesExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setSchemaTypesExpanded(false);
      expect(layoutStore.state.schemaTypesExpanded).toBe(false);
    });
  });

  describe('setAiChatsExpanded', () => {
    it('should set aiChatsExpanded to true', async () => {
      const { setAiChatsExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setAiChatsExpanded(true);
      expect(layoutStore.state.aiChatsExpanded).toBe(true);
    });

    it('should set aiChatsExpanded to false', async () => {
      const { setAiChatsExpanded, layoutStore } = await import('$lib/stores/layout.svelte');
      setAiChatsExpanded(false);
      expect(layoutStore.state.aiChatsExpanded).toBe(false);
    });

    it('persists aiChatsExpanded after initialization', async () => {
      const persistedState = { version: 4, sidebarCollapsed: false, aiChatsExpanded: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, setAiChatsExpanded } =
        await import('$lib/stores/layout.svelte');

      loadPersistedLayoutState();
      vi.clearAllMocks();

      setAiChatsExpanded(true);

      expect(LayoutPersistenceService.save).toHaveBeenCalledWith(
        expect.objectContaining({ aiChatsExpanded: true })
      );
    });
  });

  describe('aiChatsExpanded hydration from persisted state', () => {
    it('round-trips a persisted true value', async () => {
      const persistedState = { version: 4, sidebarCollapsed: false, aiChatsExpanded: true };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');
      loadPersistedLayoutState();

      expect(layoutStore.state.aiChatsExpanded).toBe(true);
    });

    it('defaults to false when the persisted state predates the field', async () => {
      // Simulates a pre-migration record with no aiChatsExpanded key at all.
      const persistedState = { version: 1, sidebarCollapsed: false };
      vi.mocked(LayoutPersistenceService.load).mockReturnValue(persistedState);

      const { loadPersistedLayoutState, layoutStore } = await import('$lib/stores/layout.svelte');
      loadPersistedLayoutState();

      expect(layoutStore.state.aiChatsExpanded).toBe(false);
    });
  });
});
