/**
 * ReactiveNodeManager - Svelte store-based reactive wrapper for NodeManager
 *
 * This file provides Svelte store reactivity for the NodeManager service.
 * Uses writable stores for proper reactivity outside of .svelte components.
 */

import { NodeManager, type NodeManagerEvents, type Node } from './nodeManager';
import { eventBus } from './eventBus';
import { nodes, rootNodeIds, invalidateNodeCache } from './nodeStore';

export class ReactiveNodeManager extends NodeManager {
  private currentStoreNodes: Map<string, Node> = new Map();
  private isTyping = false;
  private typingTimer: number | null = null;

  constructor(events: NodeManagerEvents) {
    super(events);

    // Initialize stores with current state
    this.syncStores();

    // Set up EventBus subscriptions for reactive updates
    this.setupReactiveEventBusIntegration();
  }

  /**
   * Set up EventBus subscriptions for reactive UI updates
   */
  private setupReactiveEventBusIntegration(): void {
    // Listen for node status changes to trigger reactivity
    eventBus.subscribe('node:status-changed', (_event) => {
      // Update stores when node status changes
      this.syncStores();
    });

    // Listen for decoration updates to trigger re-rendering
    eventBus.subscribe('decoration:update-needed', (event) => {
      // Type guard to ensure the event has nodeId property
      if (this.hasNodeId(event)) {
        // Update the specific node in stores
        const node = super.nodes.get(event.nodeId);
        if (node) {
          this.syncStores();
        }
      }
    });

    // Listen for hierarchy changes from EventBus
    eventBus.subscribe('hierarchy:changed', () => {
      this.syncStores();
    });

    // Listen for focus requests
    eventBus.subscribe('focus:requested', (_event) => {
      // The focus handling is still delegated to the legacy callback system
      // but we can sync stores if needed for UI updates
      this.syncStores();
    });
  }

  /**
   * Sync Svelte stores with base class state - smart synchronization
   * Only updates stores if there are actual changes to prevent unnecessary re-renders
   */
  private syncStores(forceUpdate: boolean = false): void {
    // Don't sync during rapid typing to prevent cursor jumping
    if (this.isTyping && !forceUpdate) {
      return;
    }

    const currentNodes = super.nodes;
    const currentRootIds = super.rootNodeIds;

    // Check if nodes actually changed
    let nodesChanged = false;
    if (this.currentStoreNodes.size !== currentNodes.size) {
      nodesChanged = true;
    } else {
      // Check if any node references or content changed
      for (const [id, node] of currentNodes) {
        const storeNode = this.currentStoreNodes.get(id);
        if (!storeNode || storeNode !== node) {
          nodesChanged = true;
          break;
        }
      }
    }

    // Only update nodes store if there are changes
    if (nodesChanged || forceUpdate) {
      // Update our tracking map with current references
      this.currentStoreNodes.clear();
      for (const [id, node] of currentNodes) {
        this.currentStoreNodes.set(id, node);
      }
      // Create a completely new Map with cloned Node objects for Svelte reactivity
      const newNodeMap = new Map();
      for (const [id, node] of this.currentStoreNodes) {
        // Clone the node object to trigger Svelte reactivity
        newNodeMap.set(id, { ...node });
      }
      nodes.set(newNodeMap);
    }

    // Always update root node IDs as they're lightweight
    rootNodeIds.set([...currentRootIds]);
  }

  /**
   * Override to use base class directly - stores handle reactivity
   */
  get nodes(): Map<string, Node> {
    return super.nodes;
  }

  /**
   * Override to use base class directly - stores handle reactivity
   */
  get rootNodeIds(): string[] {
    return super.rootNodeIds;
  }

  /**
   * Override to use reactive collapsed nodes
   */
  get collapsedNodes(): Set<string> {
    return super.collapsedNodes;
  }

  /**
   * Override to sync stores after operations
   */
  initializeFromLegacyData(legacyNodes: unknown[]): void {
    super.initializeFromLegacyData(legacyNodes);
    this.syncStores();
  }

  /**
   * Override to sync stores after operations
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

    // Sync stores to trigger UI reactivity - force update for node creation
    this.syncStores(true);

    return result;
  }

  /**
   * Override to sync stores after operations
   */
  deleteNode(nodeId: string): void {
    super.deleteNode(nodeId);
    this.syncStores();
  }

  /**
   * Override to sync stores after operations
   * Indent/outdent affect multiple parent-child relationships, so invalidate cache
   */
  indentNode(nodeId: string): boolean {
    const result = super.indentNode(nodeId);
    if (result) {
      invalidateNodeCache(); // Clear cache for hierarchy changes
      this.syncStores();
    }
    return result;
  }

  /**
   * Override to sync stores after operations
   * Indent/outdent affect multiple parent-child relationships, so invalidate cache
   */
  outdentNode(nodeId: string): boolean {
    const result = super.outdentNode(nodeId);
    if (result) {
      invalidateNodeCache(); // Clear cache for hierarchy changes
      this.syncStores();
    }
    return result;
  }

  /**
   * Override to sync stores after operations
   */
  toggleExpanded(nodeId: string): boolean {
    const result = super.toggleExpanded(nodeId);
    if (result) {
      this.syncStores();
    }
    return result;
  }

  /**
   * Override to sync stores after operations
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    super.combineNodes(currentNodeId, previousNodeId);
    this.syncStores();
  }

  /**
   * Override to sync stores for content updates with typing optimization
   */
  updateNodeContent(nodeId: string, content: string): void {
    super.updateNodeContent(nodeId, content);

    // Mark as typing and delay store sync
    this.isTyping = true;
    if (this.typingTimer) {
      clearTimeout(this.typingTimer);
    }

    this.typingTimer = setTimeout(() => {
      this.isTyping = false;
      this.syncStores(true); // Force update after typing stops
    }, 150) as unknown as number; // Short delay for responsive feel

    // For now, still sync immediately but let the syncStores method decide
    this.syncStores();
  }

  /**
   * Force update node content - bypasses typing optimization
   * Used for operations like node splitting where immediate UI update is required
   */
  forceUpdateNodeContent(nodeId: string, content: string): void {
    super.updateNodeContent(nodeId, content);
    this.syncStores(true); // Force immediate update
  }

  /**
   * Method for UI components to signal when typing starts
   */
  startTyping(): void {
    this.isTyping = true;
    if (this.typingTimer) {
      clearTimeout(this.typingTimer);
    }
  }

  /**
   * Method for UI components to signal when typing stops
   */
  stopTyping(): void {
    this.isTyping = false;
    if (this.typingTimer) {
      clearTimeout(this.typingTimer);
    }
    this.syncStores(true); // Force update
  }

  /**
   * Override to sync stores for collapsed state changes
   */
  toggleCollapsed(nodeId: string): boolean {
    const result = super.toggleCollapsed(nodeId);
    this.syncStores();
    return result;
  }

  /**
   * Override to use base class directly
   */
  isNodeCollapsed(nodeId: string): boolean {
    return super.isNodeCollapsed(nodeId);
  }

  /**
   * Type guard to check if an event has a nodeId property
   */
  private hasNodeId(
    event: import('./eventTypes').NodeSpaceEvent
  ): event is import('./eventTypes').NodeSpaceEvent & { nodeId: string } {
    return (
      'nodeId' in event && typeof (event as unknown as { nodeId: unknown }).nodeId === 'string'
    );
  }
}
