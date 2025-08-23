/**
 * ReactiveNodeManager - Svelte 5 reactive wrapper for NodeManager
 *
 * This file provides Svelte 5 $state() reactivity for the NodeManager service.
 * The core logic remains in NodeManager.ts for testability.
 */

import { NodeManager, type NodeManagerEvents, type Node } from './NodeManager';
import { eventBus } from './EventBus';

export interface HierarchicalNode extends Omit<Node, 'children'> {
  children: HierarchicalNode[];
}

export class ReactiveNodeManager extends NodeManager {
  private _reactiveNodes: Map<string, Node>;
  private _reactiveRootNodeIds: string[];
  private _reactivityTrigger: number;

  constructor(events: NodeManagerEvents) {
    super(events);

    // Initialize reactive state in constructor to allow for test mocking
    try {
      // Try to use $state if available (Svelte 5 environment)
      this._reactiveNodes =
        (globalThis as unknown as { $state?: <T>(value: T) => T }).$state?.(
          new Map<string, Node>()
        ) ?? new Map<string, Node>();
      this._reactiveRootNodeIds =
        (globalThis as unknown as { $state?: <T>(value: T) => T }).$state?.<string[]>([]) ?? [];
      this._reactivityTrigger =
        (globalThis as unknown as { $state?: <T>(value: T) => T }).$state?.(0) ?? 0;
    } catch {
      // Fallback for test environment
      this._reactiveNodes = new Map<string, Node>();
      this._reactiveRootNodeIds = [];
      this._reactivityTrigger = 0;
    }

    // Sync reactive state with base class state
    this.syncReactiveState();

    // Set up EventBus subscriptions for reactive updates
    this.setupReactiveEventBusIntegration();
  }

  /**
   * Set up EventBus subscriptions for reactive UI updates
   */
  private setupReactiveEventBusIntegration(): void {
    // Listen for node status changes to trigger reactivity
    eventBus.subscribe('node:status-changed', (_event) => {
      // Force reactivity update when node status changes
      this.forceUIUpdate();
    });

    // Listen for decoration updates to trigger re-rendering
    eventBus.subscribe('decoration:update-needed', (event) => {
      const decorationEvent = event as import('./EventTypes').DecorationUpdateNeededEvent;
      // Update the specific node to trigger reactivity
      const node = super.nodes.get(decorationEvent.nodeId);
      if (node) {
        this._reactiveNodes.set(decorationEvent.nodeId, { ...node });
        this.forceUIUpdate();
      }
    });

    // Listen for hierarchy changes from EventBus
    eventBus.subscribe('hierarchy:changed', () => {
      this.forceUIUpdate();
    });

    // Listen for focus requests
    eventBus.subscribe('focus:requested', (_event) => {
      // The focus handling is still delegated to the legacy callback system
      // but we can add reactive state updates here if needed
      this.forceUIUpdate();
    });
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
   * Override to use reactive collapsed nodes
   */
  get collapsedNodes(): Set<string> {
    return super.collapsedNodes;
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
    const getVisibleNodesRecursive = (nodeIds: string[], depth: number = 0, visited = new Set<string>()): Node[] => {
      const result: Node[] = [];
      for (const nodeId of nodeIds) {
        // Prevent infinite recursion from circular references
        if (visited.has(nodeId)) {
          console.warn(`Circular reference detected for node ${nodeId}, skipping to prevent duplicate keys`);
          continue;
        }
        
        const node = this._reactiveNodes.get(nodeId);
        if (node) {
          visited.add(nodeId);
          
          // Add hierarchy depth to node for CSS indentation
          const nodeWithDepth = {
            ...node,
            hierarchyDepth: depth
          };
          result.push(nodeWithDepth);
          
          // Include children if node is expanded
          if (node.expanded && node.children.length > 0) {
            result.push(...getVisibleNodesRecursive(node.children, depth + 1, visited));
          }
          
          visited.delete(nodeId); // Allow node to appear in different branches
        }
      }
      return result;
    };

    const nodes = getVisibleNodesRecursive(this._reactiveRootNodeIds);
    
    // DEFENSIVE: Validate no duplicate IDs before returning to UI
    const seenIds = new Set<string>();
    const validatedNodes = nodes.filter(node => {
      if (seenIds.has(node.id)) {
        console.error(`DUPLICATE NODE ID FILTERED OUT: ${node.id}`, node);
        return false; // Filter out duplicate
      }
      seenIds.add(node.id);
      return true;
    });

    if (validatedNodes.length !== nodes.length) {
      console.error(`Filtered out ${nodes.length - validatedNodes.length} duplicate nodes from visibleNodes`);
      // Force a full sync to recover from inconsistent state
      setTimeout(() => this.syncReactiveState(), 0);
    }

    return validatedNodes;
  }

  /**
   * Get root nodes with their children populated for hierarchical rendering
   */
  get rootNodesWithChildren(): HierarchicalNode[] {
    // Access reactivity trigger to ensure this getter re-runs when state changes
    void this._reactivityTrigger;

    const result: HierarchicalNode[] = [];
    for (const nodeId of this._reactiveRootNodeIds) {
      const node = this._reactiveNodes.get(nodeId);
      if (node) {
        // Create a copy with populated children
        const nodeWithChildren: HierarchicalNode = {
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
  private getChildrenNodes(parentId: string): HierarchicalNode[] {
    const parentNode = this._reactiveNodes.get(parentId);
    if (!parentNode || !parentNode.children.length) return [];

    return parentNode.children
      .map((childId) => {
        const childNode = this._reactiveNodes.get(childId);
        if (childNode) {
          const hierarchicalChild: HierarchicalNode = {
            ...childNode,
            children: this.getChildrenNodes(childId)
          };
          return hierarchicalChild;
        }
        return null;
      })
      .filter((node): node is HierarchicalNode => node !== null);
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
    inheritHeaderLevel?: number,
    cursorAtBeginning: boolean = false
  ): string {
    const result = super.createNode(
      afterNodeId,
      content,
      nodeType,
      inheritHeaderLevel,
      cursorAtBeginning
    );

    // CRITICAL FIX: Use full synchronization for node creation to prevent race conditions
    // Incremental updates can cause temporary inconsistencies that lead to duplicate keys
    // during rapid operations like multiple Enter key presses
    this.syncReactiveState();
    this.forceUIUpdate();

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

  /**
   * Override to trigger UI updates for collapsed state changes
   */
  toggleCollapsed(nodeId: string): boolean {
    const result = super.toggleCollapsed(nodeId);
    // Force UI update when collapsed state changes
    this.forceUIUpdate();
    return result;
  }

  /**
   * Override to use reactive state
   */
  isNodeCollapsed(nodeId: string): boolean {
    return super.isNodeCollapsed(nodeId);
  }
}
