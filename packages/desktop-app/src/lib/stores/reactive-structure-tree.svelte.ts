/**
 * ReactiveStructureTree - Reactive Store for Node Hierarchy
 *
 * Maintains a reactive map of parent→children relationships using Svelte 5 $state.
 * Subscribes to LIVE SELECT edge events (edge:created, edge:updated, edge:deleted)
 * and automatically keeps the tree synchronized and sorted.
 *
 * Features:
 * - Svelte 5 $state for automatic UI reactivity
 * - Binary search insertion to maintain sorted children
 * - Snapshot/restore for optimistic rollback
 * - Real-time synchronization via Tauri events
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { HierarchyRelationship } from '$lib/types/event-types';

interface ChildInfo {
  nodeId: string;
  order: number;
}

class ReactiveStructureTree {
  // Reactive map using Svelte 5 $state - automatically triggers reactivity
  children = $state(new Map<string, ChildInfo[]>());
  // Version counter to ensure reactivity triggers on Map mutations
  version = $state(0);
  private unlisteners: UnlistenFn[] = [];
  private initialized = false;

  /**
   * Initialize the tree with bulk load and LIVE SELECT event subscriptions
   * This should be called once during app startup
   */
  async initialize() {
    if (this.initialized) return;

    console.log('[ReactiveStructureTree] Initializing...');

    try {
      // Bulk load existing edges from backend before subscribing to events
      // This ensures the tree is populated with existing hierarchy on startup
      const initialEdges = await invoke<HierarchyRelationship[]>('get_all_edges');
      this.buildTree(initialEdges);
      console.log('[ReactiveStructureTree] Bulk loaded', initialEdges.length, 'edges');

      // Subscribe to LIVE SELECT structure events for real-time updates
      await this.subscribeToEvents();

      this.initialized = true;
      console.log('[ReactiveStructureTree] Initialization complete');
    } catch (error) {
      console.error('[ReactiveStructureTree] Initialization failed:', error);
      throw error;
    }
  }

  /**
   * Build tree from bulk relationship data
   * Called during initialization to populate the tree with existing relationships
   * @private
   */
  private buildTree(relationships: HierarchyRelationship[]) {
    const tree = new Map<string, ChildInfo[]>();

    for (const rel of relationships) {
      if (!tree.has(rel.parentId)) {
        tree.set(rel.parentId, []);
      }
      tree.get(rel.parentId)!.push({
        nodeId: rel.childId,
        order: rel.order
      });
    }

    // Sort all children arrays by order
    for (const [_parentId, children] of tree) {
      children.sort((a, b) => a.order - b.order);
    }

    this.children = tree;
  }

  /**
   * Subscribe to edge CRUD events from LIVE SELECT
   * @throws Error if critical event subscriptions fail
   */
  private async subscribeToEvents() {
    try {
      // Edge created event
      const unlistenCreated = await listen<HierarchyRelationship>('edge:created', (event) => {
        console.log('[ReactiveStructureTree] Hierarchy relationship created:', event.payload);
        this.addChild(event.payload);
      });
      this.unlisteners.push(unlistenCreated);
    } catch (error) {
      console.error('[ReactiveStructureTree] Failed to subscribe to edge:created', error);
      throw new Error('Failed to subscribe to critical structure events: edge:created');
    }

    try {
      // Edge deleted event
      const unlistenDeleted = await listen<HierarchyRelationship>('edge:deleted', (event) => {
        console.log('[ReactiveStructureTree] Hierarchy relationship deleted:', event.payload);
        this.removeChild(event.payload);
      });
      this.unlisteners.push(unlistenDeleted);
    } catch (error) {
      console.error('[ReactiveStructureTree] Failed to subscribe to edge:deleted', error);
      throw new Error('Failed to subscribe to critical structure events: edge:deleted');
    }

    try {
      // Edge updated event (for order changes during rebalancing)
      const unlistenUpdated = await listen<HierarchyRelationship>('edge:updated', (event) => {
        console.log('[ReactiveStructureTree] Hierarchy relationship updated:', event.payload);
        this.updateChildOrder(event.payload);
      });
      this.unlisteners.push(unlistenUpdated);
    } catch (error) {
      console.error('[ReactiveStructureTree] Failed to subscribe to edge:updated', error);
      throw new Error('Failed to subscribe to critical structure events: edge:updated');
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
   */
  private addChild(rel: HierarchyRelationship) {
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
      console.warn(
        `[ReactiveStructureTree] Duplicate relationship detected: ${childId} already child of ${parentId}`,
        rel
      );
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
    // Notify Svelte of the change
    this.children.set(parentId, children);
    // Increment version to trigger reactivity in derived states
    this.version++;
  }

  /**
   * Remove a child from parent's children
   */
  private removeChild(rel: HierarchyRelationship) {
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
   * Update child order (rare - only during rebalancing)
   */
  private updateChildOrder(rel: HierarchyRelationship) {
    // Remove and re-add to update order
    this.removeChild(rel);
    this.addChild(rel);
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
   * Cleanup event listeners
   */
  async destroy() {
    for (const unlisten of this.unlisteners) {
      unlisten();
    }
    this.unlisteners = [];
    this.initialized = false;
  }

  /**
   * Manually register an in-memory parent-child relationship (for placeholder promotion)
   *
   * Use this when creating parent-child relationships for nodes that haven't
   * been persisted yet and won't trigger LIVE SELECT events.
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
   * TEST ONLY: Direct access to addChild for testing binary search algorithm
   * @internal
   */
  __testOnly_addChild(edge: HierarchyRelationship) {
    this.addChild(edge);
  }
}

// Export singleton instance
export const structureTree = new ReactiveStructureTree();
