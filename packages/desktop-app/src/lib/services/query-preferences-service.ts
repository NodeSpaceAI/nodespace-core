/**
 * QueryPreferencesService
 *
 * Manages per-query view preferences (active view, view-specific config)
 * using a write-through cache backed by localStorage. Each query node has
 * independent preferences keyed by its node ID.
 *
 * Design decisions:
 * - Synchronous API: localStorage is synchronous, no async overhead needed
 * - Write-through cache: reads hit cache first, writes update both layers
 * - Defensive JSON parsing: corrupt storage data always falls back to defaults
 * - Cache invalidation: explicit `invalidateCache` for scenarios where storage
 *   may have been updated externally (e.g., other tabs via BroadcastChannel)
 */

import { createLogger } from '$lib/utils/logger';
import type { ListViewConfig, TableViewConfig, KanbanViewConfig } from '$lib/types/query';
import {
  type QueryPreferences,
  DEFAULT_QUERY_PREFERENCES
} from '$lib/types/query-preferences';

const log = createLogger('QueryPreferences');

class QueryPreferencesService {
  private cache = new Map<string, QueryPreferences>();
  private readonly STORAGE_PREFIX = 'query-prefs-';

  /**
   * Build the localStorage key for a given query node ID.
   */
  private storageKey(queryNodeId: string): string {
    return `${this.STORAGE_PREFIX}${queryNodeId}`;
  }

  /**
   * Deep-clone the default preferences to prevent mutation of the shared constant.
   */
  private defaultPreferences(): QueryPreferences {
    return {
      lastView: DEFAULT_QUERY_PREFERENCES.lastView,
      viewConfigs: {}
    };
  }

  /**
   * Validate that an unknown value conforms to the QueryPreferences shape.
   * Accepts extra/unknown fields for forward compatibility.
   */
  private isValidPreferences(value: unknown): value is QueryPreferences {
    if (!value || typeof value !== 'object') return false;
    const v = value as Record<string, unknown>;
    if (v.lastView !== 'list' && v.lastView !== 'table' && v.lastView !== 'kanban') return false;
    if (!v.viewConfigs || typeof v.viewConfigs !== 'object') return false;
    return true;
  }

  /**
   * Read preferences from localStorage for the given query node ID.
   * Returns `null` if nothing is stored or if the stored value is corrupt.
   */
  private readFromStorage(queryNodeId: string): QueryPreferences | null {
    try {
      const raw = localStorage.getItem(this.storageKey(queryNodeId));
      if (!raw) return null;

      const parsed: unknown = JSON.parse(raw);
      if (!this.isValidPreferences(parsed)) {
        log.warn('Invalid preferences in storage, discarding', { queryNodeId });
        return null;
      }
      return parsed;
    } catch (e) {
      log.warn('Failed to parse preferences from storage, using defaults', { queryNodeId, error: e });
      return null;
    }
  }

  /**
   * Write preferences to localStorage for the given query node ID.
   */
  private writeToStorage(queryNodeId: string, prefs: QueryPreferences): void {
    try {
      localStorage.setItem(this.storageKey(queryNodeId), JSON.stringify(prefs));
    } catch (e) {
      log.error('Failed to persist preferences to storage', { queryNodeId, error: e });
    }
  }

  /**
   * Get the stored preferences for a query node.
   *
   * Checks the in-memory cache first, then falls back to localStorage.
   * Returns DEFAULT_QUERY_PREFERENCES if nothing is found or if parsing fails.
   */
  getPreferences(queryNodeId: string): QueryPreferences {
    const cached = this.cache.get(queryNodeId);
    if (cached !== undefined) {
      return cached;
    }

    const stored = this.readFromStorage(queryNodeId);
    const prefs = stored ?? this.defaultPreferences();
    this.cache.set(queryNodeId, prefs);
    return prefs;
  }

  /**
   * Update the active view and optionally merge view-specific configuration.
   *
   * Persists changes to both the in-memory cache and localStorage.
   *
   * @param queryNodeId - The query node whose preferences to update
   * @param view - The view that was activated
   * @param config - Optional partial config to merge into the view's stored config
   */
  saveViewConfig(
    queryNodeId: string,
    view: 'list' | 'table' | 'kanban',
    config?: Partial<ListViewConfig | TableViewConfig | KanbanViewConfig>
  ): void {
    const current = this.getPreferences(queryNodeId);

    const updated: QueryPreferences = {
      ...current,
      lastView: view,
      viewConfigs: { ...current.viewConfigs }
    };

    if (config !== undefined) {
      // Merge the new config into any existing config for this view type.
      // Cast through unknown to satisfy the discriminated union — the caller
      // is responsible for passing config appropriate to the given view.
      const existing = (current.viewConfigs[view] ?? {}) as Record<string, unknown>;
      const incoming = config as Record<string, unknown>;
      (updated.viewConfigs as Record<string, unknown>)[view] = { ...existing, ...incoming };
    }

    this.cache.set(queryNodeId, updated);
    this.writeToStorage(queryNodeId, updated);
    log.debug('Saved view config', { queryNodeId, view });
  }

  /**
   * Remove all stored preferences for a query node.
   *
   * Clears both the in-memory cache entry and the localStorage entry.
   */
  clearPreferences(queryNodeId: string): void {
    this.cache.delete(queryNodeId);
    try {
      localStorage.removeItem(this.storageKey(queryNodeId));
    } catch (e) {
      log.error('Failed to remove preferences from storage', { queryNodeId, error: e });
    }
    log.debug('Cleared preferences', { queryNodeId });
  }

  /**
   * Evict the in-memory cache entry for a query node without touching localStorage.
   *
   * The next call to `getPreferences` will re-read from localStorage, which is
   * useful when external agents (e.g., another browser tab) may have updated the
   * stored value.
   */
  invalidateCache(queryNodeId: string): void {
    this.cache.delete(queryNodeId);
    log.debug('Cache invalidated', { queryNodeId });
  }
}

export const queryPreferencesService = new QueryPreferencesService();
