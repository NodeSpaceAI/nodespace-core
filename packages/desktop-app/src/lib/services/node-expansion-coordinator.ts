/**
 * Node Expansion Coordinator
 *
 * Bridges TabPersistenceService and ReactiveNodeService to persist/restore
 * node expansion states across application restarts.
 *
 * ## Architecture
 * - Maintains a registry of ReactiveNodeService instances (one per viewer/tab)
 * - Extracts expanded node IDs from services for persistence
 * - Restores expansion states when tabs are loaded
 * - Supports deferred restoration (register restoration before service exists)
 *
 * ## Lifecycle & Timing
 *
 * **Application Startup (Loading State):**
 * ```
 * 1. App loads → TabPersistenceService.load()
 *    └─ Returns: { tabs: [{ id, expandedNodeIds: [...] }] }
 *
 * 2. Navigation store processes loaded state
 *    └─ Calls: scheduleRestoration(tabId, expandedNodeIds)
 *    └─ State: PENDING (viewer not mounted yet)
 *
 * 3. BaseNodeViewer mounts (onMount)
 *    └─ Calls: registerViewer(tabId, service)
 *    └─ Triggers: Automatic restoration of pending states
 *    └─ State: APPLIED (nodes expanded via batchSetExpanded)
 * ```
 *
 * **Application Shutdown (Saving State):**
 * ```
 * 1. User expands/collapses nodes
 *    └─ ReactiveNodeService updates UIState.expanded
 *
 * 2. Navigation store subscription fires (debounced 500ms)
 *    └─ Calls: getExpandedNodeIds(tabId) for each tab
 *    └─ Returns: ['node-1', 'node-3'] (sparse - only expanded)
 *
 * 3. TabPersistenceService.save({ tabs: [...] })
 *    └─ Persists to localStorage with expandedNodeIds
 * ```
 *
 * ## Deferred Restoration Pattern
 *
 * The coordinator handles the timing mismatch between:
 * - Tab state loading (happens early, during app initialization)
 * - Viewer mounting (happens later, when components render)
 *
 * When `scheduleRestoration()` is called before `registerViewer()`:
 * 1. Restoration request is queued in `pendingRestorations` map
 * 2. When viewer eventually registers, pending request is auto-applied
 * 3. Queue entry is removed after application
 *
 * This ensures restoration works regardless of timing.
 *
 * ## Memory Management
 *
 * - Registry tracks registration timestamp for leak detection
 * - Warns if registry size exceeds MAX_REGISTRY_SIZE (100)
 * - Provides cleanupStaleEntries() for periodic cleanup
 * - Always call unregisterViewer() in onDestroy lifecycle
 *
 * ## Usage Example
 *
 * ```typescript
 * // In BaseNodeViewer.svelte
 * onMount(() => {
 *   NodeExpansionCoordinator.registerViewer(tabId, nodeManager);
 * });
 *
 * onDestroy(() => {
 *   NodeExpansionCoordinator.unregisterViewer(tabId);
 * });
 *
 * // In navigation.ts (load)
 * for (const tab of loadedTabs) {
 *   if (tab.expandedNodeIds?.length > 0) {
 *     NodeExpansionCoordinator.scheduleRestoration(tab.id, tab.expandedNodeIds);
 *   }
 * }
 *
 * // In navigation.ts (save)
 * const enrichedTabs = tabs.map(tab => ({
 *   ...tab,
 *   expandedNodeIds: NodeExpansionCoordinator.getExpandedNodeIds(tab.id)
 * }));
 * ```
 */

import type { ReactiveNodeService } from './reactive-node-service.svelte';

/**
 * Registry entry for a ReactiveNodeService instance
 */
interface ViewerRegistryEntry {
  service: ReactiveNodeService;
  registeredAt: number; // Timestamp for leak detection
}

/**
 * Coordinator service for node expansion state persistence
 */
export class NodeExpansionCoordinator {
  private static readonly LOG_PREFIX = '[NodeExpansionCoordinator]';
  private static readonly DEBUG = import.meta.env.DEV;

  // Memory leak prevention: Warn if registry grows too large
  private static readonly MAX_REGISTRY_SIZE = 100;
  private static readonly STALE_ENTRY_AGE_MS = 30 * 60 * 1000; // 30 minutes

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

    this.viewerRegistry.set(viewerId, {
      service,
      registeredAt: Date.now()
    });

    // Memory leak prevention: Warn if registry is growing too large
    this.checkRegistrySize();

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
   * Marks specified nodes as expanded using batch update for efficiency
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

    // Filter to only nodes that exist, and prepare batch update
    const updates = expandedNodeIds
      .filter((nodeId) => service.getUIState(nodeId) !== undefined)
      .map((nodeId) => ({ nodeId, expanded: true }));

    // Use batch API for better performance
    const changedCount = service.batchSetExpanded(updates);
    const skippedCount = expandedNodeIds.length - updates.length;

    this.log(
      `Restored expansions for viewer ${viewerId}: ${changedCount} applied, ${skippedCount} skipped (missing nodes)`
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

  /**
   * Check registry size and warn if it's growing too large
   * Helps detect memory leaks from missing unregister calls
   * @private
   */
  private static checkRegistrySize(): void {
    const size = this.viewerRegistry.size;

    if (size > this.MAX_REGISTRY_SIZE) {
      this.warn(
        `Registry size (${size}) exceeds maximum (${this.MAX_REGISTRY_SIZE}). ` +
          'This may indicate a memory leak from missing unregisterViewer() calls. ' +
          'Consider calling cleanupStaleEntries().'
      );
    }
  }

  /**
   * Remove stale entries that have been registered for too long
   * Useful for preventing memory leaks in long-running applications
   * Call this periodically if you suspect cleanup issues
   *
   * @param maxAgeMs - Maximum age in milliseconds (default: 30 minutes)
   * @returns Number of stale entries removed
   */
  static cleanupStaleEntries(maxAgeMs: number = this.STALE_ENTRY_AGE_MS): number {
    const now = Date.now();
    let removedCount = 0;

    for (const [viewerId, entry] of this.viewerRegistry.entries()) {
      const age = now - entry.registeredAt;
      if (age > maxAgeMs) {
        this.warn(`Removing stale entry for viewer ${viewerId} (age: ${Math.round(age / 1000)}s)`);
        this.viewerRegistry.delete(viewerId);
        this.pendingRestorations.delete(viewerId);
        removedCount++;
      }
    }

    if (removedCount > 0) {
      this.log(`Cleaned up ${removedCount} stale entries`);
    }

    return removedCount;
  }
}
