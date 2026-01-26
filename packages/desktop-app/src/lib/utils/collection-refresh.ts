/**
 * Collection Refresh Utility
 *
 * Shared debounced collection refresh logic used by both Tauri and browser
 * sync services to update the collections sidebar when member_of relationships change.
 *
 * Issue #832: Extracted from browser-sync-service.ts and tauri-sync-listener.ts
 * to follow DRY principle.
 */

import { get } from 'svelte/store';
import { collectionsData, collectionsState } from '$lib/stores/collections';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('CollectionRefresh');

// Debounce timer for collection refreshes during bulk operations
let collectionRefreshTimer: ReturnType<typeof setTimeout> | null = null;
const COLLECTION_REFRESH_DEBOUNCE_MS = 300;

/**
 * Debounced refresh of collections sidebar
 *
 * When member_of relationships change (especially during bulk imports),
 * we debounce the refresh to avoid excessive API calls.
 *
 * @param affectedCollectionId - Optional collection ID that was affected (for member refresh)
 */
export function scheduleCollectionRefresh(affectedCollectionId?: string): void {
  if (collectionRefreshTimer) {
    clearTimeout(collectionRefreshTimer);
  }

  collectionRefreshTimer = setTimeout(async () => {
    collectionRefreshTimer = null;
    log.debug('Refreshing collections after change');

    // Reload all collections (updates sidebar)
    await collectionsData.loadCollections();

    // If the affected collection is currently selected, also refresh its members
    if (affectedCollectionId) {
      const state = get(collectionsState);
      if (state.selectedCollectionId === affectedCollectionId) {
        log.debug('Refreshing members for selected collection', affectedCollectionId);
        await collectionsData.loadMembers(affectedCollectionId);
      }
    }
  }, COLLECTION_REFRESH_DEBOUNCE_MS);
}

/**
 * Clear any pending collection refresh
 *
 * Useful for cleanup during service destruction.
 */
export function clearCollectionRefreshTimer(): void {
  if (collectionRefreshTimer) {
    clearTimeout(collectionRefreshTimer);
    collectionRefreshTimer = null;
  }
}
