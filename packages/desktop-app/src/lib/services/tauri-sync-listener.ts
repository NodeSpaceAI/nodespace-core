/**
 * Tauri Domain Event Listener
 *
 * Listens for real-time synchronization events emitted from the Rust backend
 * via domain events. The backend's DomainEventForwarder service subscribes
 * to NodeService domain events and forwards them to the frontend via Tauri events.
 *
 * This module handles:
 * - Node events (created, updated, deleted) → updates SharedNodeStore
 * - Relationship events (has_child, mentions, member_of) → updates ReactiveStructureTree
 *
 * This enables real-time sync when external sources (MCP, other windows) modify data.
 *
 * Issue #724: Events now send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via getNode() API only when the node is in the active view.
 *
 * Issue #811: All relationship types use unified RelationshipCreated/Updated/Deleted events.
 */

import { listen } from '@tauri-apps/api/event';
import type {
  NodeEventData,
  RelationshipEvent,
  RelationshipDeletedPayload
} from '$lib/types/event-types';
import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { backendAdapter } from './backend-adapter';
import { createLogger } from '$lib/utils/logger';
import { scheduleCollectionRefresh } from '$lib/utils/collection-refresh';

const log = createLogger('TauriSync');

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
      log.debug(`${eventType}: updated store for node`, nodeId);
    } else {
      log.warn(`${eventType}: node not found`, nodeId);
    }
  } catch (error) {
    log.error(`${eventType}: failed to fetch node`, { nodeId, error });
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
    log.debug('Not running in Tauri environment, skipping sync listener initialization');
    return;
  }

  log.info('Initializing Tauri real-time sync listeners');

  try {
    // Listen for node events and update SharedNodeStore
    // Issue #724: Events now send only node_id, fetch full data if needed
    // Issue #832: node:created includes nodeType for reactive UI updates
    await listen<NodeEventData>('node:created', (event) => {
      log.debug(`Node created: ${event.payload.id} (type: ${event.payload.nodeType})`);

      // Issue #832: If a collection node is created, refresh collections sidebar
      if (event.payload.nodeType === 'collection') {
        scheduleCollectionRefresh();
      }

      // Fetch full node data since the node might be in the current view
      fetchAndUpdateNode(event.payload.id, 'node:created');
    });

    await listen<NodeEventData>('node:updated', (event) => {
      log.debug(`Node updated: ${event.payload.id}`);
      // Issue #724: Only fetch if node is already in the store (visible to user)
      if (sharedNodeStore.hasNode(event.payload.id)) {
        fetchAndUpdateNode(event.payload.id, 'node:updated');
      } else {
        log.debug('Node not in store, skipping fetch:', event.payload.id);
      }
    });

    await listen<{ id: string }>('node:deleted', (event) => {
      log.debug(`Node deleted: ${event.payload.id}`);
      sharedNodeStore.deleteNode(event.payload.id, { type: 'database', reason: 'domain-event' }, true);

      // Issue #832: We don't know if deleted node was a collection without fetching,
      // but if we have it cached in collectionsData, we should refresh
      // For simplicity, we rely on the UI to handle stale data gracefully
      // A more robust solution would cache node types or include type in delete events
    });

    // ========================================================================
    // Unified Relationship Events (Issue #811)
    // All relationship types (has_child, member_of, mentions, custom) use these events.
    // ========================================================================

    await listen<RelationshipEvent>('relationship:created', (event) => {
      const rel = event.payload;
      log.debug(`Relationship created: ${rel.relationshipType} (${rel.fromId} -> ${rel.toId})`);

      // Handle different relationship types
      if (rel.relationshipType === 'has_child') {
        // Hierarchy relationship
        if (structureTree) {
          const order = (rel.properties?.order as number) ?? Date.now();
          const existingChildren = structureTree.getChildrenWithOrder(rel.fromId);
          const alreadyExists = existingChildren.some((c) => c.nodeId === rel.toId);
          if (!alreadyExists) {
            structureTree.addChild({
              parentId: rel.fromId,
              childId: rel.toId,
              order
            });
          }
        }
      } else if (rel.relationshipType === 'member_of') {
        // Collection membership changed - refresh collections sidebar
        log.debug(`Member added: ${rel.fromId} to collection ${rel.toId}`);
        scheduleCollectionRefresh(rel.toId);
      } else if (rel.relationshipType === 'mentions') {
        // Mention relationship created - target node's mentionedIn needs refresh
        // mentionedIn is populated by get_children_tree, so we need to refetch the tree
        // for the target node to get updated backlinks
        log.debug(`Mention created: ${rel.fromId} mentions ${rel.toId}`);

        // If the target node is currently displayed, its mentionedIn will update
        // on next tree load. For immediate reactivity, the user can refresh the view.
        // Future enhancement: call loadChildrenTree for toId if it's the current view.
      } else {
        // Custom relationship type
        log.debug(`Custom relationship created: ${rel.relationshipType}`);
      }
    });

    await listen<RelationshipEvent>('relationship:updated', (event) => {
      const rel = event.payload;
      log.debug(`Relationship updated: ${rel.relationshipType} (${rel.fromId} -> ${rel.toId})`);

      // Handle order updates for hierarchy
      if (rel.relationshipType === 'has_child') {
        // Future: Update child order in structure tree
        log.debug(`Hierarchy order updated for ${rel.toId}`);
      }
    });

    await listen<RelationshipDeletedPayload>('relationship:deleted', (event) => {
      const { id, fromId, toId, relationshipType } = event.payload;
      log.debug(`Relationship deleted: ${relationshipType} (${id}) from ${fromId} to ${toId}`);

      if (relationshipType === 'has_child') {
        // Hierarchy deletion - update ReactiveStructureTree
        if (structureTree) {
          structureTree.removeChild({
            parentId: fromId,
            childId: toId,
            order: 0 // Order doesn't matter for removal
          });
        }
      } else if (relationshipType === 'member_of') {
        // Collection membership removed - refresh collections sidebar
        log.debug(`Member removed from collection: ${id}`);
        scheduleCollectionRefresh(toId);
      } else if (relationshipType === 'mentions') {
        // Mention relationship deleted - target node's mentionedIn needs refresh
        log.debug(`Mention deleted: ${id} (${fromId} -> ${toId})`);

        // Same as creation: mentionedIn updates on next tree load for toId.
        // Future enhancement: call loadChildrenTree for toId if it's the current view.
      }
    });

    // Listen for synchronization errors
    await listen<Record<string, unknown>>('sync:error', (event) => {
      const message = String(event.payload.message);
      const errorType = String(event.payload.errorType);
      log.error(`Sync error (${errorType}): ${message}`);
    });

    // Listen for synchronization status changes
    await listen<Record<string, unknown>>('sync:status', (event) => {
      const status = String(event.payload.status);
      const reason = event.payload.reason ? String(event.payload.reason) : '';
      log.info(`Sync status: ${status}${reason ? ` (${reason})` : ''}`);
    });

    log.info('Real-time sync listeners initialized successfully');
  } catch (error) {
    log.error('Failed to initialize sync listeners', error);
    throw new Error(`Failed to initialize sync listeners: ${error}`);
  }
}

/**
 * Check if running in Tauri environment
 */
function isRunningInTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI__' in window;
}
