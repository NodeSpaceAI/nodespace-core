/**
 * Tab Persistence Service
 *
 * Manages persistence of tab state to localStorage/Tauri store.
 * Handles loading, saving, and migration of tab state across application restarts.
 */

import type { TabState } from '$lib/stores/navigation';
import { formatDateTitle, parseDateString } from '$lib/utils/date-formatting';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('TabPersistence');

/**
 * Persisted tab state structure with versioning for future migrations
 */
export interface PersistedTabState {
  version: number;
  tabs: TabState['tabs'];
  panes: TabState['panes'];
  activePaneId: string;
  activeTabIds: Record<string, string>;
}

/**
 * Service for persisting and loading tab state
 */
export class TabPersistenceService {
  private static readonly STORAGE_KEY = 'nodespace:tab-state';
  private static readonly DEBOUNCE_MS = 500;

  private static saveTimer: ReturnType<typeof setTimeout> | null = null;
  private static pendingState: TabState | null = null;

  /**
   * Save tab state to persistent storage (debounced)
   * @param state - The current tab state to save
   */
  static save(state: TabState): void {
    // Clear existing timer
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
    }

    // Store pending state for flush()
    this.pendingState = state;

    // Debounce the save operation
    this.saveTimer = setTimeout(() => {
      this.saveImmediate(state);
      this.pendingState = null;
    }, this.DEBOUNCE_MS);
  }

  /**
   * Save tab state immediately without debouncing
   * @param state - The current tab state to save
   */
  private static saveImmediate(state: TabState): void {
    try {
      const persisted: PersistedTabState = {
        version: 1,
        tabs: state.tabs,
        panes: state.panes,
        activePaneId: state.activePaneId,
        activeTabIds: state.activeTabIds
      };

      // For now, use localStorage (Tauri store integration can be added later)
      localStorage.setItem(this.STORAGE_KEY, JSON.stringify(persisted));

      log.debug('State saved successfully');
    } catch (error) {
      log.error('Failed to save state:', error);
    }
  }

  /**
   * Load persisted tab state from storage
   * @returns The loaded tab state or null if no valid state exists
   */
  static load(): PersistedTabState | null {
    try {
      const stored = localStorage.getItem(this.STORAGE_KEY);

      if (!stored) {
        log.debug('No saved state found');
        return null;
      }

      const parsed = JSON.parse(stored) as PersistedTabState;

      // Validate the structure
      if (!this.isValidState(parsed)) {
        log.warn('Invalid state structure, ignoring');
        return null;
      }

      // Handle version migrations
      const migrated = this.migrate(parsed);

      log.debug('State loaded successfully');
      return migrated;
    } catch (error) {
      log.error('Failed to load state:', error);
      return null;
    }
  }

  /**
   * Validate that the loaded state has the expected structure
   * @param state - The state to validate
   * @returns True if the state is valid
   */
  private static isValidState(state: unknown): state is PersistedTabState {
    if (!state || typeof state !== 'object') {
      return false;
    }

    const s = state as Record<string, unknown>;

    // Basic structure validation
    if (
      typeof s.version !== 'number' ||
      !Array.isArray(s.tabs) ||
      !Array.isArray(s.panes) ||
      typeof s.activePaneId !== 'string' ||
      typeof s.activeTabIds !== 'object' ||
      s.activeTabIds === null
    ) {
      return false;
    }

    // Validate tab structure
    const tabs = s.tabs as unknown[];
    if (
      !tabs.every((tab) => {
        if (!tab || typeof tab !== 'object') return false;
        const t = tab as Record<string, unknown>;
        return (
          typeof t.id === 'string' &&
          typeof t.title === 'string' &&
          (t.type === 'node' || t.type === 'placeholder') &&
          typeof t.closeable === 'boolean' &&
          typeof t.paneId === 'string'
        );
      })
    ) {
      return false;
    }

    // Validate pane structure
    const panes = s.panes as unknown[];
    if (
      !panes.every((pane) => {
        if (!pane || typeof pane !== 'object') return false;
        const p = pane as Record<string, unknown>;
        return (
          typeof p.id === 'string' &&
          typeof p.width === 'number' &&
          Array.isArray(p.tabIds) &&
          (p.tabIds as unknown[]).every((id) => typeof id === 'string')
        );
      })
    ) {
      return false;
    }

    // Validate activeTabIds map
    const activeTabIds = s.activeTabIds as Record<string, unknown>;
    if (!Object.values(activeTabIds).every((id) => typeof id === 'string')) {
      return false;
    }

    return true;
  }

  /**
   * Sanitize state to remove duplicates and invalid references
   * @param state - The state to sanitize
   * @returns The sanitized state (new object, immutable)
   *
   * @remarks
   * This function performs defensive data sanitization for state loaded from
   * external storage (localStorage/Tauri). It creates new objects at each level
   * to ensure immutability and removes duplicate IDs that may have been introduced
   * by external factors or bugs in older versions.
   */
  private static sanitize(state: PersistedTabState): PersistedTabState {
    // Remove duplicate pane IDs
    const seenPaneIds = new Set<string>();
    const uniquePanes = state.panes.filter((pane) => {
      if (seenPaneIds.has(pane.id)) {
        log.error(`Removing duplicate pane ID: ${pane.id}`);
        return false;
      }
      seenPaneIds.add(pane.id);
      return true;
    });

    // Remove duplicate tab IDs from each pane's tabIds array
    // Explicitly create new objects to ensure immutability
    const sanitizedPanes = uniquePanes.map((pane) => {
      const uniqueTabIds = [...new Set(pane.tabIds)];

      if (uniqueTabIds.length !== pane.tabIds.length) {
        const duplicateCount = pane.tabIds.length - uniqueTabIds.length;
        log.warn(
          `Removed ${duplicateCount} duplicate tab ID(s) from pane ${pane.id}'s tabIds array`
        );
      }

      return {
        ...pane,
        tabIds: uniqueTabIds
      };
    });

    // Remove duplicate tab IDs from tabs array
    const seenTabIds = new Set<string>();
    const uniqueTabs = state.tabs.filter((tab) => {
      if (seenTabIds.has(tab.id)) {
        log.error(`Removing duplicate tab ID: ${tab.id}`);
        return false;
      }
      seenTabIds.add(tab.id);
      return true;
    });

    return {
      ...state,
      panes: sanitizedPanes,
      tabs: uniqueTabs
    };
  }

  /**
   * Migrate state from older versions to current version
   * @param state - The state to migrate
   * @returns The migrated state
   */
  private static migrate(state: PersistedTabState): PersistedTabState {
    // Sanitize state to remove duplicates (applies to all versions)
    const sanitized = this.sanitize(state);

    // Recompute titles for date nodes (they should never be persisted as static values)
    // Date titles like "Today/Tomorrow/Yesterday" are dynamic and must be recomputed on load
    const fixedTabs = sanitized.tabs.map(tab => {
      // For date nodes, always recompute title from nodeId
      if (tab.content?.nodeType === 'date' && tab.content?.nodeId) {
        const date = parseDateString(tab.content.nodeId);
        if (date) {
          return { ...tab, title: formatDateTitle(date) };
        }
      }
      return tab;
    });

    return { ...sanitized, tabs: fixedTabs };
  }

  /**
   * Clear all persisted tab state
   * Useful for testing or resetting to default state
   */
  static clear(): void {
    try {
      localStorage.removeItem(this.STORAGE_KEY);
      log.debug('State cleared');
    } catch (error) {
      log.error('Failed to clear state:', error);
    }
  }

  /**
   * Flush any pending saves immediately
   * Useful for testing or before app shutdown
   */
  static flush(): void {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }

    // If there's a pending state, save it immediately
    if (this.pendingState) {
      this.saveImmediate(this.pendingState);
      this.pendingState = null;
    }
  }

  /**
   * Force immediate save without debouncing
   * Exposed for cases where debouncing should be bypassed
   * @param state - The current tab state to save
   */
  static saveNow(state: TabState): void {
    this.saveImmediate(state);
  }
}
