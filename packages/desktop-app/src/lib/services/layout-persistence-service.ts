/**
 * Layout Persistence Service
 *
 * Manages persistence of layout state (sidebar collapsed/expanded) to localStorage.
 * Handles loading and saving of layout state across application restarts.
 */

import type { LayoutState } from '$lib/stores/layout';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('LayoutPersistence');

/**
 * Persisted layout state structure with versioning for future migrations
 */
export interface PersistedLayoutState {
  version: number;
  sidebarCollapsed: boolean;
}

/**
 * Service for persisting and loading layout state
 */
export class LayoutPersistenceService {
  private static readonly STORAGE_KEY = 'nodespace:layout-state';
  private static readonly DEBOUNCE_MS = 300;

  private static saveTimer: ReturnType<typeof setTimeout> | null = null;
  private static pendingState: LayoutState | null = null;

  /**
   * Save layout state to persistent storage (debounced)
   * @param state - The current layout state to save
   */
  static save(state: LayoutState): void {
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
   * Save layout state immediately without debouncing
   * @param state - The current layout state to save
   */
  private static saveImmediate(state: LayoutState): void {
    try {
      const persisted: PersistedLayoutState = {
        version: 1,
        sidebarCollapsed: state.sidebarCollapsed
      };

      localStorage.setItem(this.STORAGE_KEY, JSON.stringify(persisted));
      log.debug(`State saved: sidebarCollapsed=${state.sidebarCollapsed}`);
    } catch (error) {
      log.error('Failed to save state:', error);

      // Clear pending state and timer to prevent retry with potentially stale data
      this.pendingState = null;
      if (this.saveTimer) {
        clearTimeout(this.saveTimer);
        this.saveTimer = null;
      }
    }
  }

  /**
   * Load persisted layout state from storage
   * @returns The loaded layout state or null if no valid state exists
   */
  static load(): PersistedLayoutState | null {
    try {
      const stored = localStorage.getItem(this.STORAGE_KEY);

      if (!stored) {
        log.debug('No saved state found');
        return null;
      }

      const parsed = JSON.parse(stored) as PersistedLayoutState;

      // Validate the structure
      if (!this.isValidState(parsed)) {
        log.warn('Invalid state structure, ignoring');
        return null;
      }

      // Handle version migrations
      const migrated = this.migrate(parsed);

      log.debug(`State loaded: sidebarCollapsed=${migrated.sidebarCollapsed}`);
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
  private static isValidState(state: unknown): state is PersistedLayoutState {
    if (!state || typeof state !== 'object') {
      return false;
    }

    const s = state as Record<string, unknown>;

    // Basic structure validation
    if (typeof s.version !== 'number' || typeof s.sidebarCollapsed !== 'boolean') {
      return false;
    }

    return true;
  }

  /**
   * Migrate state from older versions to current version
   * @param state - The state to migrate
   * @returns The migrated state
   */
  private static migrate(state: PersistedLayoutState): PersistedLayoutState {
    // Currently only version 1 exists, so no migration needed
    // Future versions can add migration logic here
    return state;
  }

  /**
   * Clear all persisted layout state
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
   * @param state - The current layout state to save
   */
  static saveNow(state: LayoutState): void {
    this.saveImmediate(state);
  }
}
