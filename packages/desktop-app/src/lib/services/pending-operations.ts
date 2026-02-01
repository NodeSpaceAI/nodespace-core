/**
 * Pending Operations Tracker
 *
 * Tracks pending move operations to prevent race conditions between
 * indent/outdent and content updates. This is in a separate module
 * to avoid circular dependencies between SharedNodeStore and ReactiveNodeService.
 */

// Track pending move operations to prevent race conditions
// When indent/outdent fires a moveNodeCommand, subsequent operations must wait for it
const pendingMoveOperations: Map<string, Promise<void>> = new Map();

/**
 * Wait for all pending move operations to complete before starting a new hierarchy change.
 * This prevents "Sibling not found" errors during rapid Enter+Tab+Shift+Tab sequences.
 *
 * Race condition scenario:
 * 1. User indents B under A (fires moveNodeCommand async)
 * 2. User immediately outdents C (which references B as insertAfterNodeId)
 * 3. If step 1's moveNodeCommand hasn't completed, edge A→B doesn't exist yet
 * 4. Backend fails with "Sibling not found: B" because B isn't in A's has_child edges
 */
export async function waitForPendingMoveOperations(): Promise<void> {
  if (pendingMoveOperations.size === 0) return;
  await Promise.all(pendingMoveOperations.values());
}

/**
 * Track a move operation and automatically clean up when done
 */
export function trackMoveOperation(nodeId: string, operation: Promise<void>): Promise<void> {
  console.debug(`[PendingOps] Tracking move for ${nodeId.substring(0, 8)}`);
  const trackedPromise = operation.finally(() => {
    console.debug(`[PendingOps] Move completed for ${nodeId.substring(0, 8)}`);
    pendingMoveOperations.delete(nodeId);
  });
  pendingMoveOperations.set(nodeId, trackedPromise);
  return trackedPromise;
}

/**
 * Get the pending move operation for a specific node (if any).
 * Used by PersistenceCoordinator to wait for moves before updates.
 */
export function getPendingMoveOperation(nodeId: string): Promise<void> | undefined {
  const op = pendingMoveOperations.get(nodeId);
  console.debug(`[PendingOps] getPendingMoveOperation(${nodeId.substring(0, 8)}): ${op ? 'FOUND' : 'none'}`);
  return op;
}
