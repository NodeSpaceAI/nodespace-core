/**
 * ReactiveStructureTree - Reactive Store for Node Hierarchy
 *
 * Maintains a reactive map of parent→children relationships using Svelte 5 $state.
 * Tree updates are managed via explicit method calls from components and services
 * (addChild, removeChild, moveInMemoryRelationship, batchAddRelationships).
 *
 * Features:
 * - Svelte 5 $state for automatic UI reactivity
 * - Binary search insertion to maintain sorted children
 * - Snapshot/restore for optimistic rollback
 */

import type { HierarchyRelationship } from '$lib/types/event-types';

interface ChildInfo {
  nodeId: string;
  order: number;
}

class ReactiveStructureTree {
  // Reactive map using Svelte 5 $state.raw() - Map mutations trigger reactivity automatically
  // Using $state.raw() instead of $state() allows direct Map mutations to be tracked
  children = $state.raw(new Map<string, ChildInfo[]>());

  /**
   * Get ordered child IDs for a parent (reactive)
   * This is the main public API - returns nodes in sorted order by edge.order
   */
  getChildren(parentId: string): string[] {
    const childInfos = this.children.get(parentId) || [];
    // Children are already sorted on insertion
    return childInfos.map((c) => c.nodeId);
  }

  /**
   * Get child info with order (for internal use, debugging)
   */
  getChildrenWithOrder(parentId: string): ChildInfo[] {
    return this.children.get(parentId) || [];
  }

  /**
   * Check if a node has children (reactive)
   */
  hasChildren(parentId: string): boolean {
    const children = this.children.get(parentId);
    return children ? children.length > 0 : false;
  }

  /**
   * Get parent of a node by searching all edges
   * Note: This is O(n) - consider adding reverse map if performance issue
   */
  getParent(nodeId: string): string | null {
    for (const [parentId, childrenList] of this.children) {
      if (childrenList.some((c) => c.nodeId === nodeId)) {
        return parentId;
      }
    }
    return null;
  }

  /**
   * Add a child with binary search insertion to maintain sort by order
   *
   * Note: This method is public to support browser mode SSE sync (BrowserSyncService)
   * which needs to update the tree structure when external changes are detected.
   */
  addChild(rel: HierarchyRelationship) {
    const parentId = rel.parentId;
    const childId = rel.childId;
    const order = rel.order;

    let children = this.children.get(parentId);
    if (!children) {
      children = [];
      this.children.set(parentId, children);
    }

    // Check if child already exists in current parent
    const existingIndex = children.findIndex((c) => c.nodeId === childId);
    if (existingIndex >= 0) {
      // Duplicate detected - this is expected when relationships are loaded from multiple sources
      // (e.g., bulk edge load + children-tree endpoint). Silently handle it.
      // Update order if different
      if (children[existingIndex].order !== order) {
        children[existingIndex].order = order;
        // Re-sort array
        children.sort((a, b) => a.order - b.order);
        // Notify Svelte of the change
        this.children.set(parentId, children);
      }
      return;
    }

    // Check if child already has a DIFFERENT parent (tree invariant violation)
    const currentParent = this.getParent(childId);
    if (currentParent && currentParent !== parentId) {
      console.error(
        `[ReactiveStructureTree] Tree invariant violation: ${childId} already has parent ${currentParent}, cannot add to ${parentId}`,
        rel
      );
      // In a tree structure, a node can only have one parent
      // This indicates either a database inconsistency or a bug in event emission
      // For now, we'll ignore the duplicate edge to preserve tree structure
      return;
    }

    // Binary search to find insertion position
    const insertIndex = this.findInsertPosition(children, order);

    // Insert at correct position to maintain sort
    const newChild: ChildInfo = {
      nodeId: childId,
      order
    };

    children.splice(insertIndex, 0, newChild);
    // Notify Svelte of the change ($state.raw() tracks this automatically)
    this.children.set(parentId, children);
  }

  /**
   * Remove a child from parent's children
   *
   * Note: This method is public to support browser mode SSE sync (BrowserSyncService)
   * which needs to update the tree structure when external changes are detected.
   */
  removeChild(rel: HierarchyRelationship) {
    const parentId = rel.parentId;
    const childId = rel.childId;

    const children = this.children.get(parentId) || [];
    const filtered = children.filter((c) => c.nodeId !== childId);

    if (filtered.length === 0) {
      this.children.delete(parentId);
    } else {
      this.children.set(parentId, filtered);
    }
  }

  /**
   * Binary search to find insertion position to maintain sort by order
   * @param children - sorted array of ChildInfo
   * @param order - order value to insert
   * @returns index where item should be inserted
   */
  private findInsertPosition(children: ChildInfo[], order: number): number {
    let left = 0;
    let right = children.length;

    while (left < right) {
      const mid = Math.floor((left + right) / 2);
      if (children[mid].order < order) {
        left = mid + 1;
      } else {
        right = mid;
      }
    }

    return left;
  }

  /**
   * Take snapshot of tree for optimistic rollback
   */
  snapshot(): Map<string, ChildInfo[]> {
    // Deep copy the map
    const snapshot = new Map<string, ChildInfo[]>();
    for (const [parentId, childrenList] of this.children) {
      snapshot.set(parentId, [...childrenList]);
    }
    return snapshot;
  }

  /**
   * Restore tree from snapshot (rollback on error)
   */
  restore(snapshot: Map<string, ChildInfo[]>) {
    this.children = snapshot;
  }

  /**
   * Manually register an in-memory parent-child relationship (for placeholder promotion)
   *
   * Use this when creating parent-child relationships for nodes that haven't
   * been persisted yet and won't trigger domain events.
   *
   * @param parentId - Parent node ID
   * @param childId - Child node ID
   * @param order - Sort order (use 1.0 for first child, or get max order + 1)
   *
   * @note This creates a temporary in-memory relationship. Once Issue #603 is complete
   * (backend returns relationship data from createNode API), this workaround can be
   * replaced with proper optimistic updates from backend responses.
   */
  addInMemoryRelationship(parentId: string, childId: string, order: number = 1.0) {
    this.addChild({
      parentId,
      childId,
      order
    });
  }

  /**
   * Batch add multiple relationships.
   * With $state.raw(), reactivity is automatic so no special batching needed.
   */
  batchAddRelationships(relationships: Array<{parentId: string, childId: string, order: number}>) {
    for (const rel of relationships) {
      this.addChild(rel);
    }
  }

  /**
   * Move a child from one parent to another in-memory (for browser dev mode)
   * This is necessary because domain events don't fire automatically in browser mode.
   *
   * @param oldParentId - The current parent ID (null if root)
   * @param newParentId - The new parent ID
   * @param childId - The child node ID being moved
   * @param order - Optional order value for the new position (defaults to appending)
   */
  moveInMemoryRelationship(
    oldParentId: string | null,
    newParentId: string,
    childId: string,
    order?: number
  ) {
    // Remove from old parent
    if (oldParentId) {
      this.removeChild({
        parentId: oldParentId,
        childId,
        order: 0 // order not used for removal
      });
    }

    // Calculate order if not provided - append to end
    let newOrder = order;
    if (newOrder === undefined) {
      const existingChildren = this.children.get(newParentId) || [];
      newOrder = existingChildren.length > 0 ? existingChildren.length + 1.0 : 1.0;
    }

    // Add to new parent
    this.addChild({
      parentId: newParentId,
      childId,
      order: newOrder
    });
  }

  /**
   * TEST ONLY: Direct access to addChild for testing binary search algorithm
   * @internal
   */
  __testOnly_addChild(edge: HierarchyRelationship) {
    this.addChild(edge);
  }
}

// Export singleton instance
export const structureTree = new ReactiveStructureTree();
