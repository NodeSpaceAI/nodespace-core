/**
 * Node Expansion Coordinator
 *
 * Bridges TabPersistenceService and ReactiveNodeService to persist/restore
 * node expansion states across application restarts.
 *
 * ## Architecture
 * - Maintains a registry of ReactiveNodeService instances (one per viewer/tab)
 * - Extracts collapsed node IDs from services for persistence (viewers load all
 *   nodes expanded, so the collapsed ids are the sparse exceptions)
 * - Restores collapsed states when tabs are loaded
 * - Supports deferred restoration (register restoration before service exists)
 *
 * ## Lifecycle & Timing
 *
 * **Application Startup (Loading State):**
 * ```
 * 1. App loads → TabPersistenceService.load()
 *    └─ Returns: { tabs: [{ id, collapsedNodeIds: [...] }] }
 *
 * 2. Navigation store processes loaded state
 *    └─ Calls: scheduleRestoration(tabId, collapsedNodeIds)
 *    └─ State: PENDING (viewer not mounted yet)
 *
 * 3. BaseNodeViewer mounts (onMount)
 *    └─ Calls: registerViewer(tabId, service)
 *    └─ Triggers: Automatic restoration of pending states
 *    └─ State: APPLIED (nodes collapsed via batchSetExpanded)
 * ```
 *
 * **Application Shutdown (Saving State):**
 * ```
 * 1. User expands/collapses nodes
 *    └─ ReactiveNodeService updates UIState.expanded
 *
 * 2. Navigation store subscription fires (debounced 500ms)
 *    └─ Calls: getCollapsedNodeIds(tabId) for each tab
 *    └─ Returns: ['node-1', 'node-3'] (sparse - only collapsed)
 *
 * 3. TabPersistenceService.save({ tabs: [...] })
 *    └─ Persists to localStorage with collapsedNodeIds
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
 *   if (tab.collapsedNodeIds?.length > 0) {
 *     NodeExpansionCoordinator.scheduleRestoration(tab.id, tab.collapsedNodeIds);
 *   }
 * }
 *
 * // In navigation.ts (save)
 * const enrichedTabs = tabs.map(tab => ({
 *   ...tab,
 *   collapsedNodeIds: NodeExpansionCoordinator.getCollapsedNodeIds(tab.id)
 * }));
 * ```
 */

import type { ReactiveNodeService } from './reactive-node-service.svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('NodeExpansionCoordinator');

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
   * Value: Array of node IDs to collapse
   */
  private static pendingRestorations = new Map<string, string[]>();

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
    // Silently register viewer (no log spam)

    this.viewerRegistry.set(viewerId, {
      service,
      registeredAt: Date.now()
    });

    // Memory leak prevention: Warn if registry is growing too large
    this.checkRegistrySize();

    // Check if there are pending expansion states to restore
    const pendingStates = this.pendingRestorations.get(viewerId);
    if (pendingStates && pendingStates.length > 0) {
      log.debug(`Restoring ${pendingStates.length} collapsed nodes for viewer: ${viewerId}`);
      this.restoreCollapsedStates(viewerId, pendingStates);
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
    // Silently unregister viewer (no log spam)
    this.viewerRegistry.delete(viewerId);
    // Also clean up any pending restorations that weren't applied
    this.pendingRestorations.delete(viewerId);
  }

  /**
   * Extract collapsed node IDs from a viewer
   *
   * Viewers load every node expanded, so only the exceptions are persisted: the
   * returned sparse array lists the nodes the user collapsed. A node that is
   * missing from the list (including a node created later) is expanded.
   *
   * While a viewer has not registered yet, its pending restoration is returned
   * unchanged so that a save before the viewer mounts cannot drop saved state.
   *
   * @param viewerId - The viewer ID to extract from
   * @returns Array of node IDs that are collapsed
   */
  static getCollapsedNodeIds(viewerId: string): string[] {
    const entry = this.viewerRegistry.get(viewerId);
    if (!entry) {
      // Normal during app startup before viewers mount
      return [...(this.pendingRestorations.get(viewerId) ?? [])];
    }

    const { service } = entry;
    const collapsedIds: string[] = [];

    // Note: service.nodes returns Map entries [id, Node][], so we need to destructure
    for (const [nodeId] of service.nodes) {
      if (service.hasUIState(nodeId) && service.getUIState(nodeId)?.expanded === false) {
        collapsedIds.push(nodeId);
      }
    }

    return collapsedIds;
  }

  /**
   * Restore collapsed states to a viewer
   * Marks specified nodes as collapsed using batch update for efficiency. Every
   * other node keeps the viewer's default (expanded).
   *
   * If the viewer hasn't been registered yet, the restoration
   * will be queued and applied when the viewer registers.
   *
   * @param viewerId - The viewer ID to restore to
   * @param collapsedNodeIds - Array of node IDs to mark as collapsed
   */
  static restoreCollapsedStates(viewerId: string, collapsedNodeIds: string[]): void {
    // Nothing is collapsed: the default already applies. Still consume the request.
    if (collapsedNodeIds.length === 0) {
      this.pendingRestorations.delete(viewerId);
      return;
    }

    const entry = this.viewerRegistry.get(viewerId);

    if (!entry) {
      // Viewer not registered yet - queue the restoration
      log.debug(
        `Viewer not yet registered, queueing ${collapsedNodeIds.length} collapsed nodes: ${viewerId}`
      );
      this.pendingRestorations.set(viewerId, collapsedNodeIds);
      return;
    }

    const { service } = entry;

    // Filter to only nodes that exist, and prepare batch update
    const updates = collapsedNodeIds
      .filter((nodeId) => service.hasUIState(nodeId))
      .map((nodeId) => ({ nodeId, expanded: false }));

    // Use batch API for better performance
    const changedCount = service.batchSetExpanded(updates);
    const skippedCount = collapsedNodeIds.length - updates.length;

    log.debug(
      `Restored collapsed nodes for viewer ${viewerId}: ${changedCount} applied, ${skippedCount} skipped (missing nodes)`
    );
  }

  /**
   * Schedule a restoration for a viewer that hasn't been registered yet
   * This is useful when loading tab state before viewers are mounted
   *
   * @param viewerId - The viewer ID to schedule restoration for
   * @param collapsedNodeIds - Array of node IDs to mark as collapsed
   */
  static scheduleRestoration(viewerId: string, collapsedNodeIds: string[]): void {
    log.debug(`Scheduling ${collapsedNodeIds.length} collapsed nodes for viewer: ${viewerId}`);
    this.pendingRestorations.set(viewerId, collapsedNodeIds);
  }

  /**
   * Clear all registrations and pending restorations
   * Useful for testing or application reset
   */
  static clear(): void {
    log.debug('Clearing all viewer registrations and pending restorations');
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
      log.warn(
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
        log.warn(`Removing stale entry for viewer ${viewerId} (age: ${Math.round(age / 1000)}s)`);
        this.viewerRegistry.delete(viewerId);
        this.pendingRestorations.delete(viewerId);
        removedCount++;
      }
    }

    if (removedCount > 0) {
      log.debug(`Cleaned up ${removedCount} stale entries`);
    }

    return removedCount;
  }
}
