/**
 * Scroll State Store
 *
 * Manages scroll positions for viewers across different tabs and panes.
 * Each viewer is identified by a composite key: `${tabId}-${paneId}`
 *
 * **Terminology:**
 * - **Pane**: A container in the split-pane layout (e.g., left pane, right pane)
 * - **Tab**: A document/node being viewed (can appear in multiple panes)
 * - **Viewer**: A BaseNodeViewer component instance (identified by tab + pane combination)
 * - **Viewer ID**: Unique identifier for a viewer instance: `${tabId}-${paneId}`
 *
 * This ensures:
 * - Independent scroll positions per viewer instance
 * - Scroll state preserved when switching tabs
 * - Scroll state preserved when switching panes
 * - Same node in different tabs/panes maintains separate scroll positions
 */

// Map from viewer ID to scroll position
const scrollPositions = new Map<string, number>();

/**
 * Generate unique viewer ID from tab and pane IDs
 *
 * A "viewer" is a BaseNodeViewer component instance displaying a specific
 * tab (document/node) within a specific pane (split-pane container).
 * The same tab can appear in multiple panes simultaneously, each maintaining
 * independent scroll state.
 *
 * @param tabId - The tab/document identifier
 * @param paneId - The pane/container identifier
 * @returns Composite viewer ID in format `${tabId}-${paneId}`
 *
 * @example
 * ```typescript
 * // Same tab in two different panes
 * const leftViewerId = getViewerId('doc-123', 'pane-left');  // "doc-123-pane-left"
 * const rightViewerId = getViewerId('doc-123', 'pane-right'); // "doc-123-pane-right"
 * // Each viewer maintains independent scroll position
 * ```
 */
export function getViewerId(tabId: string, paneId: string): string {
  return `${tabId}-${paneId}`;
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
