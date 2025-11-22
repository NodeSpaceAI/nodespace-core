/**
 * Node Hierarchy Utilities
 *
 * Helper functions for managing parent-child relationships in the node graph.
 * Addresses code review feedback from Issue #528 PR review.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store';
import { structureTree as reactiveStructureTree } from '$lib/stores/reactive-structure-tree.svelte';

/**
 * Registers a child node with its parent in the SharedNodeStore children cache.
 *
 * This ensures the promoted node appears in the parent's children array,
 * preventing orphaning and UI disappearance issues.
 *
 * **Critical for Issue #528 fix**: After promoting a placeholder, the children
 * cache must be updated to establish the parent-child relationship in the
 * frontend (backend edges are created separately via NodeOperations).
 *
 * @param parentId - The ID of the parent node
 * @param childId - The ID of the child node to register
 *
 * @example
 * ```typescript
 * // After promoting a placeholder:
 * const promotedNode = { ...placeholder, content: newContent };
 * await backend.createNode(promotedNode);
 * registerChildWithParent(parentNodeId, promotedNode.id); // ✅ Update cache
 * ```
 */
export function registerChildWithParent(parentId: string, childId: string): void {
  // Validate parent exists in store
  const parent = sharedNodeStore.getNode(parentId);
  if (!parent) {
    console.warn(
      `[registerChildWithParent] Parent ${parentId} not found in store, skipping cache update`
    );
    return;
  }

  // Get existing children (if any)
  const existingChildren = reactiveStructureTree.getChildren(parentId);

  // Don't add duplicate
  if (existingChildren.includes(childId)) {
    return;
  }

  // CRITICAL FIX: Manually add in-memory relationship to ReactiveStructureTree
  // For placeholder promotion, the node hasn't been persisted yet so LIVE SELECT won't fire
  // We need to manually register the relationship so visibleNodesFromStores includes the promoted node
  // This prevents a new placeholder from being created immediately after promotion
  //
  // TODO: This is a workaround. The proper fix (per hierarchy-reactivity-architecture-review.md)
  // is for the backend to return complete relationship data from node creation, allowing the
  // frontend to apply a proper optimistic update instead of manufacturing relationship records.
  // See Issue #528 for the placeholder promotion bug context.
  const order = existingChildren.length > 0 ? existingChildren.length + 1.0 : 1.0;
  reactiveStructureTree.addInMemoryRelationship(parentId, childId, order);
}
