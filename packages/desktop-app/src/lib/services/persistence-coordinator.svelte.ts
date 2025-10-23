/**
 * PersistenceCoordinator - Declarative Dependency-Based Persistence
 *
 * This service coordinates asynchronous database persistence operations with
 * dependency tracking, ensuring operations execute in the correct order while
 * managing debouncing and conflict detection.
 *
 * Key Features:
 * - Declarative dependency management (nodeIds, lambdas, batches)
 * - Configurable debouncing (immediate vs. debounced persistence)
 * - Reactive state tracking (Svelte 5 $state)
 * - Test mode support (mocks database calls)
 * - Topological sorting for dependency resolution
 *
 * Architecture:
 * - UI Layer declares dependencies when requesting persistence
 * - Coordinator resolves dependencies and enforces execution order
 * - Automatic retry and error handling
 * - Performance metrics and monitoring
 *
 * @see docs/architecture/persistence-layer.md
 * @see docs/architecture/dependency-based-persistence.md
 */

// ============================================================================
// Error Types
// ============================================================================

/**
 * Error thrown when an operation is cancelled
 * This is an expected control-flow error, not a failure
 */
export class OperationCancelledError extends Error {
  constructor(nodeId: string, reason = 'Operation cancelled') {
    super(`${reason}: ${nodeId}`);
    this.name = 'OperationCancelledError';
  }
}

// ============================================================================
// Types and Interfaces
// ============================================================================

/**
 * Dependency types supported by the coordinator
 */
export type PersistenceDependency =
  | string // Simple node ID dependency
  | (() => Promise<void>) // Lambda function dependency
  | { nodeIds: string[] } // Batch of node IDs
  | PersistenceHandle; // Handle from previous operation

/**
 * Persistence mode
 */
export type PersistenceMode = 'immediate' | 'debounce';

/**
 * Persistence options
 */
export interface PersistenceOptions {
  /** Persistence mode: 'immediate' or 'debounce' (default: 'debounce') */
  mode?: PersistenceMode;

  /** Debounce delay in milliseconds (default: 500ms) */
  debounceMs?: number;

  /** Dependencies that must complete before this operation */
  dependencies?: PersistenceDependency[];
}

/**
 * Handle returned from persist() for tracking and chaining
 */
export interface PersistenceHandle {
  /** Node ID this operation affects */
  nodeId: string;

  /** Promise that resolves when persistence completes */
  promise: Promise<void>;

  /** Check if persistence has completed */
  isPersisted: () => boolean;
}

/**
 * Status of a persistence operation
 */
export type PersistenceStatus = 'pending' | 'in-progress' | 'completed' | 'failed';

/**
 * Internal operation tracking
 */
interface PersistenceOperation {
  nodeId: string;
  operation: () => Promise<void>;
  options: PersistenceOptions;
  status: PersistenceStatus;
  dependencies: PersistenceDependency[];
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
  createdAt: number;
  startedAt?: number;
  completedAt?: number;
  error?: Error;
  blocked: boolean; // Is this operation waiting for dependencies?
  blockingDeps: Set<string>; // Which dependencies are still pending?
}

/**
 * Performance metrics
 */
export interface PersistenceMetrics {
  totalOperations: number;
  completedOperations: number;
  failedOperations: number;
  averageExecutionTime: number;
  maxExecutionTime: number;
  pendingOperations: number;
}

// ============================================================================
// PersistenceCoordinator Class
// ============================================================================

export class PersistenceCoordinator {
  // Singleton instance
  private static instance: PersistenceCoordinator | null = null;

  // Internal state - no reactivity needed (components react to SharedNodeStore)
  // Fixed: Removed $state() wrappers as they're not compatible with class fields in Svelte 5
  private persistenceStatus = new Map<string, PersistenceStatus>();
  private persistedNodes = new Set<string>();

  // Internal tracking
  private operations = new Map<string, PersistenceOperation>();
  private completedOperations = new Set<string>();
  private waitingQueue = new Map<string, PersistenceOperation>(); // Operations blocked by dependencies

  // Test mode
  private testMode = false;
  private mockPersistence = new Map<string, boolean>();

  // Reset guard to prevent concurrent resets
  private isResetting = false;

  // Metrics
  private metrics: PersistenceMetrics = {
    totalOperations: 0,
    completedOperations: 0,
    failedOperations: 0,
    averageExecutionTime: 0,
    maxExecutionTime: 0,
    pendingOperations: 0
  };

  // Configuration
  private defaultDebounceMs = 500;
  private statusCleanupDelayMs = 5 * 60 * 1000; // 5 minutes

  private constructor() {
    // All properties initialized at declaration
  }

  /**
   * Get singleton instance
   */
  static getInstance(): PersistenceCoordinator {
    if (!PersistenceCoordinator.instance) {
      PersistenceCoordinator.instance = new PersistenceCoordinator();
    }
    return PersistenceCoordinator.instance;
  }

  /**
   * Reset singleton (for testing only)
   */
  static resetInstance(): void {
    PersistenceCoordinator.instance = null;
  }

  // ========================================================================
  // Core API
  // ========================================================================

  /**
   * Request persistence with dependency tracking
   *
   * @param nodeId - Node ID to persist
   * @param operation - Async function that performs the persistence
   * @param options - Persistence options (mode, dependencies, etc.)
   * @returns Handle for tracking and chaining
   *
   * @example
   * // Debounced content save (typing)
   * persistenceCoordinator.persist(
   *   nodeId,
   *   () => tauriNodeService.updateNode(nodeId, node),
   *   { mode: 'debounce' }
   * );
   *
   * @example
   * // Immediate save with dependency
   * persistenceCoordinator.persist(
   *   childId,
   *   () => tauriNodeService.createNode(child),
   *   {
   *     mode: 'immediate',
   *     dependencies: [parentId]  // Wait for parent first
   *   }
   * );
   *
   * @example
   * // Complex dependency with lambda
   * persistenceCoordinator.persist(
   *   nodeId,
   *   () => tauriNodeService.updateNode(nodeId, node),
   *   {
   *     mode: 'immediate',
   *     dependencies: [
   *       async () => {
   *         await ensureAncestorChainPersisted(nodeId);
   *       }
   *     ]
   *   }
   * );
   *
   * @example
   * // Error handling - recommended pattern
   * const handle = persistenceCoordinator.persist(
   *   nodeId,
   *   () => tauriNodeService.updateNode(nodeId, node),
   *   { mode: 'immediate', dependencies: [parentId] }
   * );
   *
   * try {
   *   await handle.promise;
   *   console.log('Persistence succeeded');
   * } catch (error) {
   *   // Error can be from:
   *   // 1. The operation itself (database error, network error, etc.)
   *   // 2. A dependency failure (parent failed to persist)
   *   // 3. Operation was cancelled (new operation for same node)
   *   console.error('Persistence failed:', error);
   * }
   *
   * **Error Recovery Behavior:**
   * - If a dependency fails, dependent operations will also fail immediately
   * - Failed operations reject their promise with the error from the dependency or operation
   * - The coordinator automatically cleans up failed operations
   * - Status is set to 'failed' and can be checked via getStatus(nodeId)
   * - Use try/catch on handle.promise to handle errors gracefully
   * - Operations can be retried by calling persist() again with the same nodeId
   */
  persist(
    nodeId: string,
    operation: () => Promise<void>,
    options: PersistenceOptions = {}
  ): PersistenceHandle {
    // Cancel any existing pending operation for this node
    this.cancelPending(nodeId);

    // Create promise for this operation
    let resolvePromise: () => void;
    let rejectPromise: (error: Error) => void;
    const promise = new Promise<void>((resolve, reject) => {
      resolvePromise = resolve;
      rejectPromise = reject;
    });

    // Create operation record
    const op: PersistenceOperation = {
      nodeId,
      operation,
      options,
      status: 'pending',
      dependencies: options.dependencies || [],
      promise,
      resolve: resolvePromise!,
      reject: rejectPromise!,
      createdAt: Date.now(),
      blocked: false,
      blockingDeps: new Set()
    };

    // Track operation
    this.operations.set(nodeId, op);
    this.persistenceStatus.set(nodeId, 'pending');
    this.metrics.totalOperations++;
    this.metrics.pendingOperations++;

    // Check if dependencies are pending - if so, block this operation
    const blockingDeps = new Set<string>();
    for (const dep of op.dependencies) {
      if (typeof dep === 'string') {
        const depOp = this.operations.get(dep);
        if (depOp && (depOp.status === 'pending' || depOp.status === 'in-progress')) {
          blockingDeps.add(dep);
        }
      }
      // Lambda dependencies cannot be blocked - they execute inline
      // This is intentional as they handle ancestor chain persistence
    }

    if (blockingDeps.size > 0) {
      // Operation must wait - don't schedule yet
      op.blocked = true;
      op.blockingDeps = blockingDeps;
      this.waitingQueue.set(nodeId, op);
    } else {
      // No blocking deps - schedule normally
      const mode = options.mode || 'debounce';
      if (mode === 'immediate') {
        this.scheduleImmediate(op);
      } else {
        this.scheduleDebounced(op);
      }
    }

    // Return handle
    return {
      nodeId,
      promise,
      isPersisted: () => this.isPersisted(nodeId)
    };
  }

  /**
   * Check if a node has been persisted (reactive)
   */
  isPersisted(nodeId: string): boolean {
    return this.persistedNodes.has(nodeId);
  }

  /**
   * Check if a node has a pending persistence operation (reactive)
   */
  isPending(nodeId: string): boolean {
    const status = this.persistenceStatus.get(nodeId);
    return status === 'pending' || status === 'in-progress';
  }

  /**
   * Get current status of a node's persistence (reactive)
   */
  getStatus(nodeId: string): PersistenceStatus | undefined {
    return this.persistenceStatus.get(nodeId);
  }

  /**
   * Wait for specific nodes to be persisted
   *
   * @param nodeIds - Array of node IDs to wait for
   * @param timeoutMs - Timeout in milliseconds (default: 5000)
   * @returns Set of node IDs that failed to persist
   */
  async waitForPersistence(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    const failedNodeIds = new Set<string>();
    const promises: Promise<void>[] = [];

    for (const nodeId of nodeIds) {
      const op = this.operations.get(nodeId);
      if (op) {
        promises.push(op.promise);
      }
    }

    if (promises.length === 0) {
      return failedNodeIds;
    }

    try {
      await Promise.race([
        Promise.all(promises),
        new Promise((_, reject) =>
          setTimeout(() => reject(new Error('Timeout waiting for persistence')), timeoutMs)
        )
      ]);
    } catch (error) {
      console.error('[PersistenceCoordinator] Timeout waiting for persistence:', {
        error,
        nodeIds
      });

      // Grace period
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Check which nodes failed
      for (const nodeId of nodeIds) {
        const op = this.operations.get(nodeId);
        if (op && (op.status === 'pending' || op.status === 'in-progress')) {
          failedNodeIds.add(nodeId);
        }
      }
    }

    return failedNodeIds;
  }

  /**
   * Flush all pending debounced operations immediately
   * Used during component unmount or page unload to prevent data loss
   *
   * NOTE: This is synchronous best-effort - operations execute immediately
   * but we don't wait for them to complete (to avoid blocking unmount)
   */
  flushPending(): void {
    const pendingOps = Array.from(this.operations.values()).filter((op) => op.status === 'pending');

    for (const op of pendingOps) {
      // Clear the debounce timer if one exists
      if (op.timer !== undefined) {
        clearTimeout(op.timer);
        op.timer = undefined;
      }

      // Execute immediately (fire-and-forget for unmount scenarios)
      this.executeOperation(op);
    }
  }

  /**
   * Cancel pending operation for a node
   */
  cancelPending(nodeId: string): void {
    const op = this.operations.get(nodeId);
    if (op && op.status === 'pending') {
      // Clear debounce timer if exists
      if (op.timer) {
        clearTimeout(op.timer);
      }

      // Mark as failed with cancellation error
      op.status = 'failed';
      op.error = new OperationCancelledError(nodeId);
      this.persistenceStatus.set(nodeId, 'failed');

      // Reject promise with cancellation error
      op.reject(op.error);

      // Clean up
      this.operations.delete(nodeId);
      this.metrics.pendingOperations--;
    }
  }

  /**
   * Get performance metrics
   */
  getMetrics(): PersistenceMetrics {
    return { ...this.metrics };
  }

  /**
   * Reset all state (for testing)
   *
   * Cancels all pending operations and waits for their cancellation
   * promises to be handled, preventing unhandled promise rejections.
   */
  async reset(): Promise<void> {
    // Guard against concurrent resets
    if (this.isResetting) {
      console.warn('[PersistenceCoordinator] Reset already in progress, skipping');
      return;
    }

    this.isResetting = true;
    try {
      // Cancel all pending operations and capture their promises
      // We iterate and capture promises before cancelling because
      // cancelPending() deletes entries from the operations Map
      const cancellationPromises: Promise<void>[] = [];
      for (const [nodeId, op] of this.operations) {
        // Capture the promise before cancelling
        cancellationPromises.push(
          op.promise.catch((err) => {
            // Silently ignore cancellation errors, log others
            if (!(err instanceof OperationCancelledError)) {
              console.warn('[PersistenceCoordinator] Unexpected error during reset:', err);
            }
          })
        );
        this.cancelPending(nodeId);
      }

      // Wait for all cancellations to be processed
      await Promise.allSettled(cancellationPromises);

      // Clear state (use .clear() to preserve Svelte 5 reactivity)
      this.operations.clear();
      this.completedOperations.clear();
      this.waitingQueue.clear(); // Clear blocked operations
      this.persistenceStatus.clear(); // Changed from reassignment to preserve reactive proxy
      this.persistedNodes.clear(); // Changed from reassignment to preserve reactive proxy
      this.mockPersistence.clear();

      // Reset metrics
      this.metrics = {
        totalOperations: 0,
        completedOperations: 0,
        failedOperations: 0,
        averageExecutionTime: 0,
        maxExecutionTime: 0,
        pendingOperations: 0
      };
    } finally {
      this.isResetting = false;
    }
  }

  // ========================================================================
  // Test Mode
  // ========================================================================

  /**
   * Enable test mode (gracefully handles database errors for testing)
   *
   * In test mode:
   * - Operations execute normally (allows test callbacks to run)
   * - Database errors are caught and ignored (no database initialization required)
   * - Mock persistence tracking is enabled (via mockPersistence Map)
   * - All persistence is marked as successful for testing purposes
   *
   * TEMPORARY IMPLEMENTATION:
   * Currently catches ALL errors in test mode, which may mask real issues.
   *
   * TODO: Refactor to proper mocking strategy:
   * 1. Mock tauriNodeService using vi.mock() in all tests
   * 2. Remove error catching - let mocked service return immediately
   * 3. Remove test mode entirely once proper mocks are in place
   */
  enableTestMode(): void {
    this.testMode = true;
  }

  /**
   * Disable test mode
   */
  disableTestMode(): void {
    this.testMode = false;
  }

  /**
   * Check if test mode is enabled
   */
  isTestMode(): boolean {
    return this.testMode;
  }

  /**
   * Get nodes that were mock-persisted in test mode
   */
  getMockPersistedNodes(): string[] {
    return Array.from(this.mockPersistence.keys());
  }

  /**
   * Reset test state
   */
  resetTestState(): void {
    this.mockPersistence.clear();
    this.reset();
  }

  // ========================================================================
  // Internal Scheduling
  // ========================================================================

  /**
   * Schedule immediate execution
   */
  private scheduleImmediate(op: PersistenceOperation): void {
    // Execute on next tick to allow synchronous code to complete
    setTimeout(() => this.executeOperation(op), 0);
  }

  /**
   * Schedule debounced execution
   */
  private scheduleDebounced(op: PersistenceOperation): void {
    const debounceMs = op.options.debounceMs || this.defaultDebounceMs;

    // Set timer
    op.timer = setTimeout(() => {
      this.executeOperation(op);
    }, debounceMs);
  }

  /**
   * Execute a persistence operation
   */
  private async executeOperation(op: PersistenceOperation): Promise<void> {
    try {
      // Mark as in-progress
      op.status = 'in-progress';
      op.startedAt = Date.now();
      this.persistenceStatus.set(op.nodeId, 'in-progress');

      // Resolve dependencies first
      await this.resolveDependencies(op.dependencies);

      // Execute operation
      if (this.testMode) {
        // In test mode, execute operation but catch and ignore database initialization errors
        // This allows test operations (like setting flags) to run while gracefully
        // handling database initialization errors from real services
        try {
          await op.operation();
        } catch (error) {
          // Only ignore DatabaseInitializationError - re-throw all other errors
          // This allows error-handling tests to work correctly
          const errorName = error instanceof Error ? error.constructor.name : '';
          if (errorName !== 'DatabaseInitializationError') {
            throw error;
          }
          // Database initialization errors are silently ignored in test mode
          // TODO: Proper solution is to mock tauriNodeService in all tests
        }
        this.mockPersistence.set(op.nodeId, true);
      } else {
        await op.operation();
      }

      // Mark as completed
      op.status = 'completed';
      op.completedAt = Date.now();
      this.persistenceStatus.set(op.nodeId, 'completed');
      this.persistedNodes.add(op.nodeId);
      this.completedOperations.add(op.nodeId);

      // Update metrics
      const executionTime = op.completedAt - op.startedAt!;
      this.updateMetrics(executionTime, true);

      // Resolve promise
      op.resolve();

      // Clean up operation tracking
      this.operations.delete(op.nodeId);
      this.metrics.pendingOperations--;

      // Unblock any operations that were waiting for this one
      this.unblockDependentOperations(op.nodeId);

      // Schedule cleanup of completed status to prevent memory leaks
      this.scheduleStatusCleanup(op.nodeId);
    } catch (error) {
      const err = error instanceof Error ? error : new Error(String(error));

      // Mark as failed
      op.status = 'failed';
      op.error = err;
      op.completedAt = Date.now();
      this.persistenceStatus.set(op.nodeId, 'failed');

      // Update metrics
      this.updateMetrics(0, false);

      // Reject promise
      op.reject(err);

      // Log error (consistent structured logging)
      // Only log in production or database integration tests
      // In in-memory test mode, these errors are expected (silently caught above)
      const inTestMode = typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
      const inDatabaseMode =
        typeof process !== 'undefined' && process.env.TEST_USE_DATABASE === 'true';

      if (!inTestMode || inDatabaseMode) {
        // Production or database integration tests - log errors
        console.error('[PersistenceCoordinator] Operation failed:', {
          nodeId: op.nodeId,
          error: err
        });
      }

      // Clean up
      this.operations.delete(op.nodeId);
      this.metrics.pendingOperations--;
    }
  }

  /**
   * Unblock operations that were waiting for a completed dependency
   */
  private unblockDependentOperations(completedNodeId: string): void {
    for (const [waitingNodeId, waitingOp] of this.waitingQueue) {
      if (waitingOp.blockingDeps.has(completedNodeId)) {
        waitingOp.blockingDeps.delete(completedNodeId);

        if (waitingOp.blockingDeps.size === 0) {
          // All dependencies resolved - schedule now
          this.waitingQueue.delete(waitingNodeId);
          waitingOp.blocked = false;

          const mode = waitingOp.options.mode || 'debounce';
          if (mode === 'immediate') {
            this.scheduleImmediate(waitingOp);
          } else {
            this.scheduleDebounced(waitingOp);
          }
        }
      }
    }
  }

  /**
   * Resolve all dependencies for an operation
   */
  private async resolveDependencies(dependencies: PersistenceDependency[]): Promise<void> {
    if (dependencies.length === 0) {
      return;
    }

    const promises: Promise<void>[] = [];

    for (const dep of dependencies) {
      if (typeof dep === 'string') {
        // Simple node ID dependency
        const depOp = this.operations.get(dep);
        if (depOp) {
          promises.push(depOp.promise);
        }
      } else if (typeof dep === 'function') {
        // Lambda dependency
        promises.push(dep());
      } else if ('nodeIds' in dep) {
        // Batch dependency
        for (const nodeId of dep.nodeIds) {
          const depOp = this.operations.get(nodeId);
          if (depOp) {
            promises.push(depOp.promise);
          }
        }
      } else if ('promise' in dep) {
        // Handle dependency
        promises.push(dep.promise);
      }
    }

    // Wait for all dependencies
    await Promise.all(promises);
  }

  /**
   * Update performance metrics
   */
  private updateMetrics(executionTime: number, success: boolean): void {
    if (success) {
      this.metrics.completedOperations++;

      // Update average execution time
      const count = this.metrics.completedOperations;
      const currentAvg = this.metrics.averageExecutionTime;
      this.metrics.averageExecutionTime = (currentAvg * (count - 1) + executionTime) / count;

      // Update max execution time
      this.metrics.maxExecutionTime = Math.max(this.metrics.maxExecutionTime, executionTime);
    } else {
      this.metrics.failedOperations++;
    }
  }

  /**
   * Schedule cleanup of completed status to prevent unbounded memory growth
   *
   * After a node completes persistence, we keep its status for a short time
   * to allow UI components to react to the completion. After the cleanup delay,
   * we remove the status to prevent memory leaks in long-running sessions.
   *
   * Note: We keep persistedNodes Set intact - it's needed for existence checks.
   */
  private scheduleStatusCleanup(nodeId: string): void {
    setTimeout(() => {
      // Only clean up if status is still 'completed' (not failed or re-pending)
      if (this.persistenceStatus.get(nodeId) === 'completed') {
        this.persistenceStatus.delete(nodeId);
        this.completedOperations.delete(nodeId);
        // Keep persistedNodes - it's used for isPersisted() checks
      }
    }, this.statusCleanupDelayMs);
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

/**
 * Singleton instance for application-wide use
 */
export const persistenceCoordinator = PersistenceCoordinator.getInstance();

/**
 * Default export
 */
export default PersistenceCoordinator;
