/**
 * ReactiveNodeData - Reactive Store for Node Content and Properties
 *
 * Maintains a reactive map of node data (content, properties, metadata) using Svelte 5 $state.
 * Subscribes to LIVE SELECT node events (node:created, node:updated, node:deleted)
 * and automatically keeps the data synchronized with debounced content updates.
 *
 * Features:
 * - Svelte 5 $state for automatic UI reactivity
 * - Debounced content updates (400ms for optimal typing batching)
 * - Immediate property updates with instant persistence
 * - Real-time synchronization via Tauri events
 * - Complete separation from structure (no beforeSiblingId or parentId)
 */

import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { Node } from '$lib/types';
import type { NodeEventData } from '$lib/services/event-types';
import { tauriNodeService } from '$lib/services/tauri-node-service';

/**
 * Debounced content update pending for a node
 * Tracks the pending content update and timeout handle
 */
interface PendingContentUpdate {
  content: string;
  version: number;
  timeoutId: ReturnType<typeof setTimeout>;
}

class ReactiveNodeData {
  // Reactive map using Svelte 5 $state - automatically triggers reactivity
  // Stores complete node data (content, properties, metadata) without structure fields
  nodes = $state(new Map<string, Node>());

  // Track pending debounced content updates to avoid duplicate persistence
  private pendingContentUpdates = new Map<string, PendingContentUpdate>();
  private unlisteners: UnlistenFn[] = [];
  private initialized = false;
  private readonly CONTENT_DEBOUNCE_MS = 400; // Optimal typing batching

  /**
   * Initialize the store with LIVE SELECT event subscriptions
   * This should be called once during app startup
   */
  async initialize() {
    if (this.initialized) return;

    console.log('[ReactiveNodeData] Initializing...');

    try {
      // Subscribe to LIVE SELECT node events
      await this.subscribeToEvents();

      this.initialized = true;
      console.log('[ReactiveNodeData] Initialization complete');
    } catch (error) {
      console.error('[ReactiveNodeData] Initialization failed:', error);
      throw error;
    }
  }

  /**
   * Subscribe to node CRUD events from LIVE SELECT
   * @throws Error if critical event subscriptions fail
   */
  private async subscribeToEvents() {
    try {
      // Node created event
      const unlistenCreated = await listen<{ nodeId: string; nodeData: NodeEventData }>(
        'node:created',
        (event) => {
          console.log('[ReactiveNodeData] Node created:', event.payload);
          this.addNode(event.payload.nodeData);
        }
      );
      this.unlisteners.push(unlistenCreated);
    } catch (error) {
      console.error('[ReactiveNodeData] Failed to subscribe to node:created', error);
      throw new Error('Failed to subscribe to critical node events: node:created');
    }

    try {
      // Node updated event
      const unlistenUpdated = await listen<{ nodeId: string; nodeData: NodeEventData }>(
        'node:updated',
        (event) => {
          console.log('[ReactiveNodeData] Node updated:', event.payload);
          this.updateNodeData(event.payload.nodeData);
        }
      );
      this.unlisteners.push(unlistenUpdated);
    } catch (error) {
      console.error('[ReactiveNodeData] Failed to subscribe to node:updated', error);
      throw new Error('Failed to subscribe to critical node events: node:updated');
    }

    try {
      // Node deleted event
      const unlistenDeleted = await listen<{ nodeId: string }>(
        'node:deleted',
        (event) => {
          console.log('[ReactiveNodeData] Node deleted:', event.payload.nodeId);
          this.removeNode(event.payload.nodeId);
        }
      );
      this.unlisteners.push(unlistenDeleted);
    } catch (error) {
      console.error('[ReactiveNodeData] Failed to subscribe to node:deleted', error);
      throw new Error('Failed to subscribe to critical node events: node:deleted');
    }
  }

  /**
   * Get a node by ID (reactive)
   * Returns the node if it exists in the store, undefined otherwise
   * This is a reactive getter - components that call this will re-render when the node changes
   */
  getNode(nodeId: string): Node | undefined {
    return this.nodes.get(nodeId);
  }

  /**
   * Update node content with 400ms debounce
   * Batches typing changes to reduce database writes
   * Content updates are NOT immediate - they're batched for optimal performance
   *
   * @param nodeId - ID of the node to update
   * @param content - New content value
   * @param version - Current version for optimistic concurrency control
   * @throws Error if node doesn't exist
   */
  updateContent(nodeId: string, content: string, version: number) {
    // Verify node exists
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`[ReactiveNodeData] Cannot update content: node ${nodeId} not found`);
    }

    // Cancel any pending update for this node
    const pending = this.pendingContentUpdates.get(nodeId);
    if (pending) {
      clearTimeout(pending.timeoutId);
      console.log(`[ReactiveNodeData] Cancelled pending update for ${nodeId}`);
    }

    // Update local state immediately for responsive UI
    node.content = content;
    node.version = version;

    // Schedule debounced persistence
    const timeoutId = setTimeout(() => {
      this.persistContent(nodeId, content, version);
    }, this.CONTENT_DEBOUNCE_MS);

    // Track pending update
    this.pendingContentUpdates.set(nodeId, {
      content,
      version,
      timeoutId
    });

    console.log(
      `[ReactiveNodeData] Scheduled debounced content update for ${nodeId} (${this.CONTENT_DEBOUNCE_MS}ms)`
    );
  }

  /**
   * Persist content update to backend
   * Called after debounce timeout
   * @private
   */
  private async persistContent(nodeId: string, content: string, version: number) {
    try {
      // Remove from pending list
      this.pendingContentUpdates.delete(nodeId);

      // Persist to backend
      const updated = await tauriNodeService.updateNode(nodeId, version, { content });

      // Update local store with new version from backend
      const node = this.nodes.get(nodeId);
      if (node) {
        node.content = updated.content;
        node.version = updated.version;
        node.modifiedAt = updated.modifiedAt;
        // Notify Svelte of the change
        this.nodes.set(nodeId, node);
      }

      console.log(`[ReactiveNodeData] Persisted content for ${nodeId}, new version: ${updated.version}`);
    } catch (error) {
      console.error(`[ReactiveNodeData] Failed to persist content for ${nodeId}:`, error);
      // TODO: Handle conflict resolution and retry logic
      // For now, log the error and let the UI handle retry through manual updates
    }
  }

  /**
   * Update node properties with immediate persistence
   * Properties are critical for node behavior, so updates persist immediately
   * without debouncing
   *
   * @param nodeId - ID of the node to update
   * @param properties - Properties object to merge
   * @param version - Current version for optimistic concurrency control
   * @throws Error if node doesn't exist
   */
  updateProperties(nodeId: string, properties: Record<string, unknown>, version: number) {
    // Verify node exists
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`[ReactiveNodeData] Cannot update properties: node ${nodeId} not found`);
    }

    // Update local state immediately
    node.properties = {
      ...node.properties,
      ...properties
    };
    node.version = version;

    // Notify Svelte of the change
    this.nodes.set(nodeId, node);

    // Persist immediately (no debounce for properties)
    this.persistProperties(nodeId, node.properties, version);

    console.log(`[ReactiveNodeData] Updated properties for ${nodeId}`);
  }

  /**
   * Persist properties to backend immediately
   * @private
   */
  private async persistProperties(
    nodeId: string,
    properties: Record<string, unknown>,
    version: number
  ) {
    try {
      const updated = await tauriNodeService.updateNode(nodeId, version, { properties });

      // Update local store with new version from backend
      const node = this.nodes.get(nodeId);
      if (node) {
        node.properties = updated.properties;
        node.version = updated.version;
        node.modifiedAt = updated.modifiedAt;
        // Notify Svelte of the change
        this.nodes.set(nodeId, node);
      }

      console.log(`[ReactiveNodeData] Persisted properties for ${nodeId}, new version: ${updated.version}`);
    } catch (error) {
      console.error(`[ReactiveNodeData] Failed to persist properties for ${nodeId}:`, error);
      // TODO: Handle conflict resolution and retry logic
    }
  }

  /**
   * Add a node to the store
   * @private
   */
  private addNode(nodeData: NodeEventData) {
    // Convert event data to Node format
    // Note: NodeEventData doesn't include all Node fields, but that's OK
    // We'll have the minimal fields from LIVE SELECT
    const node: Node = {
      id: nodeData.id,
      nodeType: nodeData.nodeType,
      content: nodeData.content,
      version: nodeData.version,
      createdAt: new Date(nodeData.modifiedAt).toISOString(),
      modifiedAt: nodeData.modifiedAt,
      beforeSiblingId: null, // Structure fields not stored here
      properties: {}
    };

    this.nodes.set(nodeData.id, node);
    console.log(`[ReactiveNodeData] Added node: ${nodeData.id}`);
  }

  /**
   * Update node data from LIVE SELECT event
   * @private
   */
  private updateNodeData(nodeData: NodeEventData) {
    const node = this.nodes.get(nodeData.id);

    if (!node) {
      // Node doesn't exist yet - add it
      this.addNode(nodeData);
      return;
    }

    // Update existing node
    // Cancel any pending content update for this node (server won, so we accept its version)
    const pending = this.pendingContentUpdates.get(nodeData.id);
    if (pending) {
      clearTimeout(pending.timeoutId);
      this.pendingContentUpdates.delete(nodeData.id);
      console.log(`[ReactiveNodeData] Cancelled pending update for ${nodeData.id} (server update received)`);
    }

    // Update fields from event
    node.content = nodeData.content;
    node.version = nodeData.version;
    node.modifiedAt = nodeData.modifiedAt;

    // Notify Svelte of the change
    this.nodes.set(nodeData.id, node);
    console.log(`[ReactiveNodeData] Updated node: ${nodeData.id}, version: ${nodeData.version}`);
  }

  /**
   * Remove a node from the store
   * @private
   */
  private removeNode(nodeId: string) {
    // Cancel any pending update for this node
    const pending = this.pendingContentUpdates.get(nodeId);
    if (pending) {
      clearTimeout(pending.timeoutId);
      this.pendingContentUpdates.delete(nodeId);
    }

    this.nodes.delete(nodeId);
    console.log(`[ReactiveNodeData] Removed node: ${nodeId}`);
  }

  /**
   * Flush all pending content updates
   * Called when necessary to force persistence before critical operations
   * For example: before app shutdown, before critical structural operations
   */
  async flushPendingUpdates(): Promise<void> {
    const pendingIds = Array.from(this.pendingContentUpdates.keys());

    if (pendingIds.length === 0) {
      return;
    }

    console.log(`[ReactiveNodeData] Flushing ${pendingIds.length} pending content updates`);

    // Wait for all pending updates to persist
    const promises = pendingIds.map(async (nodeId) => {
      const pending = this.pendingContentUpdates.get(nodeId);
      if (pending) {
        // Cancel the timeout and persist immediately
        clearTimeout(pending.timeoutId);
        await this.persistContent(nodeId, pending.content, pending.version);
      }
    });

    await Promise.all(promises);
    console.log('[ReactiveNodeData] All pending updates flushed');
  }

  /**
   * Take snapshot of all nodes for optimistic rollback
   */
  snapshot(): Map<string, Node> {
    // Deep copy the map
    const snapshot = new Map<string, Node>();
    for (const [nodeId, node] of this.nodes) {
      snapshot.set(nodeId, { ...node });
    }
    return snapshot;
  }

  /**
   * Restore all nodes from snapshot (rollback on error)
   */
  restore(snapshot: Map<string, Node>) {
    // Clear pending updates
    for (const timeoutId of this.pendingContentUpdates.values()) {
      clearTimeout(timeoutId.timeoutId);
    }
    this.pendingContentUpdates.clear();

    // Restore snapshot
    this.nodes = snapshot;
  }

  /**
   * Cleanup event listeners
   */
  async destroy() {
    for (const unlisten of this.unlisteners) {
      unlisten();
    }
    this.unlisteners = [];

    // Flush pending updates before destruction
    await this.flushPendingUpdates();

    // Clear all data
    this.nodes.clear();
    this.pendingContentUpdates.clear();
    this.initialized = false;
  }

  /**
   * TEST ONLY: Get pending content updates for verification
   * @internal
   */
  __testOnly_getPendingUpdates(): Map<string, Omit<PendingContentUpdate, 'timeoutId'>> {
    const pending = new Map<string, Omit<PendingContentUpdate, 'timeoutId'>>();
    for (const [nodeId, update] of this.pendingContentUpdates) {
      pending.set(nodeId, {
        content: update.content,
        version: update.version
      });
    }
    return pending;
  }
}

// Export singleton instance
export const nodeData = new ReactiveNodeData();
