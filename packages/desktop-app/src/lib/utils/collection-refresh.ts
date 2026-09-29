/**
 * Collection Refresh Utility
 *
 * Shared debounced collection refresh logic used by both Tauri and browser
 * sync services to update the collections sidebar when member_of relationships change.
 *
 * Extracted from browser-sync-service.ts and tauri-sync-listener.ts
 * to follow DRY principle.
 */

import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { schemasData } from '$lib/stores/schemas.svelte';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import { savedQueriesData } from '$lib/stores/saved-queries.svelte';
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
      const state = collectionsState.state;
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

// Debounce timer for schema refreshes
let schemaRefreshTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Debounced refresh of the node types sidebar.
 *
 * Called when a schema node is created or deleted externally (e.g. via MCP).
 */
export function scheduleSchemaRefresh(): void {
  if (schemaRefreshTimer) {
    clearTimeout(schemaRefreshTimer);
  }

  schemaRefreshTimer = setTimeout(async () => {
    schemaRefreshTimer = null;
    log.debug('Refreshing schemas after change');
    await schemasData.loadSchemas();
  }, COLLECTION_REFRESH_DEBOUNCE_MS);
}

/**
 * Clear any pending schema refresh timer.
 */
export function clearSchemaRefreshTimer(): void {
  if (schemaRefreshTimer) {
    clearTimeout(schemaRefreshTimer);
    schemaRefreshTimer = null;
  }
}

// Debounce timer for AI chats refreshes
let aiChatRefreshTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Debounced refresh of the AI Chats sidebar list.
 *
 * Called when an ai-chat node is created or updated externally (e.g. via MCP,
 * an agent tool call, or background titling filling in a chat's title) — the
 * sidebar's list items are detached snapshots (`AiChatListItem`), not live
 * `sharedNodeStore` reads, so without this the list never picks up an
 * externally-created chat or a title change until the next mount, daemon
 * reconnect, or database switch.
 */
export function scheduleAiChatRefresh(): void {
  if (aiChatRefreshTimer) {
    clearTimeout(aiChatRefreshTimer);
  }

  aiChatRefreshTimer = setTimeout(async () => {
    aiChatRefreshTimer = null;
    log.debug('Refreshing AI chats after change');
    await aiChatsData.loadAiChats();
  }, COLLECTION_REFRESH_DEBOUNCE_MS);
}

/**
 * Clear any pending AI chats refresh timer.
 */
export function clearAiChatRefreshTimer(): void {
  if (aiChatRefreshTimer) {
    clearTimeout(aiChatRefreshTimer);
    aiChatRefreshTimer = null;
  }
}

// Debounce timer for saved-query refreshes
let savedQueryRefreshTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Debounced refresh of the saved queries listed under each node type in the
 * sidebar. Called when a query node is created, updated or deleted, including
 * out-of-band writes such as a playbook install.
 */
export function scheduleSavedQueryRefresh(): void {
  if (savedQueryRefreshTimer) {
    clearTimeout(savedQueryRefreshTimer);
  }

  savedQueryRefreshTimer = setTimeout(async () => {
    savedQueryRefreshTimer = null;
    log.debug('Refreshing saved queries after change');
    await savedQueriesData.loadSavedQueries();
  }, COLLECTION_REFRESH_DEBOUNCE_MS);
}

/**
 * Clear any pending saved-query refresh timer.
 */
export function clearSavedQueryRefreshTimer(): void {
  if (savedQueryRefreshTimer) {
    clearTimeout(savedQueryRefreshTimer);
    savedQueryRefreshTimer = null;
  }
}
