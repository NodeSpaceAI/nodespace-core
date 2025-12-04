/**
 * Global database write queue to prevent SQLite "database is locked" errors
 *
 * SQLite only allows one write transaction at a time. This queue serializes
 * all database write operations to prevent concurrent access errors.
 *
 * IMPORTANT: This uses module-level state intentionally.
 * SQLite's write lock is process-global, so a singleton queue is correct.
 *
 * Usage:
 * ```typescript
 * import { queueDatabaseWrite, getQueueDepth } from '$lib/utils/database-write-queue';
 *
 * // Queue a database operation
 * await queueDatabaseWrite(() => databaseService.updateNode(nodeId, updates));
 *
 * // Monitor queue depth
 * const depth = getQueueDepth();
 * ```
 *
 * @module databaseWriteQueue
 */

import { createLogger } from '$lib/utils/logger';

const log = createLogger('DatabaseWriteQueue');

// Module-level state
let pendingDatabaseWritePromise: Promise<void> | null = null;
let queueDepth = 0;

/**
 * Threshold for warning about high queue depth
 * When queue depth exceeds this value, a console warning is emitted
 */
const QUEUE_DEPTH_WARNING_THRESHOLD = 10;

/**
 * Queue a database write operation to ensure it doesn't overlap with other writes
 * This prevents SQLite "database is locked" errors by serializing all writes
 *
 * @param operation - The async operation to execute (should return a Promise)
 * @returns The result of the operation
 */
export async function queueDatabaseWrite<T>(operation: () => Promise<T>): Promise<T> {
  const previousPromise = pendingDatabaseWritePromise;

  // Track queue depth for monitoring
  queueDepth++;
  if (queueDepth > QUEUE_DEPTH_WARNING_THRESHOLD) {
    log.warn(
      `High queue depth detected: ${queueDepth} (threshold: ${QUEUE_DEPTH_WARNING_THRESHOLD})`
    );
  }

  // Create a new promise that waits for the previous operation before running ours
  let resolveOurPromise: (() => void) | undefined;
  pendingDatabaseWritePromise = new Promise<void>((resolve) => {
    resolveOurPromise = resolve;
  });

  // Wait for previous operation to complete
  if (previousPromise) {
    try {
      await previousPromise;
    } catch {
      // Ignore errors from previous operations - they're handled by their own callers
      // We only care that the operation completed (successfully or not) before we proceed
    }
  }

  // Now execute our operation
  try {
    const result = await operation();
    if (resolveOurPromise) {
      resolveOurPromise();
    }
    queueDepth--;
    return result;
  } catch (error) {
    if (resolveOurPromise) {
      resolveOurPromise();
    }
    queueDepth--;
    throw error;
  }
}

/**
 * Get current queue depth for monitoring/debugging
 * @returns Current number of operations waiting in queue
 */
export function getQueueDepth(): number {
  return queueDepth;
}

/**
 * Reset queue state for testing purposes only
 * This function should only be called in test environments to reset module-level state
 * between test runs, ensuring test isolation.
 *
 * @throws Error if called outside of test environment
 * @internal
 */
export function __resetQueueForTesting(): void {
  if (process.env.NODE_ENV !== 'test') {
    throw new Error('__resetQueueForTesting() can only be called in test environment');
  }
  pendingDatabaseWritePromise = null;
  queueDepth = 0;
}
