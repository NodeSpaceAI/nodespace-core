/**
 * Tauri Domain Event Listener
 *
 * Listens for real-time synchronization events emitted from the Rust backend
 * via domain events. The backend's DomainEventForwarder service subscribes
 * to NodeService domain events and forwards them to the frontend via Tauri events.
 *
 * This module handles:
 * - Node events (created, updated, deleted) → updates SharedNodeStore
 * - Edge events (hierarchy, mentions) → updates ReactiveStructureTree
 *
 * This enables real-time sync when external sources (MCP, other windows) modify data.
 *
 * Issue #724: Events now send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via getNode() API only when the node is in the active view.
 */

import { listen } from '@tauri-apps/api/event';
import type { NodeEventData, EdgeRelationship } from '$lib/types/event-types';
import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { backendAdapter } from './backend-adapter';

/**
 * Normalize node data from domain events to type-specific format
 *
 * Domain events send generic Node objects where type-specific fields (like task status)
 * are stored in `properties`. This function converts them to the flat format
 * expected by the frontend stores and components.
 *
 * @param nodeData - Raw node data from domain event
 * @returns Normalized node with flat spoke fields for typed nodes
 */
function normalizeNodeData(nodeData: Node): Node {
  if (nodeData.nodeType === 'task') {
    return nodeToTaskNode(nodeData) as unknown as Node;
  }
  // Add other type-specific conversions here as needed (e.g., SchemaNode)
  return nodeData;
}

/**
 * Fetch full node data from API and update SharedNodeStore
 *
 * Issue #724: Events now send only node_id. This function fetches the full
 * node data and updates the store.
 */
async function fetchAndUpdateNode(nodeId: string, eventType: string): Promise<void> {
  try {
    const node = await backendAdapter.getNode(nodeId);
    if (node) {
      // Normalize node data to type-specific format (e.g., TaskNode with flat status)
      const normalizedNode = normalizeNodeData(node);
      // Use database source with domain-event reason to indicate external change
      sharedNodeStore.setNode(normalizedNode, { type: 'database', reason: 'domain-event' }, true);
      console.debug(`[TauriSync] ${eventType}: updated store for node`, nodeId);
    } else {
      console.warn(`[TauriSync] ${eventType}: node not found`, nodeId);
    }
  } catch (error) {
    console.error(`[TauriSync] ${eventType}: failed to fetch node`, nodeId, error);
  }
}

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
    // Listen for node events and update SharedNodeStore
    // Issue #724: Events now send only node_id, fetch full data if needed
    await listen<NodeEventData>('node:created', (event) => {
      console.debug(`[TauriSync] Node created: ${event.payload.id}`);
      // Fetch full node data since the node might be in the current view
      fetchAndUpdateNode(event.payload.id, 'node:created');
    });

    await listen<NodeEventData>('node:updated', (event) => {
      console.debug(`[TauriSync] Node updated: ${event.payload.id}`);
      // Issue #724: Only fetch if node is already in the store (visible to user)
      if (sharedNodeStore.hasNode(event.payload.id)) {
        fetchAndUpdateNode(event.payload.id, 'node:updated');
      } else {
        console.debug('[TauriSync] Node not in store, skipping fetch:', event.payload.id);
      }
    });

    await listen<{ id: string }>('node:deleted', (event) => {
      console.debug(`[TauriSync] Node deleted: ${event.payload.id}`);
      sharedNodeStore.deleteNode(event.payload.id, { type: 'database', reason: 'domain-event' }, true);
    });

    // Listen for edge events (hierarchy and mention relationships)
    // Serde internally-tagged format: fields are merged at top level (not nested)
    await listen<EdgeRelationship>('edge:created', (event) => {
      if (event.payload.type === 'hierarchy') {
        const hierarchyEdge = event.payload;
        console.debug(
          `[TauriSync] Hierarchy relationship created: ${hierarchyEdge.parentId} -> ${hierarchyEdge.childId}`
        );
        // Update ReactiveStructureTree with new edge
        if (structureTree) {
          const existingChildren = structureTree.getChildrenWithOrder(hierarchyEdge.parentId);
          const alreadyExists = existingChildren.some((c) => c.nodeId === hierarchyEdge.childId);
          if (!alreadyExists) {
            structureTree.addChild({
              parentId: hierarchyEdge.parentId,
              childId: hierarchyEdge.childId,
              order: hierarchyEdge.order ?? Date.now()
            });
          } else {
            console.debug(
              '[TauriSync] Edge already exists (optimistic), skipping:',
              hierarchyEdge.childId
            );
          }
        }
      } else if (event.payload.type === 'mention') {
        const mentionEdge = event.payload;
        console.debug(
          `[TauriSync] Mention relationship created: ${mentionEdge.sourceId} -> ${mentionEdge.targetId}`
        );
      }
    });

    await listen<EdgeRelationship>('edge:updated', (event) => {
      if (event.payload.type === 'hierarchy') {
        const hierarchyEdge = event.payload;
        console.debug(
          `[TauriSync] Hierarchy relationship updated: ${hierarchyEdge.parentId} -> ${hierarchyEdge.childId}`
        );
        // For now, just log - order updates are rare
        // Future: Add updateChildOrder method if needed
      } else if (event.payload.type === 'mention') {
        const mentionEdge = event.payload;
        console.debug(
          `[TauriSync] Mention relationship updated: ${mentionEdge.sourceId} -> ${mentionEdge.targetId}`
        );
      }
    });

    await listen<EdgeRelationship>('edge:deleted', (event) => {
      if (event.payload.type === 'hierarchy') {
        const hierarchyEdge = event.payload;
        console.debug(
          `[TauriSync] Hierarchy relationship deleted: ${hierarchyEdge.parentId} -> ${hierarchyEdge.childId}`
        );
        // Update ReactiveStructureTree by removing edge
        if (structureTree) {
          structureTree.removeChild({
            parentId: hierarchyEdge.parentId,
            childId: hierarchyEdge.childId,
            order: 0 // Order doesn't matter for removal
          });
        }
      } else if (event.payload.type === 'mention') {
        const mentionEdge = event.payload;
        console.debug(
          `[TauriSync] Mention relationship deleted: ${mentionEdge.sourceId} -> ${mentionEdge.targetId}`
        );
      }
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
