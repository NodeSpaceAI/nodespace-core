/**
 * Scroll State Store
 *
 * Manages scroll positions for viewers across different tabs, panes, and nodes.
 * Each viewer is identified by a composite key: `${nodeId}-${tabId}-${paneId}`
 *
 * **Terminology:**
 * - **Pane**: A container in the split-pane layout (e.g., left pane, right pane)
 * - **Tab**: A browser-like tab within a pane
 * - **Node**: The document/node being viewed
 * - **Viewer**: A BaseNodeViewer component instance (identified by node + tab + pane combination)
 * - **Viewer ID**: Unique identifier for a viewer instance: `${nodeId}-${tabId}-${paneId}`
 *
 * This ensures:
 * - Independent scroll positions per node+tab+pane combination
 * - Scroll state preserved when switching tabs
 * - Scroll state preserved when switching between nodes in the same tab
 * - Same node in different tabs/panes maintains separate scroll positions
 */

// Map from viewer ID to scroll position
const scrollPositions = new Map<string, number>();

/**
 * Generate unique viewer ID from node, tab, and pane IDs
 *
 * A "viewer" is a BaseNodeViewer component instance displaying a specific
 * node within a specific tab and pane. Each node+tab+pane combination maintains
 * independent scroll state, ensuring that navigating to a different node
 * within the same tab starts at the top of the document.
 *
 * @param nodeId - The node/document identifier being viewed
 * @param tabId - The tab identifier
 * @param paneId - The pane/container identifier
 * @returns Composite viewer ID in format `${nodeId}-${tabId}-${paneId}`
 *
 * @example
 * ```typescript
 * // Same node in two different panes
 * const leftViewerId = getViewerId('node-abc', 'tab-1', 'pane-left');
 * const rightViewerId = getViewerId('node-abc', 'tab-1', 'pane-right');
 * // Each viewer maintains independent scroll position
 * ```
 */
export function getViewerId(nodeId: string, tabId: string, paneId: string): string {
  return `${nodeId}-${tabId}-${paneId}`;
}

/**
 * Save scroll position for a viewer
 * @param viewerId - Composite viewer ID (from getViewerId)
 * @param position - Scroll position in pixels
 */
export function saveScrollPosition(viewerId: string, position: number): void {
  scrollPositions.set(viewerId, position);
}

/**
 * Get saved scroll position for a viewer
 * @param viewerId - Composite viewer ID (from getViewerId)
 * @returns Saved scroll position or 0 if none exists
 */
export function getScrollPosition(viewerId: string): number {
  return scrollPositions.get(viewerId) ?? 0;
}

/**
 * Clear scroll position for a viewer (e.g., when tab is closed)
 * @param viewerId - Composite viewer ID (from getViewerId)
 */
export function clearScrollPosition(viewerId: string): void {
  scrollPositions.delete(viewerId);
}

/**
 * Clear all scroll positions for a pane (when pane is closed)
 * @param paneId - Pane ID
 */
export function clearPaneScrollPositions(paneId: string): void {
  // Find all viewer IDs that belong to this pane
  const viewerIds = Array.from(scrollPositions.keys()).filter((id) => id.endsWith(`-${paneId}`));

  // Delete each one
  viewerIds.forEach((id) => scrollPositions.delete(id));
}

/**
 * Get the current size of the scroll state map (for monitoring and debugging)
 * Useful for detecting memory leaks or validating cleanup logic
 * @returns Number of stored scroll positions
 */
export function getScrollStateSize(): number {
  return scrollPositions.size;
}

/**
 * Get all viewer IDs currently tracked (for debugging)
 * Only available in development mode
 * @returns Array of viewer IDs
 */
export function getScrollStateKeys(): string[] {
  if (import.meta.env.DEV) {
    return Array.from(scrollPositions.keys());
  }
  return [];
}

/**
 * Clear all scroll positions (for testing only)
 * WARNING: Only use this in test cleanup, not in production code
 */
export function clearAllScrollPositions(): void {
  if (import.meta.env.TEST || import.meta.env.DEV) {
    scrollPositions.clear();
  }
}
