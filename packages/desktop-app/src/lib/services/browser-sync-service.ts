/**
 * Browser Sync Service - SSE Client for Browser Dev Mode
 *
 * Provides real-time synchronization via Server-Sent Events when running
 * in browser mode (not Tauri desktop app). This is the browser-mode equivalent
 * of Tauri's LIVE SELECT event subscription.
 *
 * Architecture:
 *   Browser ←──SSE──→ dev-proxy (port 3001) → SurrealDB (port 8000)
 *                                                    ↑
 *   Surrealist ─────────────────────────────────────→
 *
 * When changes are made via Surrealist (or any other DB client), the dev-proxy
 * broadcasts SSE events to connected browsers, which update the SharedNodeStore
 * and ReactiveStructureTree.
 */

/* global EventSource, MessageEvent */

import { sharedNodeStore } from './shared-node-store';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { SseEvent } from '$lib/types/sse-events';

/**
 * Connection state for the SSE client
 */
type ConnectionState = 'disconnected' | 'connecting' | 'connected';

/**
 * Browser Sync Service class
 *
 * Manages SSE connection to dev-proxy for real-time sync in browser mode.
 * Automatically reconnects on connection loss with exponential backoff.
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
    console.log('[BrowserSyncService] Connecting to SSE endpoint:', this.sseEndpoint);

    try {
      this.eventSource = new EventSource(this.sseEndpoint);

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
   * Handle parsed SSE event
   *
   * Routes events to appropriate store/tree handlers to update UI.
   */
  private handleEvent(event: SseEvent): void {
    switch (event.type) {
      case 'nodeCreated':
        console.log('[BrowserSyncService] Node created:', event.nodeId);
        // Use database source with sse-sync reason to indicate external change via SSE
        sharedNodeStore.setNode(event.nodeData, { type: 'database', reason: 'sse-sync' }, true);
        break;

      case 'nodeUpdated':
        console.log('[BrowserSyncService] Node updated:', event.nodeId);
        // Use setNode for full replacement (event contains complete node data)
        sharedNodeStore.setNode(event.nodeData, { type: 'database', reason: 'sse-sync' }, true);
        break;

      case 'nodeDeleted':
        console.log('[BrowserSyncService] Node deleted:', event.nodeId);
        sharedNodeStore.deleteNode(event.nodeId, { type: 'database', reason: 'sse-sync' }, true);
        break;

      case 'edgeCreated':
        console.log('[BrowserSyncService] Edge created:', event.parentId, '->', event.childId);
        // Update ReactiveStructureTree with new edge
        // Note: structureTree.addChild expects HierarchyRelationship format
        if (structureTree) {
          structureTree.addChild({
            parentId: event.parentId,
            childId: event.childId,
            order: Date.now() // Use timestamp as order (will be sorted properly on next load)
          });
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
