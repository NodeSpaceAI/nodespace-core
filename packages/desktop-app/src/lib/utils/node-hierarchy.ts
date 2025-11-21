/**
 * Node Hierarchy Utilities
 *
 * Helper functions for managing parent-child relationships in the node graph.
 * Addresses code review feedback from Issue #528 PR review.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store';

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
  const existingChildren = sharedNodeStore.getNodesForParent(parentId).map((n) => n.id);

  // Don't add duplicate
  if (existingChildren.includes(childId)) {
    return;
  }

  // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events
}
