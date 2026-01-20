/**
 * Browser Sync Service - SSE Client for Browser Dev Mode
 *
 * Provides real-time synchronization via Server-Sent Events when running
 * in browser mode (not Tauri desktop app). This is the browser-mode equivalent
 * of Tauri's domain event subscription.
 *
 * Architecture:
 *   Browser ←──SSE──→ dev-proxy (port 3001) → SurrealStore (events)
 *                                                    ↑
 *   Surrealist ──────────────(direct DB changes)───→
 *
 * The dev-proxy forwards domain events from SurrealStore as SSE to connected browsers,
 * which update the SharedNodeStore and ReactiveStructureTree. This allows browser dev mode
 * to have the same real-time sync as the desktop app.
 *
 * Issue #724: Events now send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via getNode() API only when the node is in the active view.
 */

/* global EventSource, MessageEvent */

import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { SseEvent } from '$lib/types/sse-events';
import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { backendAdapter } from './backend-adapter';
import { createLogger } from '$lib/utils/logger';

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
  private maxReconnectAttempts = 5;
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

    // No clientId needed (Issue #715) - dev-proxy handles filtering server-side
    const sseUrl = this.sseEndpoint;

    log.debug('Connecting to SSE endpoint:', sseUrl);

    try {
      this.eventSource = new EventSource(sseUrl);

      this.eventSource.onopen = () => {
        log.info('SSE connection established');
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
   * Normalize node data from SSE events to type-specific format
   *
   * SSE events send generic Node objects where type-specific fields (like task status)
   * are stored in `properties`. This function converts them to the flat format
   * expected by the frontend stores and components.
   *
   * @param nodeData - Raw node data from SSE event
   * @returns Normalized node with flat spoke fields for typed nodes
   */
  private normalizeNodeData(nodeData: Node): Node {
    if (nodeData.nodeType === 'task') {
      return nodeToTaskNode(nodeData) as unknown as Node;
    }
    // Add other type-specific conversions here as needed (e.g., SchemaNode)
    return nodeData;
  }

  /**
   * Handle parsed SSE event
   *
   * Routes events to appropriate store/tree handlers to update UI.
   * Filtering handled server-side by dev-proxy (Issue #715).
   *
   * Issue #724: Events now send only node_id. Frontend fetches full data
   * via getNode() API only when the node is in the active view (SharedNodeStore).
   */
  private handleEvent(event: SseEvent): void {
    // No client-side filtering needed - dev-proxy filters out events from browser operations
    // Dev-proxy NodeService has client_id="dev-proxy", so all browser HTTP operations
    // emit events with source_client_id="dev-proxy", which SSE handler filters out.

    switch (event.type) {
      case 'nodeCreated': {
        log.debug('Node created:', event.nodeId);
        // Issue #724: Fetch full node data only if we need to display it
        // For now, always fetch since the node might be in the current view
        this.fetchAndUpdateNode(event.nodeId, 'nodeCreated');
        break;
      }

      case 'nodeUpdated': {
        log.debug('Node updated:', event.nodeId);
        // Issue #724: Only fetch if node is already in the store (visible to user)
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
        break;

      // ======================================================================
      // Unified Relationship Events (Issue #811)
      // All relationship types (has_child, member_of, mentions, custom) use these events.
      // ======================================================================

      case 'relationshipCreated': {
        log.debug(`Relationship created: ${event.relationshipType} (${event.fromId} -> ${event.toId})`);

        if (event.relationshipType === 'has_child') {
          // Hierarchy relationship
          if (structureTree) {
            const order = (event.properties?.order as number) ?? Date.now();
            const existingChildren = structureTree.getChildrenWithOrder(event.fromId);
            const alreadyExists = existingChildren.some((c) => c.nodeId === event.toId);
            if (!alreadyExists) {
              structureTree.addChild({
                parentId: event.fromId,
                childId: event.toId,
                order
              });
            }
          }
        } else if (event.relationshipType === 'member_of') {
          // Collection membership - log for now
          log.debug(`Member added: ${event.fromId} to collection ${event.toId}`);
        } else if (event.relationshipType === 'mentions') {
          // Mention relationship - log for now
          log.debug(`Mention created: ${event.fromId} mentions ${event.toId}`);
        } else {
          // Custom relationship type
          log.debug(`Custom relationship created: ${event.relationshipType}`);
        }
        break;
      }

      case 'relationshipUpdated': {
        log.debug(`Relationship updated: ${event.relationshipType} (${event.fromId} -> ${event.toId})`);

        if (event.relationshipType === 'has_child') {
          // Future: Update child order in structure tree
          log.debug(`Hierarchy order updated for ${event.toId}`);
        }
        break;
      }

      case 'relationshipDeleted': {
        log.debug(`Relationship deleted: ${event.relationshipType} (${event.id}) from ${event.fromId} to ${event.toId}`);

        if (event.relationshipType === 'has_child') {
          // Hierarchy deletion - update ReactiveStructureTree
          if (structureTree) {
            structureTree.removeChild({
              parentId: event.fromId,
              childId: event.toId,
              order: 0 // Order doesn't matter for removal
            });
          }
        } else if (event.relationshipType === 'member_of') {
          log.debug(`Member removed from collection: ${event.id}`);
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
   * Issue #724: Events now send only node_id. This method fetches the full
   * node data and updates the store.
   */
  private async fetchAndUpdateNode(nodeId: string, eventType: string): Promise<void> {
    try {
      const node = await backendAdapter.getNode(nodeId);
      if (node) {
        // Normalize node data to type-specific format (e.g., TaskNode with flat status)
        const normalizedNode = this.normalizeNodeData(node);
        // Use database source with sse-sync reason to indicate external change via SSE
        sharedNodeStore.setNode(normalizedNode, { type: 'database', reason: 'sse-sync' }, true);
        log.debug(`${eventType}: updated store for node`, nodeId);
      } else {
        log.warn(`${eventType}: node not found`, nodeId);
      }
    } catch (error) {
      log.error(`${eventType}: failed to fetch node`, { nodeId, error });
    }
  }

  /**
   * Schedule reconnection with exponential backoff
   */
  private scheduleReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      log.error('Max reconnect attempts reached. Real-time sync disabled.');
      log.error('Please check if dev-proxy is running (bun run dev:browser)');
      return;
    }

    this.reconnectAttempts++;
    const delay = this.baseReconnectDelay * Math.pow(2, this.reconnectAttempts - 1);
    log.info(
      `Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts}/${this.maxReconnectAttempts})`
    );

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
