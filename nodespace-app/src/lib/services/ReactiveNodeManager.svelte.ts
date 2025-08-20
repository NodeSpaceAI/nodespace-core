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
        this._reactiveNodes.set(newNode.parentId, parentNode);
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
