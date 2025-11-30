/**
 * OptimisticOperationManager - Fire-and-Forget with Rollback
 *
 * Implements optimistic UI updates for structural operations with automatic
 * rollback on failure. This enables instant perceived latency (<10ms) while
 * ensuring consistency through backend confirmation via domain events.
 *
 * Architecture:
 * 1. Take snapshot of current state (structure + data)
 * 2. Apply optimistic change immediately (UI updates instantly)
 * 3. Fire backend operation (don't await - let domain events confirm)
 * 4. On error: Rollback to snapshot + emit error event for UI notification
 *
 * Features:
 * - <10ms perceived latency for structural changes
 * - Automatic rollback on backend failure
 * - Error event emission with retry capability
 * - Snapshot/restore for both structure and data
 *
 * See: docs/architecture/development/hierarchy-reactivity-architecture-review.md#5-fire-and-forget-with-rollback
 */

import { emit } from '@tauri-apps/api/event';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { PersistenceFailedEvent } from '$lib/types/event-types';
import type { Node } from '$lib/types';

// Development-only logging helper
const isDev = import.meta.env.DEV;
const log = (message: string, data?: unknown) => {
  if (isDev) {
    console.log(message, data ?? '');
  }
};
const logError = (message: string, error?: unknown) => {
  // Always log errors, but with less detail in production
  if (isDev) {
    console.error(message, error);
  } else {
    console.error(message);
  }
};

interface OperationSnapshot {
  structure: Map<string, Array<{ nodeId: string; order: number }>>;
  data: Map<string, Node>;
  timestamp: number;
}

interface OperationOptions {
  /**
   * Human-readable description of the operation (e.g., "indent node", "create node")
   * Used in error messages and logging
   */
  description: string;

  /**
   * Optional list of node IDs affected by this operation
   * Used for error reporting and debugging
   */
  affectedNodes?: string[];

  /**
   * Whether to take a data snapshot in addition to structure snapshot
   * Default: false (structure-only operations don't need data snapshot)
   */
  snapshotData?: boolean;
}

class OptimisticOperationManager {
  /**
   * Execute a structural change with optimistic update and rollback on failure
   *
   * Flow:
   * 1. Take snapshot (structure + optionally data)
   * 2. Execute optimistic update (UI changes immediately)
   * 3. Fire backend operation (async, don't await)
   * 4. Backend success → domain events confirms change (UI already updated)
   * 5. Backend failure → Rollback to snapshot + emit error event
   *
   * @param optimisticUpdate - Function that updates local state immediately
   * @param backendOperation - Async function that persists to backend (fire-and-forget)
   * @param options - Operation metadata for error reporting and snapshots
   *
   * @example
   * ```typescript
   * await operationManager.executeStructuralChange(
   *   () => {
   *     // Optimistic: Update local structure immediately
   *     structureTree.__testOnly_addChild({
   *       in: parentId,
   *       out: childId,
   *       order: 1.5
   *     });
   *   },
   *   async () => {
   *     // Backend: Persist (fire-and-forget)
   *     await backend.moveNode(childId, parentId);
   *   },
   *   {
   *     description: 'indent node',
   *     affectedNodes: [childId, parentId]
   *   }
   * );
   * ```
   */
  async executeStructuralChange(
    optimisticUpdate: () => void,
    backendOperation: () => Promise<void>,
    options: OperationOptions
  ): Promise<void> {
    const { description, affectedNodes = [], snapshotData = false } = options;

    // Step 1: Take snapshot for rollback
    const snapshot = this.takeSnapshot(snapshotData);

    log(`[OptimisticOperationManager] Starting operation: ${description}`, {
      affectedNodes,
      snapshotData,
      timestamp: snapshot.timestamp
    });

    try {
      // Step 2: Apply optimistic change immediately (UI updates)
      optimisticUpdate();

      log(`[OptimisticOperationManager] Optimistic update applied: ${description}`);

      // Step 3: Fire backend operation (don't await - let domain events confirm)
      // We intentionally don't await here so the UI feels instant
      // domain events will confirm success and sync state across clients
      backendOperation().catch((error) => {
        // Backend failed - rollback and notify
        this.handleBackendFailure(error, snapshot, description, affectedNodes);
      });
    } catch (error) {
      // Optimistic update failed (should be rare) - rollback immediately
      logError(
        `[OptimisticOperationManager] Optimistic update failed: ${description}`,
        error
      );
      this.restoreSnapshot(snapshot, snapshotData);
      throw error;
    }
  }

  /**
   * Take snapshot of current state for rollback
   * @private
   */
  private takeSnapshot(includeData: boolean): OperationSnapshot {
    const snapshot: OperationSnapshot = {
      structure: structureTree.snapshot(),
      data: includeData ? sharedNodeStore.snapshot() : new Map(),
      timestamp: Date.now()
    };

    return snapshot;
  }

  /**
   * Restore state from snapshot (rollback)
   * @private
   */
  private restoreSnapshot(snapshot: OperationSnapshot, includeData: boolean): void {
    log('[OptimisticOperationManager] Rolling back to snapshot', {
      timestamp: snapshot.timestamp,
      includeData
    });

    // Restore structure
    structureTree.restore(snapshot.structure);

    // Restore data if snapshot included it
    if (includeData) {
      sharedNodeStore.restore(snapshot.data);
    }

    log('[OptimisticOperationManager] Rollback complete');
  }

  /**
   * Handle backend operation failure
   * Rollback state and emit error event for UI notification
   * @private
   */
  private handleBackendFailure(
    error: unknown,
    snapshot: OperationSnapshot,
    description: string,
    affectedNodes: string[]
  ): void {
    logError(
      `[OptimisticOperationManager] Backend operation failed: ${description}`,
      error
    );

    // Rollback to snapshot
    const includeData = snapshot.data.size > 0;
    this.restoreSnapshot(snapshot, includeData);

    // Emit error event for UI notification
    this.emitErrorEvent(error, description, affectedNodes);
  }

  /**
   * Emit error event via Tauri for UI notification
   * UI can display toast with retry option
   * @private
   */
  private async emitErrorEvent(
    error: unknown,
    description: string,
    affectedNodes: string[]
  ): Promise<void> {
    const errorMessage =
      error instanceof Error ? error.message : 'Unknown error occurred';

    const event: PersistenceFailedEvent = {
      type: 'error:persistence-failed',
      namespace: 'error',
      timestamp: Date.now(),
      source: 'optimistic-operation-manager',
      message: `Failed to ${description}`,
      failedNodeIds: affectedNodes,
      failureReason: this.categorizeError(error),
      canRetry: true,
      affectedOperations: affectedNodes.map((nodeId) => ({
        nodeId,
        operation: 'update',
        error: errorMessage
      })),
      metadata: {
        originalError: errorMessage,
        description
      }
    };

    try {
      await emit('error:persistence-failed', event);
      log('[OptimisticOperationManager] Error event emitted', event);
    } catch (emitError) {
      logError(
        '[OptimisticOperationManager] Failed to emit error event',
        emitError
      );
    }
  }

  /**
   * Categorize error for better error handling
   * @private
   */
  private categorizeError(
    error: unknown
  ): 'timeout' | 'foreign-key-constraint' | 'database-locked' | 'unknown' {
    const errorMessage =
      error instanceof Error ? error.message : String(error);

    if (errorMessage.includes('timeout') || errorMessage.includes('timed out')) {
      return 'timeout';
    }
    if (
      errorMessage.includes('foreign key') ||
      errorMessage.includes('constraint')
    ) {
      return 'foreign-key-constraint';
    }
    if (errorMessage.includes('locked') || errorMessage.includes('busy')) {
      return 'database-locked';
    }

    return 'unknown';
  }

  /**
   * Execute multiple operations as a batch with shared snapshot
   * If any operation fails, all operations are rolled back
   *
   * @param operations - Array of operation definitions
   *
   * @example
   * ```typescript
   * await operationManager.executeBatch([
   *   {
   *     optimisticUpdate: () => moveNode(child1, newParent),
   *     backendOperation: async () => backend.moveNode(child1, newParent),
   *     description: 'move child 1'
   *   },
   *   {
   *     optimisticUpdate: () => moveNode(child2, newParent),
   *     backendOperation: async () => backend.moveNode(child2, newParent),
   *     description: 'move child 2'
   *   }
   * ]);
   * ```
   */
  async executeBatch(
    operations: Array<{
      optimisticUpdate: () => void;
      backendOperation: () => Promise<void>;
      description: string;
      affectedNodes?: string[];
    }>
  ): Promise<void> {
    // Take single snapshot for entire batch
    const snapshot = this.takeSnapshot(false);

    const allAffectedNodes = operations.flatMap(
      (op) => op.affectedNodes || []
    );
    const batchDescription = operations.map((op) => op.description).join(', ');

    log(
      `[OptimisticOperationManager] Starting batch operation: ${batchDescription}`,
      {
        operationCount: operations.length,
        affectedNodes: allAffectedNodes,
        timestamp: snapshot.timestamp
      }
    );

    try {
      // Apply all optimistic updates
      for (const operation of operations) {
        operation.optimisticUpdate();
      }

      log(
        `[OptimisticOperationManager] Batch optimistic updates applied: ${batchDescription}`
      );

      // Fire all backend operations (don't await - let domain events confirm)
      // Use Promise.allSettled to capture all failures for better diagnostics
      Promise.allSettled(operations.map((op) => op.backendOperation())).then(
        (results) => {
          const failures = results.filter(
            (r): r is PromiseRejectedResult => r.status === 'rejected'
          );
          if (failures.length > 0) {
            // Aggregate error messages for diagnostics
            const aggregatedError = new Error(
              `${failures.length}/${operations.length} operations failed: ` +
                failures.map((f) => f.reason?.message || String(f.reason)).join('; ')
            );
            logError(
              `[OptimisticOperationManager] Batch backend operation failed: ${batchDescription}`,
              { failureCount: failures.length, totalCount: operations.length, failures }
            );
            this.handleBackendFailure(
              aggregatedError,
              snapshot,
              batchDescription,
              allAffectedNodes
            );
          }
        }
      );
    } catch (error) {
      // Optimistic update failed - rollback immediately
      logError(
        `[OptimisticOperationManager] Batch optimistic update failed: ${batchDescription}`,
        error
      );
      this.restoreSnapshot(snapshot, false);
      throw error;
    }
  }
}

// Export singleton instance
export const optimisticOperationManager = new OptimisticOperationManager();
