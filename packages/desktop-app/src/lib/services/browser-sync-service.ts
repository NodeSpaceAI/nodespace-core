/**
 * Browser Sync Service - SSE Client for Browser Dev Mode
 *
 * Provides real-time synchronization via Server-Sent Events when running
 * in browser mode (not Tauri desktop app). This is the browser-mode equivalent
 * of Tauri's domain event subscription.
 *
 * Architecture:
 *   Browser ←──SSE──→ dev-proxy (port 3001) → SqliteStore (events)
 *
 * The dev-proxy forwards domain events from SqliteStore as SSE to connected browsers,
 * which update the SharedNodeStore and ReactiveStructureTree. This allows browser dev mode
 * to have the same real-time sync as the desktop app.
 *
 * Events send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via getNode() API only when the node is in the active view.
 */

/* global EventSource, MessageEvent */

import { sharedNodeStore, SimplePersistenceCoordinator } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { SseEvent } from '$lib/types/sse-events';
import { backendAdapter } from './backend-adapter';
import { createLogger } from '$lib/utils/logger';
import { scheduleCollectionRefresh, scheduleSchemaRefresh } from '$lib/utils/collection-refresh';
import { registerSchemaPlugin, unregisterSchemaPlugin } from '$lib/plugins/schema-plugin-loader';
import { applyHasChildCreated, applyHasChildUpdated, applyHasChildDeleted } from './hierarchy-sync';
import { normalizeNodeData } from './node-normalize';

const log = createLogger('BrowserSyncService');

/**
 * Connection state for the SSE client
 */
type ConnectionState = 'disconnected' | 'connecting' | 'connected';

/**
 * Browser Sync Service class
 *
 * Manages SSE connection to dev-proxy for real-time sync in browser mode.
 * Automatically reconnects on connection loss with exponential backoff.
 *
 * ## Event Ordering Guarantees
 *
 * **IMPORTANT**: SSE events are NOT guaranteed to arrive in the order they
 * occurred on the server. Due to network latency and buffering, events may
 * arrive out-of-order, potentially causing the following scenarios:
 *
 * ### Documented Race Conditions
 *
 * 1. **Relationship Created Before Node Exists**
 *    - Scenario: Backend creates Node N1 under Parent P
 *    - Expected order: node:created → relationship:created
 *    - Possible order: relationship:created → node:created
 *    - Result: ReactiveStructureTree adds relationship before SharedNodeStore has node
 *    - Mitigation: Both stores handle missing nodes gracefully
 *
 * 2. **Node Deleted Before Relationship Deleted**
 *    - Scenario: Backend deletes Node N1 and its relationships
 *    - Expected order: relationship:deleted → node:deleted (relationships deleted first)
 *    - Possible order: node:deleted → relationship:deleted
 *    - Result: Node gone from store but relationship still in tree until relationship:deleted arrives
 *    - Mitigation: Both stores are idempotent and handle orphaned references
 *
 * 3. **Bulk Operations with Interleaved Events**
 *    - Scenario: Creating/deleting multiple nodes and relationships simultaneously
 *    - Result: Events may arrive completely out-of-order
 *    - Mitigation: Each store processes events independently and handles duplicates
 *
 * ### Defensive Measures
 *
 * The following safeguards prevent data corruption:
 *
 * - **ReactiveStructureTree.addChild()**:
 *   - Detects duplicate edges (same parent/child pair)
 *   - Detects tree invariant violations (node with multiple parents)
 *   - Updates order if edge already exists with different order
 *
 * - **ReactiveStructureTree.removeChild()**:
 *   - Gracefully handles missing edges (no error if edge doesn't exist)
 *
 * - **SharedNodeStore.setNode()**:
 *   - Overwrites with latest data using version tracking
 *   - Handles Last-Write-Wins conflict resolution
 *
 * ### Future Improvements
 *
 * If out-of-order events cause user-visible issues:
 * 1. Implement event batching to group related operations
 * 2. Add sequence numbers to verify event ordering
 * 3. Buffer events and deliver in correct order
 * 4. Add cache to detect missing nodes and delay edge processing
 *
 * See: https://github.com/malibio/nodespace-core/issues/643
 */
class BrowserSyncService {
  private eventSource: EventSource | null = null;
  private reconnectAttempts = 0;
  private readonly maxReconnectDelay = 30_000; // 30 second ceiling
  private baseReconnectDelay = 1000; // 1 second
  private connectionState: ConnectionState = 'disconnected';
  private reconnectTimeout: ReturnType<typeof setTimeout> | null = null;

  /**
   * Base URL for the dev-proxy SSE endpoint
   */
  private readonly sseEndpoint = 'http://localhost:3001/api/events';

  /**
   * Initialize SSE connection (browser mode only)
   *
   * This method should be called once during app initialization.
   * It only establishes a connection when running in browser mode (not Tauri).
   */
  async initialize(): Promise<void> {
    // Only run in browser mode (not Tauri)
    if (!this.isBrowserMode()) {
      log.debug('Skipping - running in Tauri mode');
      return;
    }

    log.debug('Initializing SSE connection for browser mode');
    this.connect();
  }

  /**
   * Check if running in browser mode (not Tauri)
   */
  private isBrowserMode(): boolean {
    return (
      typeof window !== 'undefined' &&
      !('__TAURI__' in window) &&
      !('__TAURI_INTERNALS__' in window)
    );
  }

  /**
   * Establish SSE connection to dev-proxy
   */
  private connect(): void {
    if (this.connectionState === 'connecting' || this.connectionState === 'connected') {
      log.debug('Already connected or connecting');
      return;
    }

    this.connectionState = 'connecting';

    // No clientId needed - dev-proxy handles filtering server-side
    const sseUrl = this.sseEndpoint;

    log.debug('Connecting to SSE endpoint:', sseUrl);

    try {
      this.eventSource = new EventSource(sseUrl);

      this.eventSource.onopen = () => {
        if (this.reconnectAttempts > 0) {
          log.info(`SSE reconnected after ${this.reconnectAttempts} attempt(s)`);
        } else {
          log.info('SSE connection established');
        }
        this.connectionState = 'connected';
        this.reconnectAttempts = 0;
      };

      this.eventSource.onmessage = (event) => {
        this.handleMessage(event);
      };

      this.eventSource.onerror = (error) => {
        log.warn('SSE connection error:', error);
        this.connectionState = 'disconnected';
        this.eventSource?.close();
        this.eventSource = null;
        this.scheduleReconnect();
      };
    } catch (error) {
      log.error('Failed to create EventSource:', error);
      this.connectionState = 'disconnected';
      this.scheduleReconnect();
    }
  }

  /**
   * Handle incoming SSE message
   */
  private handleMessage(event: MessageEvent): void {
    try {
      const data = JSON.parse(event.data) as SseEvent;
      log.debug('Received event:', data.type);
      this.handleEvent(data);
    } catch (error) {
      log.error('Failed to parse SSE event:', { error, data: event.data });
    }
  }

  /**
   * Handle parsed SSE event
   *
   * Routes events to appropriate store/tree handlers to update UI.
   * Filtering handled server-side by dev-proxy.
   *
   * Events send only node_id. Frontend fetches full data
   * via getNode() API only when the node is in the active view (SharedNodeStore).
   */
  private handleEvent(event: SseEvent): void {
    // No client-side filtering needed - dev-proxy filters out events from browser operations
    // Dev-proxy NodeService has client_id="dev-proxy", so all browser HTTP operations
    // emit events with source_client_id="dev-proxy", which SSE handler filters out.

    switch (event.type) {
      case 'nodeCreated': {
        log.debug(`Node created: ${event.nodeId} (type: ${event.nodeType})`);

        // If a collection node is created, refresh collections sidebar
        if (event.nodeType === 'collection') {
          scheduleCollectionRefresh();
        }

        // If a schema node is created, refresh the schema types sidebar
        if (event.nodeType === 'schema') {
          scheduleSchemaRefresh();
          registerSchemaPlugin(event.nodeId).catch((err) =>
            log.error('Failed to register schema plugin:', err)
          );
        }

        // Fetch full node data only if we need to display it — for a node
        // this session hasn't seen before, always fetch, since it might
        // belong in the current view (sidebar list, tree, ...).
        //
        // Skip the fetch when a local write for this exact node is still in
        // flight: that write is almost certainly what CAUSED this creation
        // (e.g. `createSchemaInstance`'s own POST, or a follow-up write
        // issued immediately after, before this echo had a chance to
        // arrive — the "+ New chat" then model-selection sequence is the
        // reproduction case). A re-fetch racing that write can return the
        // PRE-write snapshot at the SAME version number the write's own
        // optimistic apply is still sitting at (the optimistic apply never
        // bumps `.version` — only the write's response does), which is
        // exactly the equal-version case `shouldSkipStaleAiChatUpdate`
        // otherwise has to reason about after the fact. The in-flight
        // write's own response (or a later, correctly-guarded `nodeUpdated`
        // echo) hydrates the real state regardless, so skipping here loses
        // nothing.
        if (!SimplePersistenceCoordinator.getInstance().hasPending(event.nodeId)) {
          this.fetchAndUpdateNode(event.nodeId, 'nodeCreated');
        } else {
          log.debug('Node has a pending local write, skipping creation echo fetch:', event.nodeId);
        }
        break;
      }

      case 'nodeUpdated': {
        log.debug('Node updated:', event.nodeId);
        // Only fetch if node is already in the store (visible to user)
        // This avoids unnecessary API calls for nodes not in the current view
        if (sharedNodeStore.hasNode(event.nodeId)) {
          this.fetchAndUpdateNode(event.nodeId, 'nodeUpdated');
        } else {
          log.debug('Node not in store, skipping fetch:', event.nodeId);
        }
        break;
      }

      case 'nodeDeleted':
        log.debug('Node deleted:', event.nodeId);
        sharedNodeStore.deleteNode(event.nodeId, { type: 'database', reason: 'sse-sync' }, true);
        // We don't know if deleted node was a collection without fetching,
        // but if we have it cached in collectionsData, we should refresh
        // For simplicity, we rely on the UI to handle stale data gracefully
        unregisterSchemaPlugin(event.nodeId);
        break;

      // ======================================================================
      // Unified Relationship Events
      // All relationship types (has_child, member_of, mentions, custom) use these events.
      // ======================================================================

      case 'relationshipCreated': {
        log.debug(`Relationship created: ${event.relationshipType} (${event.fromId} -> ${event.toId})`);
        if (event.relationshipType === 'has_child') {
          applyHasChildCreated(structureTree, {
            parentId: event.fromId,
            childId: event.toId,
            order: (event.properties as { order?: unknown } | undefined)?.order
          });
        } else if (event.relationshipType === 'member_of') {
          log.debug(`Member added: ${event.fromId} to collection ${event.toId}`);
          scheduleCollectionRefresh(event.toId);
        } else if (event.relationshipType === 'mentions') {
          log.debug(`Mention created: ${event.fromId} mentions ${event.toId}`);
        } else {
          log.debug(`Custom relationship created: ${event.relationshipType}`);
        }
        break;
      }

      case 'relationshipUpdated': {
        log.debug(`Relationship updated: ${event.relationshipType} (${event.fromId} -> ${event.toId})`);
        if (event.relationshipType === 'has_child') {
          applyHasChildUpdated(structureTree, {
            parentId: event.fromId,
            childId: event.toId,
            order: (event.properties as { order?: unknown } | undefined)?.order
          });
        }
        break;
      }

      case 'relationshipDeleted': {
        log.debug(`Relationship deleted: ${event.relationshipType} (${event.id}) from ${event.fromId} to ${event.toId}`);
        if (event.relationshipType === 'has_child') {
          applyHasChildDeleted(structureTree, {
            parentId: event.fromId,
            childId: event.toId
          });
        } else if (event.relationshipType === 'member_of') {
          log.debug(`Member removed from collection: ${event.id}`);
          scheduleCollectionRefresh(event.toId);
        } else if (event.relationshipType === 'mentions') {
          log.debug(`Mention deleted: ${event.id}`);
        }
        break;
      }

      default:
        log.warn('Unknown event type:', (event as SseEvent).type);
    }
  }

  /**
   * Fetch full node data from API and update SharedNodeStore
   *
   * Events send only node_id. This method fetches the full
   * node data and updates the store.
   */
  private async fetchAndUpdateNode(nodeId: string, eventType: string): Promise<void> {
    try {
      // ADR-053: capture the database generation before the read for parity with
      // the Tauri path. Browser (HTTP-adapter) mode does not currently support
      // switching databases, so the epoch never advances here — the guard is
      // defensive and future-proof.
      const epoch = sharedNodeStore.currentEpoch();
      const node = await backendAdapter.getNode(nodeId);
      if (sharedNodeStore.currentEpoch() !== epoch) return;
      if (node) {
        // Normalize node data to type-specific format (e.g., TaskNode with flat status)
        const normalizedNode = normalizeNodeData(node);
        // Use database source with sse-sync reason to indicate external change via SSE
        sharedNodeStore.setNode(normalizedNode, { type: 'database', reason: 'sse-sync' }, true);
        // `nodeUpdated` SSE events never carry `nodeType` (mirrors the Tauri
        // `node:updated` payload), so a schema's `title_template` edit is only
        // detectable here, after the full node is fetched. Refresh is a no-op
        // for anything that isn't a non-core schema. Fire-and-forget: a failure
        // only leaves that type's title stale.
        if (normalizedNode.nodeType === 'schema') {
          registerSchemaPlugin(normalizedNode.id).catch((err) =>
            log.error('Failed to refresh schema plugin on node event:', err)
          );
        }
        log.debug(`${eventType}: updated store for node`, nodeId);
      } else {
        log.warn(`${eventType}: node not found`, nodeId);
      }
    } catch (error) {
      log.error(`${eventType}: failed to fetch node`, { nodeId, error });
    }
  }

  /**
   * Schedule reconnection with capped exponential backoff.
   * Retries indefinitely — temporary outages (e.g. Vite HMR restart) self-heal.
   */
  private scheduleReconnect(): void {
    this.reconnectAttempts++;
    const delay = Math.min(
      this.baseReconnectDelay * Math.pow(2, this.reconnectAttempts - 1),
      this.maxReconnectDelay
    );
    log.info(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);

    this.reconnectTimeout = setTimeout(() => {
      this.reconnectTimeout = null;
      this.connect();
    }, delay);
  }

  /**
   * Get current connection state
   */
  getConnectionState(): ConnectionState {
    return this.connectionState;
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.connectionState === 'connected';
  }

  /**
   * Destroy the service and close SSE connection
   *
   * Should be called when the app is unmounted to clean up resources.
   */
  destroy(): void {
    log.debug('Destroying service');

    if (this.reconnectTimeout) {
      clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }

    if (this.eventSource) {
      this.eventSource.close();
      this.eventSource = null;
    }

    this.connectionState = 'disconnected';
    this.reconnectAttempts = 0;
  }
}

/**
 * Singleton instance for application-wide use
 */
export const browserSyncService = new BrowserSyncService();

/**
 * Default export
 */
export default BrowserSyncService;
