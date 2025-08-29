/**
 * ReactiveNodeManager - Svelte store-based reactive wrapper for NodeManager
 *
 * This file provides Svelte store reactivity for the NodeManager service.
 * Uses writable stores for proper reactivity outside of .svelte components.
 */

import { NodeManager, type NodeManagerEvents, type Node } from './nodeManager';
import { eventBus } from './eventBus';
import { nodes, rootNodeIds } from './nodeStore';

export class ReactiveNodeManager extends NodeManager {
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
   * Sync Svelte stores with base class state
   */
  private syncStores(): void {
    // Update stores with current base class state
    nodes.set(new Map(super.nodes));
    rootNodeIds.set([...super.rootNodeIds]);
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

    // Sync stores to trigger UI reactivity
    this.syncStores();

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
   */
  indentNode(nodeId: string): boolean {
    const result = super.indentNode(nodeId);
    if (result) {
      this.syncStores();
    }
    return result;
  }

  /**
   * Override to sync stores after operations
   */
  outdentNode(nodeId: string): boolean {
    const result = super.outdentNode(nodeId);
    if (result) {
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
   * Override to sync stores for content updates
   */
  updateNodeContent(nodeId: string, content: string): void {
    super.updateNodeContent(nodeId, content);
    this.syncStores();
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
