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
 * 1. **Edge Created Before Node Exists**
 *    - Scenario: Backend creates Node N1 under Parent P
 *    - Expected order: node:created → edge:created
 *    - Possible order: edge:created → node:created
 *    - Result: ReactiveStructureTree adds edge before SharedNodeStore has node
 *    - Mitigation: Both stores handle missing nodes gracefully
 *
 * 2. **Node Deleted Before Edge Deleted**
 *    - Scenario: Backend deletes Node N1 and its edges
 *    - Expected order: edge:deleted → node:deleted (edges deleted first)
 *    - Possible order: node:deleted → edge:deleted
 *    - Result: Node gone from store but edge still in tree until edge:deleted arrives
 *    - Mitigation: Both stores are idempotent and handle orphaned references
 *
 * 3. **Bulk Operations with Interleaved Events**
 *    - Scenario: Creating/deleting multiple nodes and edges simultaneously
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
      console.log('[BrowserSyncService] Skipping - running in Tauri mode');
      return;
    }

    console.log('[BrowserSyncService] Initializing SSE connection for browser mode');
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
      console.log('[BrowserSyncService] Already connected or connecting');
      return;
    }

    this.connectionState = 'connecting';

    // No clientId needed (Issue #715) - dev-proxy handles filtering server-side
    const sseUrl = this.sseEndpoint;

    console.log('[BrowserSyncService] Connecting to SSE endpoint:', sseUrl);

    try {
      this.eventSource = new EventSource(sseUrl);

      this.eventSource.onopen = () => {
        console.log('[BrowserSyncService] SSE connection established');
        this.connectionState = 'connected';
        this.reconnectAttempts = 0;
      };

      this.eventSource.onmessage = (event) => {
        this.handleMessage(event);
      };

      this.eventSource.onerror = (error) => {
        console.warn('[BrowserSyncService] SSE connection error:', error);
        this.connectionState = 'disconnected';
        this.eventSource?.close();
        this.eventSource = null;
        this.scheduleReconnect();
      };
    } catch (error) {
      console.error('[BrowserSyncService] Failed to create EventSource:', error);
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
      console.log('[BrowserSyncService] Received event:', data.type);
      this.handleEvent(data);
    } catch (error) {
      console.error('[BrowserSyncService] Failed to parse SSE event:', error, event.data);
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
        console.log('[BrowserSyncService] Node created:', event.nodeId);
        // Issue #724: Fetch full node data only if we need to display it
        // For now, always fetch since the node might be in the current view
        this.fetchAndUpdateNode(event.nodeId, 'nodeCreated');
        break;
      }

      case 'nodeUpdated': {
        console.log('[BrowserSyncService] Node updated:', event.nodeId);
        // Issue #724: Only fetch if node is already in the store (visible to user)
        // This avoids unnecessary API calls for nodes not in the current view
        if (sharedNodeStore.hasNode(event.nodeId)) {
          this.fetchAndUpdateNode(event.nodeId, 'nodeUpdated');
        } else {
          console.log('[BrowserSyncService] Node not in store, skipping fetch:', event.nodeId);
        }
        break;
      }

      case 'nodeDeleted':
        console.log('[BrowserSyncService] Node deleted:', event.nodeId);
        sharedNodeStore.deleteNode(event.nodeId, { type: 'database', reason: 'sse-sync' }, true);
        break;

      case 'edgeCreated':
        console.log('[BrowserSyncService] Edge created:', event.parentId, '->', event.childId);
        // Update ReactiveStructureTree with new edge
        // Note: structureTree.addChild expects HierarchyRelationship format
        // IMPORTANT: Check if edge already exists (added optimistically by createNode)
        // to avoid overwriting the correct order with Date.now()
        if (structureTree) {
          const existingChildren = structureTree.getChildrenWithOrder(event.parentId);
          const alreadyExists = existingChildren.some((c) => c.nodeId === event.childId);
          if (!alreadyExists) {
            structureTree.addChild({
              parentId: event.parentId,
              childId: event.childId,
              order: Date.now() // Use timestamp as order (will be sorted properly on next load)
            });
          } else {
            console.log(
              '[BrowserSyncService] Edge already exists (optimistic), skipping:',
              event.childId
            );
          }
        }
        break;

      case 'edgeDeleted':
        console.log('[BrowserSyncService] Edge deleted:', event.parentId, '->', event.childId);
        // Update ReactiveStructureTree by removing edge
        if (structureTree) {
          structureTree.removeChild({
            parentId: event.parentId,
            childId: event.childId,
            order: 0 // Order doesn't matter for removal
          });
        }
        break;

      default:
        console.warn('[BrowserSyncService] Unknown event type:', (event as SseEvent).type);
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
        console.log(`[BrowserSyncService] ${eventType}: updated store for node`, nodeId);
      } else {
        console.warn(`[BrowserSyncService] ${eventType}: node not found`, nodeId);
      }
    } catch (error) {
      console.error(`[BrowserSyncService] ${eventType}: failed to fetch node`, nodeId, error);
    }
  }

  /**
   * Schedule reconnection with exponential backoff
   */
  private scheduleReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      console.error(
        '[BrowserSyncService] Max reconnect attempts reached. Real-time sync disabled.'
      );
      console.error(
        '[BrowserSyncService] Please check if dev-proxy is running (bun run dev:browser)'
      );
      return;
    }

    this.reconnectAttempts++;
    const delay = this.baseReconnectDelay * Math.pow(2, this.reconnectAttempts - 1);
    console.log(
      `[BrowserSyncService] Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts}/${this.maxReconnectAttempts})`
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
    console.log('[BrowserSyncService] Destroying service');

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
