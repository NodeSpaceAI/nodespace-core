/**
 * Tauri LIVE SELECT Event Listener
 *
 * Listens for real-time synchronization events emitted from the Rust backend
 * via SurrealDB LIVE SELECT subscriptions. Converts Tauri events to frontend
 * EventBus events for consumption by UI components and services.
 *
 * This bridge connects the Tauri backend (LiveQueryService) with the frontend
 * event system, enabling real-time multi-client synchronization.
 */

import { listen } from '@tauri-apps/api/event';
import { eventBus } from './event-bus';
import type {
  RealtimeNodeCreatedEvent,
  RealtimeNodeUpdatedEvent,
  RealtimeNodeDeletedEvent,
  RealtimeEdgeCreatedEvent,
  RealtimeEdgeUpdatedEvent,
  RealtimeEdgeDeletedEvent,
  RealtimeSyncErrorEvent,
  RealtimeSyncStatusEvent,
  NodeEventData,
  EdgeEventData
} from './event-types';

/**
 * Initialize Tauri real-time synchronization event listeners
 *
 * Sets up listeners for all real-time synchronization events (polling-based MVP)
 * and bridges them to the frontend EventBus. Should be called once during app initialization.
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
    // Listen for node events
    await listen<NodeEventData>('node:created', (event) => {
      handleNodeCreatedEvent(event.payload);
    });

    await listen<NodeEventData>('node:updated', (event) => {
      handleNodeUpdatedEvent(event.payload);
    });

    await listen<{ id: string }>('node:deleted', (event) => {
      handleNodeDeletedEvent(event.payload);
    });

    // Listen for edge events
    await listen<EdgeEventData>('edge:created', (event) => {
      handleEdgeCreatedEvent(event.payload);
    });

    await listen<EdgeEventData>('edge:updated', (event) => {
      handleEdgeUpdatedEvent(event.payload);
    });

    await listen<{ id: string }>('edge:deleted', (event) => {
      handleEdgeDeletedEvent(event.payload);
    });

    // Listen for synchronization errors
    await listen<Record<string, unknown>>('sync:error', (event) => {
      handleSyncErrorEvent(event.payload);
    });

    // Listen for synchronization status changes
    await listen<Record<string, unknown>>('sync:status', (event) => {
      handleSyncStatusEvent(event.payload);
    });

    console.info('✅ Tauri real-time sync listeners initialized successfully');
  } catch (error) {
    console.error('❌ Failed to initialize Tauri sync listeners', error);
    throw new Error(`Failed to initialize sync listeners: ${error}`);
  }
}

/**
 * Handle node created events from Tauri backend
 */
function handleNodeCreatedEvent(payload: NodeEventData): void {
  try {
    const event: Omit<RealtimeNodeCreatedEvent, 'timestamp'> = {
      type: 'node:created',
      namespace: 'sync',
      source: 'tauri-polling-service',
      nodeId: payload.id,
      nodeData: payload,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`📗 Node created: ${payload.id}`);
  } catch (error) {
    console.error('Error processing node:created event', error);
  }
}

/**
 * Handle node updated events from Tauri backend
 */
function handleNodeUpdatedEvent(payload: NodeEventData): void {
  try {
    const event: Omit<RealtimeNodeUpdatedEvent, 'timestamp'> = {
      type: 'node:updated',
      namespace: 'sync',
      source: 'tauri-polling-service',
      nodeId: payload.id,
      nodeData: payload,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`📘 Node updated: ${payload.id} (v${payload.version})`);
  } catch (error) {
    console.error('Error processing node:updated event', error);
  }
}

/**
 * Handle node deleted events from Tauri backend
 */
function handleNodeDeletedEvent(payload: { id: string }): void {
  try {
    const event: Omit<RealtimeNodeDeletedEvent, 'timestamp'> = {
      type: 'node:deleted',
      namespace: 'sync',
      source: 'tauri-polling-service',
      nodeId: payload.id,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`📕 Node deleted: ${payload.id}`);
  } catch (error) {
    console.error('Error processing node:deleted event', error);
  }
}

/**
 * Handle edge created events from Tauri backend
 */
function handleEdgeCreatedEvent(payload: EdgeEventData): void {
  try {
    const event: Omit<RealtimeEdgeCreatedEvent, 'timestamp'> = {
      type: 'edge:created',
      namespace: 'sync',
      source: 'tauri-polling-service',
      edgeId: payload.id,
      edgeData: payload,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`🔗 Edge created: ${payload.in} → ${payload.out}`);
  } catch (error) {
    console.error('Error processing edge:created event', error);
  }
}

/**
 * Handle edge updated events from Tauri backend
 */
function handleEdgeUpdatedEvent(payload: EdgeEventData): void {
  try {
    const event: Omit<RealtimeEdgeUpdatedEvent, 'timestamp'> = {
      type: 'edge:updated',
      namespace: 'sync',
      source: 'tauri-polling-service',
      edgeId: payload.id,
      edgeData: payload,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`🔄 Edge updated: ${payload.in} → ${payload.out}`);
  } catch (error) {
    console.error('Error processing edge:updated event', error);
  }
}

/**
 * Handle edge deleted events from Tauri backend
 */
function handleEdgeDeletedEvent(payload: { id: string }): void {
  try {
    const event: Omit<RealtimeEdgeDeletedEvent, 'timestamp'> = {
      type: 'edge:deleted',
      namespace: 'sync',
      source: 'tauri-polling-service',
      edgeId: payload.id,
      metadata: {
        receivedAt: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.debug(`❌ Edge deleted: ${payload.id}`);
  } catch (error) {
    console.error('Error processing edge:deleted event', error);
  }
}

/**
 * Handle synchronization error events
 */
function handleSyncErrorEvent(payload: Record<string, unknown>): void {
  try {
    const message = String(payload.message);
    const errorType = String(payload.errorType) as
      | 'subscription-failed'
      | 'stream-interrupted'
      | 'parse-error'
      | 'unknown';
    const retryable = Boolean(payload.retryable);

    const event: Omit<RealtimeSyncErrorEvent, 'timestamp'> = {
      type: 'error:sync-failed',
      namespace: 'error',
      source: 'tauri-live-query',
      message,
      errorType,
      retryable,
      metadata: {
        timestamp: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.error(`❌ Sync error (${errorType}): ${message}`);
  } catch (error) {
    console.error('Error processing sync-error event', error);
  }
}

/**
 * Handle synchronization status change events
 */
function handleSyncStatusEvent(payload: Record<string, unknown>): void {
  try {
    const status = String(payload.status) as 'connected' | 'disconnected' | 'reconnecting';
    const reason = payload.reason ? String(payload.reason) : undefined;

    const event: Omit<RealtimeSyncStatusEvent, 'timestamp'> = {
      type: 'sync:status-changed',
      namespace: 'sync',
      source: 'tauri-live-query',
      status,
      reason,
      metadata: {
        timestamp: new Date().toISOString()
      }
    };

    eventBus.emit(event);
    console.info(`🔌 Sync status: ${status}${reason ? ` (${reason})` : ''}`);
  } catch (error) {
    console.error('Error processing sync-status event', error);
  }
}

/**
 * Check if running in Tauri environment
 */
function isRunningInTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI__' in window;
}
