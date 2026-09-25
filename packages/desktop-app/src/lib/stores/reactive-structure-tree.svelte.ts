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
 * - Reverse child→parent index for O(1) getParent
 * - runBatch() to apply a burst of mutations with one reactive notification
 * - Snapshot/restore for optimistic rollback
 */

import type { HierarchyRelationship } from '$lib/types/event-types';
import { createLogger } from '$lib/utils/logger';

interface ChildInfo {
  nodeId: string;
  order: number;
}

export class ReactiveStructureTree {
  // Reactive map using Svelte 5 $state.raw() - only reassignment triggers reactivity,
  // so we must reassign after every mutation (see notifyChange())
  children = $state.raw(new Map<string, ChildInfo[]>());

  /**
   * Reverse child→parent index so getParent is O(1) instead of a scan over
   * every edge in the tree. Non-reactive bookkeeping: every mutator
   * (addChildInternal, removeChild, clear, restore) keeps it consistent, and
   * getParent verifies each hit against `children` (self-healing on a stale
   * entry), so an inconsistent entry can never leak out of the API.
   */
  private parentIndex = new Map<string, string>();

  /** Depth of nested runBatch() calls; while >0, notifyChange() is deferred. */
  private batchDepth = 0;
  /** Whether any mutation was notified inside the current batch. */
  private batchDirty = false;

  /**
   * Reassign the children map to trigger Svelte reactivity.
   * $state.raw() only tracks reassignment, not in-place Map mutations.
   * Inside runBatch() the reassignment is deferred to the end of the
   * outermost batch so a burst of mutations invalidates consumers once.
   */
  private notifyChange() {
    if (this.batchDepth > 0) {
      this.batchDirty = true;
      return;
    }
    this.children = new Map(this.children);
  }

  /**
   * Run a sequence of tree mutations as one batch: mutations apply in call
   * order, but the reactive notification (the map reassignment every
   * getChildren consumer depends on) fires at most once, when the outermost
   * batch completes.
   *
   * Used by the sync-event coalescer to apply a burst of relationship events
   * without paying one full-tree invalidation per event.
   */
  runBatch(mutate: () => void): void {
    this.batchDepth++;
    try {
      mutate();
    } finally {
      this.batchDepth--;
      if (this.batchDepth === 0 && this.batchDirty) {
        this.batchDirty = false;
        this.children = new Map(this.children);
      }
    }
  }

  /** Rebuild the reverse child→parent index from the current children map. */
  private rebuildParentIndex(): void {
    this.parentIndex.clear();
    for (const [parentId, childrenList] of this.children) {
      for (const child of childrenList) {
        this.parentIndex.set(child.nodeId, parentId);
      }
    }
  }

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
   * Get parent of a node (reactive).
   *
   * O(1) via the reverse child→parent index — a full-edge scan here made a
   * cloud-sync relationship burst O(N²) on the main thread. Each hit is
   * verified against the children map (and a stale entry dropped) so the
   * index can never disagree with the tree an observer sees.
   */
  getParent(nodeId: string): string | null {
    // Read the reactive map first so consumers re-run on any tree change,
    // including when the lookup below misses ($state.raw tracks reassignment).
    const children = this.children;
    const parentId = this.parentIndex.get(nodeId);
    if (parentId === undefined) return null;
    const siblings = children.get(parentId);
    if (siblings && siblings.some((c) => c.nodeId === nodeId)) {
      return parentId;
    }
    // Stale entry (e.g. the map was replaced externally) — self-heal.
    this.parentIndex.delete(nodeId);
    return null;
  }

  /**
   * Add a child with binary search insertion to maintain sort by order
   *
   * Note: This method is public to support browser mode SSE sync (BrowserSyncService)
   * which needs to update the tree structure when external changes are detected.
   */
  addChild(rel: HierarchyRelationship) {
    this.addChildInternal(rel);
    this.notifyChange();
  }

  /**
   * Core mutation logic for adding a child. Does NOT trigger reactivity —
   * caller is responsible for calling notifyChange() after mutations complete.
   */
  private addChildInternal(rel: HierarchyRelationship) {
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
      // Refresh the reverse index (self-healing, cheap) and update order if different.
      this.parentIndex.set(childId, parentId);
      if (children[existingIndex].order !== order) {
        children[existingIndex].order = order;
        // Re-sort array
        children.sort((a, b) => a.order - b.order);
        this.children.set(parentId, children);
      }
      return;
    }

    // Child already under a DIFFERENT parent → REPARENT (move it), don't reject.
    // A node has exactly one parent, so a new has_child edge under a different
    // parent means the node moved (indent/outdent/drag). Cloud sync delivers the
    // new-parent edge and the old-parent edge-delete as independent events that can
    // arrive in either order — and a move applied on the receiver's daemon emits a
    // RelationshipCreated for the new parent but no delete for the old (the daemon
    // reparents in one step). Rejecting here left the node stranded under its old
    // parent on every OTHER window (indent/outdent not
    // reflected across windows / cloud sync). Prune the old parent, then add below.
    const currentParent = this.getParent(childId);
    if (currentParent && currentParent !== parentId) {
      const oldChildren = this.children.get(currentParent);
      if (oldChildren) {
        const pruned = oldChildren.filter((c) => c.nodeId !== childId);
        if (pruned.length === 0) {
          this.children.delete(currentParent);
        } else {
          this.children.set(currentParent, pruned);
        }
      }
    }

    // Binary search to find insertion position
    const insertIndex = this.findInsertPosition(children, order);

    // Insert at correct position to maintain sort
    const newChild: ChildInfo = {
      nodeId: childId,
      order
    };

    children.splice(insertIndex, 0, newChild);
    this.children.set(parentId, children);
    this.parentIndex.set(childId, parentId);
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

    // Drop the reverse-index entry only when this edge actually existed AND
    // the index still points at this parent — a stale delete for an edge the
    // child was already reparented away from must not clobber the live entry.
    if (filtered.length !== children.length && this.parentIndex.get(childId) === parentId) {
      this.parentIndex.delete(childId);
    }

    if (filtered.length === 0) {
      this.children.delete(parentId);
    } else {
      this.children.set(parentId, filtered);
    }
    this.notifyChange();
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
    this.rebuildParentIndex();
  }

  /**
   * Clear all tree data (used during database hot-swap)
   */
  clear() {
    this.children = new Map();
    this.parentIndex.clear();
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
   * @note This creates a temporary in-memory relationship. Once the backend
   * returns relationship data from the createNode API, this workaround can be
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
   * Batch add multiple relationships with a single reactivity notification.
   */
  batchAddRelationships(
    relationships: Array<{ parentId: string; childId: string; order: number }>
  ) {
    for (const rel of relationships) {
      this.addChildInternal(rel);
    }
    this.notifyChange();
  }

  /**
   * Move a child from one parent to another in-memory (for browser dev mode)
   * This is necessary because domain events don't fire automatically in browser mode.
   *
   * @param oldParentId - The current parent ID (null if root)
   * @param newParentId - The new parent ID (null to make the child a root node)
   * @param childId - The child node ID being moved
   * @param order - Optional order value for the new position (defaults to appending)
   */
  moveInMemoryRelationship(
    oldParentId: string | null,
    newParentId: string | null,
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

    // Root nodes have no parent edge — removal alone makes childId a root
    if (!newParentId) return;

    // Calculate order if not provided - append after last child by value (not count)
    let newOrder = order;
    if (newOrder === undefined) {
      const existingChildren = this.children.get(newParentId) || [];
      const lastOrder = existingChildren[existingChildren.length - 1]?.order ?? 0;
      newOrder = lastOrder + 1.0;
    }

    // Add to new parent
    this.addChild({
      parentId: newParentId,
      childId,
      order: newOrder
    });
  }

  updateChildOrder(parentId: string, childId: string, order: number): void {
    const children = this.children.get(parentId);
    if (!children) return;
    const existingIndex = children.findIndex((c) => c.nodeId === childId);
    if (existingIndex < 0) return;
    if (children[existingIndex].order !== order) {
      children[existingIndex].order = order;
      children.sort((a, b) => a.order - b.order);
      this.children.set(parentId, children);
      this.notifyChange();
    }
  }

  /**
   * Check hierarchy invariants after hydration.
   * I1: No orphan edges (every childId/parentId in children exists in nodeIds set).
   * I2: Single parent per child (no childId under two different parents).
   * I3: Sorted order (each parent's ChildInfo[] non-decreasing by order).
   *
   * Call once after batchAddRelationships completes during hydration.
   * In dev mode throws on violation; in prod logs only.
   *
   * @param nodeIds - Set of all known node IDs (from SharedNodeStore.nodes)
   * @param virtualIds - Additional allowed IDs not in nodeIds (e.g. '__root__', virtual date nodes)
   */
  assertInvariants(nodeIds: Set<string>, virtualIds: Set<string> = new Set()): void {
    const isKnown = (id: string) => nodeIds.has(id) || virtualIds.has(id);
    const seen = new Map<string, string>(); // childId -> parentId
    const violations: string[] = [];

    for (const [parentId, childInfos] of this.children) {
      if (!isKnown(parentId)) {
        violations.push(`I1: orphan parentId "${parentId}" not in nodeIds`);
      }
      let prevOrder = -Infinity;
      for (const { nodeId, order } of childInfos) {
        if (!isKnown(nodeId)) {
          violations.push(`I1: orphan childId "${nodeId}" (parent "${parentId}") not in nodeIds`);
        }
        const existingParent = seen.get(nodeId);
        if (existingParent && existingParent !== parentId) {
          violations.push(
            `I2: dual-parent "${nodeId}" under "${existingParent}" and "${parentId}"`
          );
        }
        seen.set(nodeId, parentId);
        if (order < prevOrder) {
          violations.push(
            `I3: unsorted order for "${nodeId}" under "${parentId}" (${order} < ${prevOrder})`
          );
        }
        prevOrder = order;
      }
    }

    if (violations.length === 0) return;

    const invLog = createLogger('HierarchyInvariant');
    for (const v of violations) {
      invLog.error(v);
    }
    if (import.meta.env.DEV) {
      throw new Error(`HierarchyInvariant violations:\n${violations.join('\n')}`);
    }
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
