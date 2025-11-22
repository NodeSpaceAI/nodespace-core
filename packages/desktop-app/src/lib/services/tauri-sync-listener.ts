/**
 * Tauri LIVE SELECT Event Listener
 *
 * Listens for real-time synchronization events emitted from the Rust backend
 * via SurrealDB LIVE SELECT subscriptions.
 *
 * Note: This module provides debug logging for sync events. The actual event
 * handling is done directly by:
 * - ReactiveNodeData (node events)
 * - ReactiveStructureTree (hierarchy events)
 *
 * These stores subscribe to Tauri events directly rather than through an
 * intermediate event bus layer.
 */

import { listen } from '@tauri-apps/api/event';
import type { NodeEventData, HierarchyRelationship } from '$lib/types/event-types';

/**
 * Initialize Tauri real-time synchronization event listeners
 *
 * Sets up listeners for logging/debugging sync events.
 * Should be called once during app initialization.
 *
 * @returns Promise resolving when all listeners are registered
 */
export async function initializeTauriSyncListeners(): Promise<void> {
  if (!isRunningInTauri()) {
    console.debug('Not running in Tauri environment, skipping sync listener initialization');
    return;
  }

  console.info('Initializing Tauri real-time sync listeners');

  try {
    // Listen for node events (debug logging)
    await listen<NodeEventData>('node:created', (event) => {
      console.debug(`[TauriSync] Node created: ${event.payload.id}`);
    });

    await listen<NodeEventData>('node:updated', (event) => {
      console.debug(`[TauriSync] Node updated: ${event.payload.id} (v${event.payload.version})`);
    });

    await listen<{ id: string }>('node:deleted', (event) => {
      console.debug(`[TauriSync] Node deleted: ${event.payload.id}`);
    });

    // Listen for hierarchy events (debug logging)
    // Note: Backend still emits 'edge:*' events - these will be renamed to 'hierarchy:*' in a future refactor
    await listen<HierarchyRelationship>('edge:created', (event) => {
      console.debug(`[TauriSync] Hierarchy relationship created: ${event.payload.parentId} -> ${event.payload.childId}`);
    });

    await listen<HierarchyRelationship>('edge:updated', (event) => {
      console.debug(`[TauriSync] Hierarchy relationship updated: ${event.payload.parentId} -> ${event.payload.childId}`);
    });

    await listen<{ id: string }>('edge:deleted', (event) => {
      console.debug(`[TauriSync] Hierarchy relationship deleted: ${event.payload.id}`);
    });

    // Listen for synchronization errors
    await listen<Record<string, unknown>>('sync:error', (event) => {
      const message = String(event.payload.message);
      const errorType = String(event.payload.errorType);
      console.error(`[TauriSync] Sync error (${errorType}): ${message}`);
    });

    // Listen for synchronization status changes
    await listen<Record<string, unknown>>('sync:status', (event) => {
      const status = String(event.payload.status);
      const reason = event.payload.reason ? String(event.payload.reason) : '';
      console.info(`[TauriSync] Sync status: ${status}${reason ? ` (${reason})` : ''}`);
    });

    console.info('[TauriSync] Real-time sync listeners initialized successfully');
  } catch (error) {
    console.error('[TauriSync] Failed to initialize sync listeners', error);
    throw new Error(`Failed to initialize sync listeners: ${error}`);
  }
}

/**
 * Check if running in Tauri environment
 */
function isRunningInTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI__' in window;
}
