/**
 * Node Expansion Coordinator
 *
 * Bridges TabPersistenceService and ReactiveNodeService to persist/restore
 * node expansion states across application restarts.
 *
 * Architecture:
 * - Maintains a registry of ReactiveNodeService instances (one per viewer/tab)
 * - Extracts expanded node IDs from services for persistence
 * - Restores expansion states when tabs are loaded
 * - Supports deferred restoration (register restoration before service exists)
 *
 * Usage:
 * 1. ReactiveNodeService instances register on creation
 * 2. TabPersistenceService extracts expansion states during save
 * 3. App shell schedules restoration during load
 * 4. Restoration happens automatically when service registers
 */

import type { ReactiveNodeService } from './reactive-node-service.svelte';

/**
 * Registry entry for a ReactiveNodeService instance
 */
interface ViewerRegistryEntry {
  service: ReactiveNodeService;
  viewerId: string;
}

/**
 * Coordinator service for node expansion state persistence
 */
export class NodeExpansionCoordinator {
  private static readonly LOG_PREFIX = '[NodeExpansionCoordinator]';
  private static readonly DEBUG = import.meta.env.DEV;

  /**
   * Map of viewer IDs to ReactiveNodeService instances
   * Key: viewerId (typically tab.id)
   * Value: Service instance
   */
  private static viewerRegistry = new Map<string, ViewerRegistryEntry>();

  /**
   * Map of pending restoration requests
   * Key: viewerId
   * Value: Array of node IDs to expand
   */
  private static pendingRestorations = new Map<string, string[]>();

  /**
   * Log message in development mode only
   */
  private static log(message: string): void {
    if (this.DEBUG) {
      console.log(`${this.LOG_PREFIX} ${message}`);
    }
  }

  /**
   * Log warning in all environments
   */
  private static warn(message: string): void {
    console.warn(`${this.LOG_PREFIX} ${message}`);
  }

  /**
   * Log error in all environments
   */
  private static error(message: string, error?: unknown): void {
    console.error(`${this.LOG_PREFIX} ${message}`, error || '');
  }

  /**
   * Register a ReactiveNodeService instance for a viewer
   * Called when a new tab/viewer is created
   *
   * If there are pending restoration requests for this viewer,
   * they will be applied automatically.
   *
   * @param viewerId - Unique identifier for the viewer (typically tab.id)
   * @param service - The ReactiveNodeService instance
   */
  static registerViewer(viewerId: string, service: ReactiveNodeService): void {
    this.log(`Registering viewer: ${viewerId}`);

    this.viewerRegistry.set(viewerId, { service, viewerId });

    // Check if there are pending expansion states to restore
    const pendingStates = this.pendingRestorations.get(viewerId);
    if (pendingStates && pendingStates.length > 0) {
      this.log(`Applying ${pendingStates.length} pending expansions for viewer: ${viewerId}`);
      this.restoreExpansionStates(viewerId, pendingStates);
      this.pendingRestorations.delete(viewerId);
    }
  }

  /**
   * Unregister a viewer when tab is closed
   * Cleans up registry to prevent memory leaks
   *
   * @param viewerId - The viewer ID to unregister
   */
  static unregisterViewer(viewerId: string): void {
    this.log(`Unregistering viewer: ${viewerId}`);
    this.viewerRegistry.delete(viewerId);
    // Also clean up any pending restorations that weren't applied
    this.pendingRestorations.delete(viewerId);
  }

  /**
   * Extract expanded node IDs from a viewer
   * Returns sparse array of only expanded nodes (collapsed is default state)
   *
   * @param viewerId - The viewer ID to extract from
   * @returns Array of node IDs that are expanded, or empty array if viewer not found
   */
  static getExpandedNodeIds(viewerId: string): string[] {
    const entry = this.viewerRegistry.get(viewerId);
    if (!entry) {
      this.warn(`Viewer not found for extraction: ${viewerId}`);
      return [];
    }

    const { service } = entry;
    const expandedIds: string[] = [];

    // Get all nodes from the service
    // Note: service.nodes returns Map entries [id, Node][], so we need to destructure
    const nodes = service.nodes;

    // Check expansion state for each node
    for (const [nodeId] of nodes) {
      const uiState = service.getUIState(nodeId);
      if (uiState?.expanded) {
        expandedIds.push(nodeId);
      }
    }

    this.log(`Extracted ${expandedIds.length} expanded nodes from viewer: ${viewerId}`);
    return expandedIds;
  }

  /**
   * Restore expansion states to a viewer
   * Marks specified nodes as expanded
   *
   * If the viewer hasn't been registered yet, the restoration
   * will be queued and applied when the viewer registers.
   *
   * @param viewerId - The viewer ID to restore to
   * @param expandedNodeIds - Array of node IDs to mark as expanded
   */
  static restoreExpansionStates(viewerId: string, expandedNodeIds: string[]): void {
    // Handle empty array - still consume the restoration request
    if (expandedNodeIds.length === 0) {
      // Clear any pending restorations for this viewer
      this.pendingRestorations.delete(viewerId);
      return;
    }

    const entry = this.viewerRegistry.get(viewerId);

    if (!entry) {
      // Viewer not registered yet - queue the restoration
      this.log(
        `Viewer not yet registered, queueing ${expandedNodeIds.length} expansions: ${viewerId}`
      );
      this.pendingRestorations.set(viewerId, expandedNodeIds);
      return;
    }

    const { service } = entry;
    let appliedCount = 0;
    let skippedCount = 0;

    // Apply expansion states to each node
    for (const nodeId of expandedNodeIds) {
      const uiState = service.getUIState(nodeId);
      if (uiState) {
        // Node exists - toggle it to expanded state
        service.toggleExpanded(nodeId);
        appliedCount++;
      } else {
        // Node doesn't exist (may have been deleted since last session)
        skippedCount++;
      }
    }

    this.log(
      `Restored expansions for viewer ${viewerId}: ${appliedCount} applied, ${skippedCount} skipped`
    );
  }

  /**
   * Schedule a restoration for a viewer that hasn't been registered yet
   * This is useful when loading tab state before viewers are mounted
   *
   * @param viewerId - The viewer ID to schedule restoration for
   * @param expandedNodeIds - Array of node IDs to mark as expanded
   */
  static scheduleRestoration(viewerId: string, expandedNodeIds: string[]): void {
    this.log(`Scheduling ${expandedNodeIds.length} expansions for viewer: ${viewerId}`);
    this.pendingRestorations.set(viewerId, expandedNodeIds);
  }

  /**
   * Clear all registrations and pending restorations
   * Useful for testing or application reset
   */
  static clear(): void {
    this.log('Clearing all viewer registrations and pending restorations');
    this.viewerRegistry.clear();
    this.pendingRestorations.clear();
  }

  /**
   * Get the number of registered viewers
   * Useful for debugging and testing
   */
  static getRegisteredViewerCount(): number {
    return this.viewerRegistry.size;
  }

  /**
   * Get the number of pending restorations
   * Useful for debugging and testing
   */
  static getPendingRestorationCount(): number {
    return this.pendingRestorations.size;
  }
}
