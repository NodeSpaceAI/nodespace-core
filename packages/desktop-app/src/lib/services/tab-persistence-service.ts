/**
 * Tab Persistence Service
 *
 * Manages persistence of tab state to localStorage/Tauri store.
 * Handles loading, saving, and migration of tab state across application restarts.
 */

import type { TabState } from '$lib/stores/navigation';

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
  private static readonly LOG_PREFIX = '[TabPersistence]';
  private static readonly STORAGE_KEY = 'nodespace:tab-state';
  private static readonly DEBOUNCE_MS = 500;
  private static readonly DEBUG = import.meta.env.DEV;

  private static saveTimer: ReturnType<typeof setTimeout> | null = null;
  private static pendingState: TabState | null = null;

  /**
   * Log message in development mode only
   */
  private static log(message: string): void {
    if (this.DEBUG) {
      console.log(`${this.LOG_PREFIX} ${message}`);
    }
  }

  /**
   * Log warning in all environments
   */
  private static warn(message: string): void {
    console.warn(`${this.LOG_PREFIX} ${message}`);
  }

  /**
   * Log error in all environments
   */
  private static error(message: string, error?: unknown): void {
    console.error(`${this.LOG_PREFIX} ${message}`, error || '');
  }

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

      this.log('State saved successfully');
    } catch (error) {
      this.error('Failed to save state:', error);
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
        this.log('No saved state found');
        return null;
      }

      const parsed = JSON.parse(stored) as PersistedTabState;

      // Validate the structure
      if (!this.isValidState(parsed)) {
        this.warn('Invalid state structure, ignoring');
        return null;
      }

      // Handle version migrations
      const migrated = this.migrate(parsed);

      this.log('State loaded successfully');
      return migrated;
    } catch (error) {
      this.error('Failed to load state:', error);
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
   * Migrate state from older versions to current version
   * @param state - The state to migrate
   * @returns The migrated state
   */
  private static migrate(state: PersistedTabState): PersistedTabState {
    // Currently only version 1 exists, so no migration needed
    // Future versions can add migration logic here:
    //
    // if (state.version === 1) {
    //   // Migrate from v1 to v2
    //   return { ...state, version: 2, newField: defaultValue };
    // }

    return state;
  }

  /**
   * Clear all persisted tab state
   * Useful for testing or resetting to default state
   */
  static clear(): void {
    try {
      localStorage.removeItem(this.STORAGE_KEY);
      this.log('State cleared');
    } catch (error) {
      this.error('Failed to clear state:', error);
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
}
