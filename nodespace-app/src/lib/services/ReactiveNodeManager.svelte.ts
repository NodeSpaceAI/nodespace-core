/**
 * ReactiveNodeManager - Svelte 5 reactive wrapper for NodeManager
 *
 * This file provides Svelte 5 $state() reactivity for the NodeManager service.
 * The core logic remains in NodeManager.ts for testability.
 */

import { NodeManager, type NodeManagerEvents, type Node } from './NodeManager';

export class ReactiveNodeManager extends NodeManager {
  private _reactiveNodes = $state(new Map<string, Node>());
  private _reactiveRootNodeIds = $state<string[]>([]);
  private _reactivityTrigger = $state(0); // Counter to force UI updates

  constructor(events: NodeManagerEvents) {
    super(events);

    // Sync reactive state with base class state
    this.syncReactiveState();
  }

  /**
   * Override to use reactive state
   */
  get nodes(): Map<string, Node> {
    return this._reactiveNodes;
  }

  /**
   * Override to use reactive state
   */
  get rootNodeIds(): string[] {
    return this._reactiveRootNodeIds;
  }

  /**
   * Override to use reactive state for UI updates
   * Note: Manual implementation instead of $derived due to Svelte 5 reactivity issues
   * Returns flat list with hierarchy depth for CSS-based indentation
   */
  get visibleNodes(): Node[] {
    // Access reactivity trigger to ensure this getter re-runs when state changes
    void this._reactivityTrigger;

    // Recursive helper function to get visible nodes with depth
    const getVisibleNodesRecursive = (nodeIds: string[], depth: number = 0): Node[] => {
      const result: Node[] = [];
      for (const nodeId of nodeIds) {
        const node = this._reactiveNodes.get(nodeId);
        if (node) {
          // Add hierarchy depth to node for CSS indentation
          const nodeWithDepth = {
            ...node,
            hierarchyDepth: depth
          };
          result.push(nodeWithDepth);
          // Include children if node is expanded
          if (node.expanded && node.children.length > 0) {
            result.push(...getVisibleNodesRecursive(node.children, depth + 1));
          }
        }
      }
      return result;
    };

    return getVisibleNodesRecursive(this._reactiveRootNodeIds);
  }

  /**
   * Get root nodes with their children populated for hierarchical rendering
   */
  get rootNodesWithChildren(): Node[] {
    // Access reactivity trigger to ensure this getter re-runs when state changes
    void this._reactivityTrigger;

    const result: Node[] = [];
    for (const nodeId of this._reactiveRootNodeIds) {
      const node = this._reactiveNodes.get(nodeId);
      if (node) {
        // Create a copy with populated children
        const nodeWithChildren = {
          ...node,
          children: this.getChildrenNodes(nodeId)
        };
        result.push(nodeWithChildren);
      }
    }
    return result;
  }

  /**
   * Force UI update by incrementing reactivity trigger
   */
  private forceUIUpdate(): void {
    this._reactivityTrigger++;
  }

  /**
   * Recursively get child nodes for a given parent
   */
  private getChildrenNodes(parentId: string): Node[] {
    const parentNode = this._reactiveNodes.get(parentId);
    if (!parentNode || !parentNode.children.length) return [];

    return parentNode.children
      .map((childId) => {
        const childNode = this._reactiveNodes.get(childId);
        if (childNode) {
          return {
            ...childNode,
            children: this.getChildrenNodes(childId)
          };
        }
        return null;
      })
      .filter(Boolean) as Node[];
  }

  /**
   * Override to sync reactive state after operations
   */
  initializeFromLegacyData(legacyNodes: unknown[]): void {
    super.initializeFromLegacyData(legacyNodes);
    // Full sync needed for initialization
    this.syncReactiveState();
  }

  /**
   * Override to sync reactive state after operations
   */
  createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    inheritHeaderLevel?: number
  ): string {
    const result = super.createNode(afterNodeId, content, nodeType, inheritHeaderLevel);

    // CRITICAL FIX: Comprehensive reactive state synchronization
    // The base class modifies multiple parts of the state during createNode:
    // 1. Creates new node in _nodes map
    // 2. Updates parent's children array OR root nodes array
    // 3. Clears autoFocus on all nodes and sets it on new node
    // We must sync ALL these changes to reactive state:

    const newNode = super.nodes.get(result);
    if (!newNode) return result;

    // Add the new node to reactive state
    this._reactiveNodes.set(result, newNode);

    // Update the parent node's children array in reactive state
    if (newNode.parentId) {
      const parentNode = super.nodes.get(newNode.parentId);
      if (parentNode) {
        // Create a new object reference to trigger Svelte reactivity
        this._reactiveNodes.set(newNode.parentId, { ...parentNode });
        // Force UI update
        this.forceUIUpdate();
      }
    } else {
      // Update root nodes list for root-level insertions
      const baseRootIds = super.rootNodeIds;
      this._reactiveRootNodeIds.length = 0;
      this._reactiveRootNodeIds.push(...baseRootIds);
    }

    // Sync autoFocus changes - base class clears all and sets new node
    // Rather than iterate all nodes, sync efficiently:
    this.updateAutoFocusState();

    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  deleteNode(nodeId: string): void {
    super.deleteNode(nodeId);
    // Incremental update: remove node from reactive state
    this._reactiveNodes.delete(nodeId);
    // Update root nodes list if this was a root node
    const rootIndex = this._reactiveRootNodeIds.indexOf(nodeId);
    if (rootIndex !== -1) {
      this._reactiveRootNodeIds.splice(rootIndex, 1);
    }
  }

  /**
   * Override to sync reactive state after operations
   */
  indentNode(nodeId: string): boolean {
    const result = super.indentNode(nodeId);
    if (result) {
      // CRITICAL FIX: Use full synchronization for complex hierarchy changes
      // Indent operations modify multiple parent-child relationships that
      // can't be safely handled with incremental updates
      this.syncReactiveState();
      this.forceUIUpdate();
    }
    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  outdentNode(nodeId: string): boolean {
    const result = super.outdentNode(nodeId);
    if (result) {
      // CRITICAL FIX: Use full synchronization for complex hierarchy changes
      // Outdent operations modify multiple parent-child relationships that
      // can't be safely handled with incremental updates
      this.syncReactiveState();
      this.forceUIUpdate();
    }
    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  toggleExpanded(nodeId: string): boolean {
    const result = super.toggleExpanded(nodeId);
    if (result) {
      // Incremental update: only update the specific node
      const updatedNode = super.nodes.get(nodeId);
      if (updatedNode) {
        this._reactiveNodes.set(nodeId, updatedNode);
        // CRITICAL FIX: Force UI update to trigger visibleNodes reactivity
        this.forceUIUpdate();
      }
    }
    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    super.combineNodes(currentNodeId, previousNodeId);
    // CRITICAL FIX: Use full synchronization for complex node operations
    // combineNodes modifies content, deletes nodes, and potentially affects hierarchy
    // which requires comprehensive state sync to ensure UI reactivity
    this.syncReactiveState();
    this.forceUIUpdate();
  }

  /**
   * Update hierarchy state for parent/child relationships
   */
  private updateHierarchyState(nodeId: string, updatedNode: Node): void {
    // Update parent node if it exists
    if (updatedNode.parentId) {
      const parentNode = super.nodes.get(updatedNode.parentId);
      if (parentNode) {
        // Create new object reference to trigger Svelte reactivity
        this._reactiveNodes.set(updatedNode.parentId, { ...parentNode });
      }
    }

    // Update root nodes list based on current state
    const baseRootIds = super.rootNodeIds;
    this._reactiveRootNodeIds.length = 0;
    this._reactiveRootNodeIds.push(...baseRootIds);
  }

  /**
   * Sync reactive state with base class state (full synchronization)
   */
  private syncReactiveState(): void {
    // Clear reactive state
    this._reactiveNodes.clear();
    this._reactiveRootNodeIds.length = 0;

    // Copy from base class
    const baseNodes = super.nodes;
    const baseRootIds = super.rootNodeIds;

    // Update reactive state
    for (const [id, node] of baseNodes) {
      this._reactiveNodes.set(id, node);
    }

    this._reactiveRootNodeIds.push(...baseRootIds);
  }

  /**
   * Update autoFocus state efficiently - only update nodes that changed
   */
  private updateAutoFocusState(): void {
    // Base class sets autoFocus on exactly one node and clears all others
    // Find the node with autoFocus=true and sync efficiently
    for (const [id, baseNode] of super.nodes) {
      const reactiveNode = this._reactiveNodes.get(id);
      if (reactiveNode && reactiveNode.autoFocus !== baseNode.autoFocus) {
        this._reactiveNodes.set(id, baseNode);
      }
    }
  }
}
