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
  createNode(afterNodeId: string, content: string = '', nodeType: string = 'text'): string {
    const result = super.createNode(afterNodeId, content, nodeType);
    
    // Critical: Need full sync because createNode updates:
    // 1. New node in _nodes
    // 2. Parent's children array (or _rootNodeIds)
    // 3. AutoFocus states on multiple nodes
    this.syncReactiveState();
    
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
      // Incremental update: update modified nodes
      const updatedNode = super.nodes.get(nodeId);
      if (updatedNode) {
        this._reactiveNodes.set(nodeId, updatedNode);
        // Update parent and root nodes lists as needed
        this.updateHierarchyState(nodeId, updatedNode);
      }
    }
    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  outdentNode(nodeId: string): boolean {
    const result = super.outdentNode(nodeId);
    if (result) {
      // Incremental update: update modified nodes
      const updatedNode = super.nodes.get(nodeId);
      if (updatedNode) {
        this._reactiveNodes.set(nodeId, updatedNode);
        // Update parent and root nodes lists as needed
        this.updateHierarchyState(nodeId, updatedNode);
      }
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
      }
    }
    return result;
  }

  /**
   * Override to sync reactive state after operations
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    super.combineNodes(currentNodeId, previousNodeId);
    // Incremental update: update combined node and remove deleted node
    const updatedNode = super.nodes.get(previousNodeId);
    if (updatedNode) {
      this._reactiveNodes.set(previousNodeId, updatedNode);
    }
    // Remove the deleted node
    this._reactiveNodes.delete(currentNodeId);
    const rootIndex = this._reactiveRootNodeIds.indexOf(currentNodeId);
    if (rootIndex !== -1) {
      this._reactiveRootNodeIds.splice(rootIndex, 1);
    }
  }

  /**
   * Update hierarchy state for parent/child relationships
   */
  private updateHierarchyState(nodeId: string, updatedNode: Node): void {
    // Update parent node if it exists
    if (updatedNode.parentId) {
      const parentNode = super.nodes.get(updatedNode.parentId);
      if (parentNode) {
        this._reactiveNodes.set(updatedNode.parentId, parentNode);
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
}
