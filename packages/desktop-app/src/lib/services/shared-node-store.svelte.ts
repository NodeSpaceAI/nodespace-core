/**
 * SharedNodeStore - Singleton Reactive Store for Multi-Viewer Support
 *
 * Phase 1-2 Implementation:
 * - Single source of truth for all node data (Svelte 5 $state)
 * - Observer pattern for viewer subscriptions
 * - Real-time synchronization across multiple viewers
 * - Conflict detection and resolution (Last-Write-Wins)
 * - Optimistic updates with rollback
 * - Performance tracking and metrics
 *
 * Architecture:
 * - Singleton pattern ensures single shared store
 * - Multiple ReactiveNodeService instances read from same store
 * - Per-viewer UI state (expand/collapse, focus) stored separately
 * - Database writes coordinated through existing queueDatabaseWrite()
 *
 * Future Phases:
 * - Phase 3: MCP server integration
 * - Phase 4-5: Tab/Pane management and coordination
 * - Phase 6: Memory optimization and lazy loading
 * - Phase 7: Advanced conflict resolution (OT/CRDT)
 */

import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { requiresAtomicBatching } from '$lib/utils/placeholder-detection';
import { shouldLogDatabaseErrors, isTestEnvironment } from '$lib/utils/test-environment';
import * as tauriCommands from './tauri-commands';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import type { Node } from '$lib/types';
import type {
  NodeUpdate,
  UpdateSource,
  Conflict,
  ConflictResolver,
  NodeChangeCallback,
  Unsubscribe,
  StoreMetrics,
  UpdateOptions
} from '$lib/types/update-protocol';

// ============================================================================
// Simple Debounce Utility
// ============================================================================

interface PendingOperation {
  nodeId: string;
  operation: () => Promise<void>;
  timeoutId: ReturnType<typeof setTimeout>;
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: Error) => void;
}

class SimplePersistenceCoordinator {
  private static instance: SimplePersistenceCoordinator | null = null;
  private pendingOperations = new Map<string, PendingOperation>();
  private readonly DEBOUNCE_MS = 500;

  static getInstance(): SimplePersistenceCoordinator {
    if (!SimplePersistenceCoordinator.instance) {
      SimplePersistenceCoordinator.instance = new SimplePersistenceCoordinator();
    }
    return SimplePersistenceCoordinator.instance;
  }

  static resetInstance(): void {
    SimplePersistenceCoordinator.instance = null;
  }

  persist(
    nodeId: string,
    operation: () => Promise<void>,
    options: { mode: 'immediate' | 'debounce'; dependencies?: Array<string | (() => Promise<void>)> } = { mode: 'debounce' }
  ): { promise: Promise<void> } {
    // Cancel existing pending operation for this node
    this.cancelPending(nodeId);

    let resolve: () => void = () => {};
    let reject: (error: Error) => void = () => {};
    const promise = new Promise<void>((res, rej) => {
      resolve = res;
      reject = rej;
    });

    const executeOperation = async () => {
      try {
        // Wait for dependencies if any
        if (options.dependencies) {
          for (const dep of options.dependencies) {
            if (typeof dep === 'function') {
              await dep();
            } else {
              // Wait for dependent node to finish
              const pending = this.pendingOperations.get(dep);
              if (pending) {
                await pending.promise;
              }
            }
          }
        }

        await operation();
        resolve();
      } catch (error) {
        reject(error instanceof Error ? error : new Error(String(error)));
      } finally {
        this.pendingOperations.delete(nodeId);
      }
    };

    if (options.mode === 'immediate') {
      const pending: PendingOperation = {
        nodeId,
        operation,
        timeoutId: setTimeout(() => {}, 0),
        promise,
        resolve,
        reject
      };
      this.pendingOperations.set(nodeId, pending);
      executeOperation();
    } else {
      const timeoutId = setTimeout(executeOperation, this.DEBOUNCE_MS);
      const pending: PendingOperation = {
        nodeId,
        operation,
        timeoutId,
        promise,
        resolve,
        reject
      };
      this.pendingOperations.set(nodeId, pending);
    }

    return { promise };
  }

  cancelPending(nodeId: string): void {
    const pending = this.pendingOperations.get(nodeId);
    if (pending) {
      clearTimeout(pending.timeoutId);
      this.pendingOperations.delete(nodeId);
    }
  }

  isPending(nodeId: string): boolean {
    return this.pendingOperations.has(nodeId);
  }

  /**
   * Flush all pending operations immediately.
   * Used on window close to prevent data loss.
   *
   * @returns Promise that resolves when all pending operations complete or timeout
   */
  async flushPending(): Promise<void> {
    const nodeIds = Array.from(this.pendingOperations.keys());
    if (nodeIds.length === 0) return;

    // Execute all pending operations immediately by clearing their timeouts and running them
    const promises: Promise<void>[] = [];
    for (const [nodeId, pending] of this.pendingOperations) {
      clearTimeout(pending.timeoutId);
      // Execute the operation directly - start the execution, then wait on the pending promise
      pending.operation().then(
        () => pending.resolve(),
        (error) => pending.reject(error instanceof Error ? error : new Error(String(error)))
      ).finally(() => {
        this.pendingOperations.delete(nodeId);
      });
      promises.push(pending.promise.catch(() => {})); // Ignore errors, just wait for completion
    }

    // Wait for all to complete with a timeout
    await Promise.race([
      Promise.all(promises),
      new Promise<void>((resolve) => setTimeout(resolve, 5000)) // 5 second timeout
    ]);
  }

  async waitForPersistence(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    const failed = new Set<string>();
    const promises = nodeIds.map(async (nodeId) => {
      const pending = this.pendingOperations.get(nodeId);
      if (pending) {
        try {
          await Promise.race([
            pending.promise,
            new Promise<void>((_, reject) => setTimeout(() => reject(new Error('Timeout')), timeoutMs))
          ]);
        } catch {
          failed.add(nodeId);
        }
      }
    });
    await Promise.all(promises);
    return failed;
  }

  /**
   * Flush specific pending operations immediately and wait for completion.
   *
   * Unlike waitForPersistence which only waits for in-flight operations,
   * this method also triggers debounced operations that haven't started yet.
   *
   * Use this when you need to ensure specific nodes are fully persisted
   * before performing dependent operations (e.g., moveNode that references them).
   *
   * @param nodeIds - Node IDs to flush and wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to persist
   */
  async flushAndWaitForNodes(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    const failed = new Set<string>();
    const promises: Promise<void>[] = [];

    for (const nodeId of nodeIds) {
      const pending = this.pendingOperations.get(nodeId);
      if (pending) {
        // Clear the debounce timeout and execute immediately
        clearTimeout(pending.timeoutId);

        // Start the operation now (if it hasn't started yet)
        pending.operation().then(
          () => pending.resolve(),
          (error) => pending.reject(error instanceof Error ? error : new Error(String(error)))
        ).finally(() => {
          this.pendingOperations.delete(nodeId);
        });

        // Wait for completion with timeout
        promises.push(
          Promise.race([
            pending.promise,
            new Promise<void>((_, reject) =>
              setTimeout(() => reject(new Error('Timeout')), timeoutMs)
            )
          ]).catch(() => {
            failed.add(nodeId);
          })
        );
      }
    }

    await Promise.all(promises);
    return failed;
  }

  getMetrics(): { pendingOperations: number } {
    return { pendingOperations: this.pendingOperations.size };
  }

  /**
   * Flush ALL pending operations immediately and wait for completion.
   *
   * This is more aggressive than flushAndWaitForNodes - it ensures the entire
   * pending operation queue is cleared before proceeding. Use this for structural
   * operations like moveNode that may depend on edges created by any pending save.
   *
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to persist
   */
  async flushAll(timeoutMs = 5000): Promise<Set<string>> {
    const allNodeIds = Array.from(this.pendingOperations.keys());
    if (allNodeIds.length === 0) {
      return new Set();
    }
    return this.flushAndWaitForNodes(allNodeIds, timeoutMs);
  }
}

// Use simple coordinator
const PersistenceCoordinator = SimplePersistenceCoordinator;

// Simple error class for cancelled operations
class OperationCancelledError extends Error {
  constructor(message = 'Operation cancelled') {
    super(message);
    this.name = 'OperationCancelledError';
  }
}

// ============================================================================
// Simple Conflict Resolver (Last-Write-Wins)
// ============================================================================

function createDefaultResolver(): ConflictResolver {
  return {
    resolve(conflict: Conflict, existingNode: Node) {
      // Last-write-wins: use the remote update
      const resolvedNode = {
        ...existingNode,
        ...conflict.remoteUpdate.changes,
        modifiedAt: new Date().toISOString()
      };
      return {
        nodeId: conflict.nodeId,
        resolvedNode,
        strategy: 'last-write-wins' as const,
        discardedUpdate: conflict.localUpdate
      };
    },
    getStrategyName() {
      return 'Last Write Wins';
    }
  };
}

// ============================================================================
// Database Write Coordination (Phase 2.4)
// ============================================================================
// NOTE: Database write coordination is now handled by PersistenceCoordinator
// All persistence operations delegate to PersistenceCoordinator.getInstance().persist()

// ============================================================================
// Constants
// ============================================================================

/**
 * Default timeout for batch updates (milliseconds)
 * Batches auto-commit after this duration of inactivity
 * Timer resets on each change, so batch only commits after true inactivity
 */
const DEFAULT_BATCH_TIMEOUT_MS = 2000; // 2 seconds

/**
 * Subscription metadata for debugging and cleanup
 */
interface Subscription {
  id: string;
  nodeId: string | null; // null = subscribe to all nodes
  callback: NodeChangeCallback;
  createdAt: number;
  callCount: number;
}

/**
 * Batch structure for atomic multi-property updates
 * Used for pattern conversions where content + nodeType must persist together
 */
interface ActiveBatch {
  nodeId: string;
  changes: Partial<Node>;
  batchId: string;
  createdAt: number;
  timeout: ReturnType<typeof setTimeout>;
  timeoutMs: number;
}

/**
 * SharedNodeStore - Reactive singleton store for node data
 *
 * Uses Svelte 5 $state for the nodes Map to provide automatic reactivity.
 * Components using $derived(sharedNodeStore.getNode(nodeId)) will re-render
 * when nodes are added, updated, or removed.
 */
export class SharedNodeStore {
  private static instance: SharedNodeStore | null = null;

  // Core state - using Svelte 5 $state for automatic reactivity
  // This enables $derived() to properly re-run when nodes change
  nodes = $state(new Map<string, Node>());

  // Track which nodes have been persisted to database
  // Avoids querying database on every update to check existence
  private persistedNodeIds = new Set<string>();

  // NOTE: childrenCache and parentsCache REMOVED (Issue #557)
  // Hierarchy is now managed by ReactiveStructureTree (domain events)
  // Use structureTree.getChildren() and structureTree.getParent() instead

  // Subscriptions for change notifications
  private subscriptions = new Map<string, Set<Subscription>>();
  private wildcardSubscriptions = new Set<Subscription>();
  private subscriptionIdCounter = 0;

  // Batch ID counter for unique batch identification
  private batchIdCounter = 0;

  // Conflict resolution
  private conflictResolver: ConflictResolver = createDefaultResolver();
  private conflictWindowMs = 5000; // 5 seconds default for conflict detection

  // Pending operations (optimistic updates)
  private pendingUpdates = new Map<string, NodeUpdate[]>();

  // Performance metrics
  private metrics: StoreMetrics = {
    updateCount: 0,
    avgUpdateTime: 0,
    maxUpdateTime: 0,
    subscriptionCount: 0,
    conflictCount: 0,
    rollbackCount: 0
  };

  // Version tracking for optimistic concurrency
  private versions = new Map<string, number>();

  // Test error tracking (populated only in NODE_ENV='test', cleared between tests)
  private testErrors: Error[] = [];

  // Batch notification flag - when true, subscriber notifications are deferred
  private isBatchingNotifications = false;
  private batchedNotifications = new Map<string, { node: Node; source: UpdateSource }>();

  // Batch update tracking for atomic multi-property updates
  // Used for pattern conversions where content + nodeType must persist together
  private activeBatches = new Map<string, ActiveBatch>();

  // Track pending tree loads to prevent duplicate concurrent loads from multiple tabs
  private pendingTreeLoads = new Map<string, Promise<Node[]>>();

  private constructor() {
    // Private constructor for singleton
  }

  /**
   * Get singleton instance
   */
  static getInstance(): SharedNodeStore {
    if (!SharedNodeStore.instance) {
      SharedNodeStore.instance = new SharedNodeStore();
    }
    return SharedNodeStore.instance;
  }

  /**
   * Reset singleton (for testing only)
   */
  static resetInstance(): void {
    SharedNodeStore.instance = null;
  }

  // ========================================================================
  // Persistence Control (Phase 1 of UpdateSource Refactor)
  // ========================================================================

  /**
   * Determine persistence behavior from explicit options or legacy source type.
   *
   * This helper implements the new explicit persistence API while maintaining
   * backward compatibility with the legacy `source.type === 'database'` checks.
   *
   * Priority (highest to lowest):
   * 1. options.markAsPersistedOnly - Mark as persisted without re-persisting
   * 2. options.skipPersistence - Skip persistence
   * 3. options.persist - Explicit persistence control
   * 4. Legacy: source.type === 'database' - Auto-skip persistence
   * 5. Default: Auto-determine based on source type and changes
   *
   * @returns Object with shouldPersist and shouldMarkAsPersisted flags
   */
  private determinePersistenceBehavior(
    source: UpdateSource,
    options: UpdateOptions,
    _changes?: Partial<Node>
  ): { shouldPersist: boolean; shouldMarkAsPersisted: boolean } {
    // Priority 1: Explicit mark-as-persisted-only (no actual persistence)
    if (options.markAsPersistedOnly) {
      return { shouldPersist: false, shouldMarkAsPersisted: true };
    }

    // Priority 2: Skip persistence flag (legacy compatibility)
    if (options.skipPersistence) {
      return { shouldPersist: false, shouldMarkAsPersisted: false };
    }

    // Priority 3: Explicit persist option (new API)
    if (options.persist !== undefined) {
      if (options.persist === false) {
        // Explicitly skip persistence
        return { shouldPersist: false, shouldMarkAsPersisted: false };
      }
      // persist === true, 'debounced', or 'immediate' - all trigger persistence
      // (Mode selection handled by PersistenceCoordinator)
      return { shouldPersist: true, shouldMarkAsPersisted: false };
    }

    // Priority 4: Legacy source.type === 'database' behavior
    // Database sources mean "loaded from backend, already persisted"
    if (source.type === 'database') {
      return { shouldPersist: false, shouldMarkAsPersisted: true };
    }

    // Priority 5: Default auto-determination
    // External sources and MCP sources trigger persistence
    // Viewer sources depend on context (handled by caller)
    return { shouldPersist: true, shouldMarkAsPersisted: false };
  }

  // ========================================================================
  // Core Node Operations
  // ========================================================================

  /**
   * Get a node by ID
   */
  getNode(nodeId: string): Node | undefined {
    return this.nodes.get(nodeId);
  }

  /**
   * Get all nodes (returns reactive Map)
   */
  getAllNodes(): Map<string, Node> {
    return this.nodes;
  }

  /**
   * Get child nodes for a parent (synchronous, in-memory lookup)
   *
   * This is a convenience method that combines:
   * 1. structureTree.getChildren(parentId) - get ordered child IDs
   * 2. Map lookup for each ID - get Node objects
   *
   * Use this when you need Node objects, not just IDs.
   * For IDs only, use structureTree.getChildren() directly (more efficient).
   * For async DB loading, use loadChildrenForParent().
   *
   * NOTE: Returns empty array in tests without ReactiveStructureTree initialized.
   *
   * @param parentId - Parent node ID, or null for root-level nodes
   * @returns Array of child Node objects in sorted order
   */
  getNodesForParent(parentId: string | null): Node[] {
    // In tests, structureTree may not be initialized
    if (!structureTree) return [];
    const cacheKey = parentId ?? '__root__';
    const childIds = structureTree.getChildren(cacheKey);
    return childIds.map(id => this.nodes.get(id)).filter((n): n is Node => n !== undefined);
  }

  /**
   * Get parent nodes for a given node (synchronous, ReactiveStructureTree-based)
   *
   * Delegates to ReactiveStructureTree which maintains hierarchy via domain events.
   *
   * NOTE: In graph-native architecture, a node can have multiple parents via different edge types.
   * Currently this method returns the parent from the primary hierarchy only.
   *
   * NOTE: In tests without ReactiveStructureTree initialized, returns empty array.
   *
   * @param nodeId - Node ID to find parents for
   * @returns Array of parent nodes (from ReactiveStructureTree)
   */
  getParentsForNode(nodeId: string): Node[] {
    // In tests, structureTree may not be initialized
    if (!structureTree) return [];
    const parentId = structureTree.getParent(nodeId);
    if (!parentId || parentId === '__root__') return [];
    const parent = this.nodes.get(parentId);
    return parent ? [parent] : [];
  }

  /**
   * Check if a node exists
   */
  hasNode(nodeId: string): boolean {
    return this.nodes.has(nodeId);
  }

  /**
   * Get node count
   */
  getNodeCount(): number {
    return this.nodes.size;
  }

  // ========================================================================
  // Update Operations with Conflict Detection
  // ========================================================================

  /**
   * Update a node with conflict detection and source tracking
   *
   * Note: Mention relationships are automatically synced by the backend when content changes.
   * The Rust backend extracts nodespace:// mentions and maintains the node_mentions table.
   *
   * @param nodeId - ID of node to update
   * @param changes - Partial node changes to apply
   * @param source - Source of the update (viewer, database, MCP)
   * @param options - Update options (conflict detection, persistence, etc.)
   */
  updateNode(
    nodeId: string,
    changes: Partial<Node>,
    source: UpdateSource,
    options: UpdateOptions = {}
  ): void {
    const startTime = performance.now();

    // Handle isComputedField flag - automatically set skipPersistence and skipConflictDetection
    if (options.isComputedField) {
      options = {
        ...options,
        skipPersistence: true,
        skipConflictDetection: true
      };
    }

    // ========================================================================
    // Batch Handling - Route updates through batch system if active
    // ========================================================================

    // Check if batch is active for this node
    if (this.activeBatches.has(nodeId)) {
      // Route through batch system
      this.addToBatch(nodeId, changes);

      // Auto-commit if requested
      if (options.batch?.commitImmediately) {
        this.commitBatch(nodeId);
      }
      return;
    }

    // Check if this update should create a new batch
    if (options.batch?.autoBatch) {
      this.startBatch(nodeId, options.batch.batchTimeout);
      this.addToBatch(nodeId, changes);

      // Auto-commit if requested
      if (options.batch.commitImmediately) {
        this.commitBatch(nodeId);
      }
      return;
    }

    // CRITICAL: Auto-restart batch for pattern-converted node types
    // After a batch commits, subsequent edits should ALSO be batched to maintain consistency
    // This prevents falling back to old debounced path which can cause partial content loss
    // IMPORTANT: Respect UpdateOptions - don't batch if caller explicitly skipped persistence
    const existingNode = this.nodes.get(nodeId);
    const nodeRequiresBatching = existingNode && requiresAtomicBatching(existingNode.nodeType);

    if (nodeRequiresBatching && changes.content !== undefined && !options.skipPersistence) {
      this.startBatch(nodeId, DEFAULT_BATCH_TIMEOUT_MS);
      this.addToBatch(nodeId, changes);
      return;
    }

    // ========================================================================
    // Normal Update Flow (No Batching)
    // ========================================================================

    try {
      // Get existing node
      const existingNode = this.nodes.get(nodeId);
      if (!existingNode) {
        console.warn(`[SharedNodeStore] Cannot update non-existent node: ${nodeId}`);
        return;
      }

      // Create update record
      const update: NodeUpdate = {
        nodeId,
        changes,
        source,
        timestamp: Date.now(),
        version: this.getNextVersion(nodeId),
        previousVersion: this.versions.get(nodeId)
      };

      // Conflict detection (unless skipped)
      if (!options.skipConflictDetection) {
        const conflict = this.detectConflict(nodeId, update);
        if (conflict) {
          this.handleConflict(conflict);
          return;
        }
      }

      // Apply update optimistically
      const updatedNode: Node = {
        ...existingNode,
        ...changes,
        modifiedAt: new Date().toISOString()
      };

      this.nodes.set(nodeId, updatedNode);
      this.versions.set(nodeId, update.version!);

      // Track pending update for potential rollback
      if (!this.pendingUpdates.has(nodeId)) {
        this.pendingUpdates.set(nodeId, []);
      }
      this.pendingUpdates.get(nodeId)!.push(update);

      // Notify subscribers
      this.notifySubscribers(nodeId, updatedNode, source);

      console.debug(`[SharedNodeStore] Node updated: ${nodeId}, type: ${this.determineUpdateType(changes)}`);

      // Update metrics
      this.metrics.updateCount++;

      // Phase 2.4: Persist to database (unless skipped)
      // IMPORTANT: For viewer-sourced updates:
      // - Structural changes persist immediately
      // - Content changes persist in debounced mode
      // This ensures hierarchy operations work while debouncing rapid typing
      const persistBehavior = this.determinePersistenceBehavior(source, options, changes);
      if (persistBehavior.shouldPersist) {
        // Issue #479: All real nodes (even blank) should be persisted
        // FOREIGN KEY validation is handled by persistence coordinator dependencies
        // Structural changes (sibling ordering) are now handled via backend moveNode()

        // Issue #709: Smart routing via plugin system supports spoke fields
        // Type-specific updaters route to spoke table methods (updateTaskNode, etc.)
        // The persistence whitelist now includes spoke field changes
        const isStructuralChange = false; // Structural changes now handled via backend moveNode()
        const isContentChange = 'content' in changes;
        const isNodeTypeChange = 'nodeType' in changes;
        const isPropertyChange = 'properties' in changes;
        // Issue #709: Check for spoke field changes (status, priority, dueDate, assignee, etc.)
        // These are persisted via type-specific updaters registered in the plugin system
        const currentNode = this.nodes.get(nodeId);
        const hasSpokeFieldUpdater = currentNode?.nodeType
          ? pluginRegistry.hasNodeUpdater(currentNode.nodeType)
          : false;
        const isSpokeFieldChange =
          hasSpokeFieldUpdater &&
          ('status' in changes ||
            'priority' in changes ||
            'dueDate' in changes ||
            'assignee' in changes ||
            'startedAt' in changes ||
            'completedAt' in changes);
        const shouldPersist =
          source.type !== 'viewer' ||
          isStructuralChange ||
          isContentChange ||
          isNodeTypeChange ||
          isPropertyChange ||
          isSpokeFieldChange;

        // Issue #479: Do NOT check isPlaceholder here - that's a UI-only concept
        // Real nodes created by user actions (Enter key) should persist even if blank
        // Only BaseNodeViewer's viewer-local placeholder should be unpersisted

        // CRITICAL: Skip persistence if batch is active for this node
        // The batch will handle persistence atomically when committed
        const hasBatchActive = this.activeBatches.has(nodeId);

        if (shouldPersist && !hasBatchActive) {
          // Delegate to PersistenceCoordinator for coordinated persistence
          // Use debounced mode for content changes (typing), immediate for structural changes
          const dependencies: Array<string | (() => Promise<void>)> = [];

          // Parent/container relationships are now managed via graph edges in the backend
          // Sibling ordering is now managed via fractional position IDs in the backend
          // No frontend foreign key dependency tracking needed

          // Add any additional dependencies from options
          if (options.persistenceDependencies) {
            dependencies.push(...options.persistenceDependencies);
          }

          // Capture handle to catch cancellation errors
          const handle = PersistenceCoordinator.getInstance().persist(
            nodeId,
            async () => {
              try {
                // Issue #479: All real nodes (even blank) should be persisted
                // No placeholder checks here - viewer-local placeholder never enters this code path

                // Check if node has been persisted - use in-memory tracking to avoid database query
                const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);

                if (isPersistedToDatabase) {
                  // IMPORTANT: For UPDATE, only send the changes (Partial<Node>), not the full node
                  // The backend expects NodeUpdate which is a partial update of specific fields
                  // Convert nullable fields properly (undefined = don't update, null = set to null)
                  const updatePayload = { ...changes };

                  // Get current node version for optimistic concurrency control
                  const currentNode = this.nodes.get(nodeId);
                  const currentVersion = currentNode?.version ?? 1;

                  try {
                    // Issue #709: Smart routing via plugin system
                    // Type-specific updaters route to spoke table methods (updateTaskNode, etc.)
                    // Generic updater falls back to hub table properties JSON update
                    //
                    // CRITICAL: Don't use type-specific updater when nodeType is CHANGING
                    // Type-specific updaters are only for spoke field updates on nodes that
                    // are ALREADY of that type. Node type changes must go through generic path.
                    const nodeType = currentNode?.nodeType;
                    const isNodeTypeChanging = 'nodeType' in updatePayload;
                    const typeUpdater =
                      nodeType && !isNodeTypeChanging ? pluginRegistry.getNodeUpdater(nodeType) : null;

                    let updatedNodeFromBackend: Node | null = null;

                    if (typeUpdater) {
                      // Type-specific path → spoke table update
                      // The plugin updater handles mapping changes to type-specific fields
                      console.debug(`[SharedNodeStore] Using type-specific updater for ${nodeType}`);
                      updatedNodeFromBackend = await typeUpdater.update(
                        nodeId,
                        currentVersion,
                        updatePayload
                      );
                    } else {
                      // Generic path → hub table properties JSON update
                      // CRITICAL: Capture updated node to get new version from backend
                      // This prevents version conflicts on subsequent updates
                      updatedNodeFromBackend = await tauriCommands.updateNode(
                        nodeId,
                        currentVersion,
                        updatePayload
                      );
                    }

                    // Update local node with backend version
                    const localNode = this.nodes.get(nodeId);
                    if (localNode && updatedNodeFromBackend) {
                      localNode.version = updatedNodeFromBackend.version;
                      this.nodes.set(nodeId, localNode);
                    }
                  } catch (updateError) {
                    // If UPDATE fails because node doesn't exist, try CREATE instead
                    // This handles cases where persistedNodeIds is out of sync (page reload, database reset)
                    // Match both "NodeNotFound" and "does not exist" error messages
                    if (
                      updateError instanceof Error &&
                      (updateError.message.includes('NodeNotFound') ||
                        updateError.message.includes('does not exist'))
                    ) {
                      console.warn(
                        `[SharedNodeStore] Node ${nodeId} not found in database, creating instead of updating`
                      );
                      await tauriCommands.createNode(updatedNode);
                      this.persistedNodeIds.add(nodeId); // Now it's persisted
                    } else {
                      // Re-throw other errors
                      throw updateError;
                    }
                  }
                } else {
                  // Node doesn't exist yet (was a placeholder or new node)
                  await tauriCommands.createNode(updatedNode);
                  this.persistedNodeIds.add(nodeId); // Track as persisted

                  // CRITICAL: Fetch the created node to get its version from backend
                  // This prevents version conflicts on subsequent updates
                  const createdNode = await tauriCommands.getNode(nodeId);
                  if (createdNode) {
                    const localNode = this.nodes.get(nodeId);
                    if (localNode) {
                      localNode.version = createdNode.version;
                      this.nodes.set(nodeId, localNode); // Update local node with backend version
                    }
                  }

                }
                // Mark update as persisted
                this.markUpdatePersisted(nodeId, update);
              } catch (dbError) {
                const error = dbError instanceof Error ? dbError : new Error(String(dbError));

                // Suppress expected errors in in-memory test mode
                if (shouldLogDatabaseErrors()) {
                  console.error(
                    `[SharedNodeStore] Database write failed for node ${nodeId}:`,
                    error,
                    '\nFull error object:',
                    JSON.stringify(dbError, null, 2)
                  );
                }

                // Always track errors in test environment for verification
                this.trackErrorIfTesting(error);

                // Rollback the optimistic update
                this.rollbackUpdate(nodeId, update);
                throw error; // Re-throw to mark operation as failed in coordinator
              }
            },
            {
              mode: isStructuralChange ? 'immediate' : 'debounce',
              dependencies: dependencies.length > 0 ? dependencies : undefined
            }
          );

          // Handle cancellation errors (expected when operations are superseded)
          handle.promise.catch((err) => {
            if (err instanceof OperationCancelledError) {
              // Operation was cancelled by a newer operation - this is expected
              return;
            }
            // Real errors are logged by PersistenceCoordinator
            // Re-throw would create unhandled rejection, so we silently handle
          });
        }
      }
    } catch (error) {
      console.error(`[SharedNodeStore] Error updating node ${nodeId}:`, error);
      throw error;
    } finally {
      const duration = performance.now() - startTime;
      this.recordMetric(duration);
    }
  }

  /**
   * Batch update multiple nodes
   */
  updateNodes(
    updates: Array<{ nodeId: string; changes: Partial<Node> }>,
    source: UpdateSource,
    options: UpdateOptions = {}
  ): void {
    for (const { nodeId, changes } of updates) {
      this.updateNode(nodeId, changes, source, options);
    }
  }

  /**
   * Set a node (create or replace)
   */
  setNode(node: Node, source: UpdateSource, skipPersistence = false): void {
    const isNewNode = !this.persistedNodeIds.has(node.id);

    // Track hierarchy changes for logging
    // New nodes trigger hierarchy change, content-only updates do not
    const existingNode = this.nodes.get(node.id);
    const isHierarchyChange = !existingNode;

    this.nodes.set(node.id, node);
    this.versions.set(node.id, this.getNextVersion(node.id));
    this.notifySubscribers(node.id, node, source);

    if (isHierarchyChange) {
      console.debug(`[SharedNodeStore] Hierarchy change for node: ${node.id}`);
    }

    // Determine persistence behavior using new explicit API
    const options: UpdateOptions = { skipPersistence };
    const { shouldMarkAsPersisted } = this.determinePersistenceBehavior(source, options);

    // Mark as persisted if explicitly requested or loaded from backend
    if (shouldMarkAsPersisted) {
      this.persistedNodeIds.add(node.id);
    }

    // Phase 2.4: Persist to database
    // IMPORTANT: For NEW nodes from viewer, persist immediately (including blank nodes!)
    // For UPDATES from viewer, skip persistence - BaseNodeViewer handles with debouncing
    // This ensures createNode() persistence works while avoiding duplicate writes on updates
    //
    // Phase 1 of Issue #479: Eliminate ephemeral nodes during editing
    // - Only skip persistence when explicitly requested via skipPersistence flag
    // - This flag is ONLY true for initial viewer placeholder (when no children exist)
    // - All other blank nodes (created via Enter key, etc.) persist immediately
    const persistBehavior = this.determinePersistenceBehavior(source, options);
    if (persistBehavior.shouldPersist) {
      const shouldPersist = source.type !== 'viewer' || isNewNode;

      if (shouldPersist) {
        // Issue #479: No placeholder checks - all real nodes should be persisted

        // Delegate to PersistenceCoordinator
        // CRITICAL FIX: Track insertAfterNodeId as dependency to prevent race conditions
        // When creating a node with insertAfterNodeId, the referenced node MUST exist in DB first
        // Otherwise backend fails with "Node 'xyz' does not exist"
        const dependencies: Array<string | (() => Promise<void>)> = [];

        // If this node references another node via insertAfterNodeId, wait for that node first
        const insertAfterNodeId = (node as Node & { insertAfterNodeId?: string | null }).insertAfterNodeId;
        if (insertAfterNodeId && !this.persistedNodeIds.has(insertAfterNodeId)) {
          dependencies.push(insertAfterNodeId);
        }

        // Issue #479: Always persist the full node including content
        // Real nodes (even with only syntax like "## ") must include content field for backend validation
        // The old code stripped content for "placeholder" header nodes, but now all user-created nodes
        // should persist with their full content, even if it's just syntax

        // Capture handle to catch cancellation errors
        const handle = PersistenceCoordinator.getInstance().persist(
          node.id,
          async () => {
            try {
              // Check if node has been persisted - use in-memory tracking to avoid database query
              const isPersistedToDatabase = this.persistedNodeIds.has(node.id);
              if (isPersistedToDatabase) {
                try {
                  // Get current version for optimistic concurrency control
                  const currentVersion = node.version ?? 1;
                  await tauriCommands.updateNode(node.id, currentVersion, node);
                } catch (updateError) {
                  // If UPDATE fails because node doesn't exist, try CREATE instead
                  const errorMessage =
                    updateError instanceof Error
                      ? updateError.message.toLowerCase()
                      : String(updateError).toLowerCase();
                  const isNodeNotFound =
                    errorMessage.includes('nodenotfound') ||
                    errorMessage.includes('not found') ||
                    errorMessage.includes('does not exist');

                  if (isNodeNotFound) {
                    console.warn(
                      `[SharedNodeStore] Node ${node.id} not found in database, creating instead of updating (error: ${errorMessage})`
                    );
                    await tauriCommands.createNode(node);
                    this.persistedNodeIds.add(node.id);
                  } else {
                    throw updateError;
                  }
                }
              } else {
                await tauriCommands.createNode(node);
                this.persistedNodeIds.add(node.id); // Track as persisted

                // CRITICAL: Fetch the created node to get its version from backend
                // This prevents version conflicts on subsequent updates
                const createdNode = await tauriCommands.getNode(node.id);
                if (createdNode) {
                  node.version = createdNode.version;
                  this.nodes.set(node.id, node); // Update local node with backend version
                }
              }
            } catch (dbError) {
              // Properly stringify Tauri errors which come as plain objects
              const errorMessage = dbError instanceof Error
                ? dbError.message
                : typeof dbError === 'object' && dbError !== null
                  ? JSON.stringify(dbError)
                  : String(dbError);
              const error = dbError instanceof Error ? dbError : new Error(errorMessage);

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                console.error(
                  `[SharedNodeStore] Database write failed for node ${node.id}:`,
                  errorMessage
                );
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              throw error; // Re-throw to mark operation as failed in coordinator
            }
          },
          {
            // CRITICAL: Use debounce mode for new viewer nodes to coalesce rapid updates
            // This prevents double-write where setNode() CREATE fires immediately,
            // then updateNode() fires debounced UPDATE shortly after
            mode: source.type === 'viewer' && isNewNode ? 'debounce' : 'immediate',
            dependencies: dependencies.length > 0 ? dependencies : undefined
          }
        );

        // Handle cancellation errors (expected when operations are superseded)
        handle.promise.catch((err) => {
          if (err instanceof OperationCancelledError) {
            // Operation was cancelled by a newer operation - this is expected
            return;
          }
          // Real errors are logged by PersistenceCoordinator
          // Re-throw would create unhandled rejection, so we silently handle
        });
      }
    }
  }

  /**
   * Batch set multiple nodes (optimized for bulk loading)
   *
   * This method adds multiple nodes to the store in a single operation,
   * triggering only ONE subscriber notification cycle instead of N separate cycles.
   *
   * Performance benefits:
   * - Single "hierarchy change" log instead of N logs
   * - One wildcard subscriber notification instead of N
   * - Reduced reactive update overhead
   *
   * @param nodes - Array of nodes to add
   * @param source - Source of the batch operation
   * @param skipPersistence - Skip database persistence (default: false)
   */
  batchSetNodes(nodes: Node[], source: UpdateSource, skipPersistence = false): void {
    if (nodes.length === 0) return;

    // Start batching notifications
    this.isBatchingNotifications = true;
    this.batchedNotifications.clear();

    // Track if any node is a hierarchy change
    let hasHierarchyChanges = false;

    // Add all nodes to the store
    for (const node of nodes) {
      const existingNode = this.nodes.get(node.id);
      const isHierarchyChange = !existingNode;

      if (isHierarchyChange) {
        hasHierarchyChanges = true;
      }

      this.nodes.set(node.id, node);
      this.versions.set(node.id, this.getNextVersion(node.id));

      // Defer notification - collect for batch
      this.batchedNotifications.set(node.id, { node, source });

      // Determine persistence behavior
      const options: UpdateOptions = { skipPersistence };
      const { shouldMarkAsPersisted } = this.determinePersistenceBehavior(source, options);

      if (shouldMarkAsPersisted) {
        this.persistedNodeIds.add(node.id);
      }
    }

    // End batching and send all notifications
    this.isBatchingNotifications = false;

    // Single hierarchy change log for entire batch
    if (hasHierarchyChanges) {
      console.debug(`[SharedNodeStore] Batch hierarchy change: ${nodes.length} nodes added`);
    }

    // Notify all subscribers once per node (but all in same microtask)
    for (const [nodeId, { node, source: nodeSource }] of this.batchedNotifications) {
      this.notifySubscribers(nodeId, node, nodeSource);
    }
    this.batchedNotifications.clear();

    // Note: Persistence is NOT batched - each node persists independently via PersistenceCoordinator
    // This is intentional to maintain individual debouncing and conflict detection per node
  }

  /**
   * Delete a node
   *
   * @param nodeId - ID of node to delete
   * @param source - Source of the deletion
   * @param skipPersistence - Skip database persistence (default: false)
   * @param dependencies - Node IDs that must be persisted before deletion (prevents FOREIGN KEY violations)
   */
  deleteNode(
    nodeId: string,
    source: UpdateSource,
    skipPersistence = false,
    dependencies: string[] = []
  ): void {
    // Cancel any active batch before deletion
    this.cancelBatch(nodeId);

    const node = this.nodes.get(nodeId);
    if (node) {
      this.nodes.delete(nodeId);
      this.versions.delete(nodeId);
      this.pendingUpdates.delete(nodeId);
      this.persistedNodeIds.delete(nodeId); // Remove from tracking set
      this.notifySubscribers(nodeId, node, source);

      console.debug(`[SharedNodeStore] Node deleted: ${nodeId}`);

      // Phase 2.4: Persist deletion to database
      const persistBehavior = this.determinePersistenceBehavior(source, { skipPersistence });
      if (persistBehavior.shouldPersist) {
        // Filter dependencies to only include nodes with pending persistence operations
        const pendingDeps = dependencies.filter((depId) =>
          PersistenceCoordinator.getInstance().isPending(depId)
        );

        // Capture handle to catch cancellation errors
        const handle = PersistenceCoordinator.getInstance().persist(
          nodeId,
          async () => {
            try {
              // Get current version for optimistic concurrency control
              // Note: node has already been removed from this.nodes, so we use the captured node variable
              const currentVersion = node.version ?? 1;
              await tauriCommands.deleteNode(nodeId, currentVersion);
            } catch (dbError) {
              const error = dbError instanceof Error ? dbError : new Error(String(dbError));

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                console.error(
                  `[SharedNodeStore] Database deletion failed for node ${nodeId}:`,
                  error
                );
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              throw error; // Re-throw to mark operation as failed in coordinator
            }
          },
          {
            mode: 'immediate',
            dependencies: pendingDeps.length > 0 ? pendingDeps : undefined
          }
        );

        // Handle cancellation errors (expected when operations are superseded)
        handle.promise.catch((err) => {
          if (err instanceof OperationCancelledError) {
            // Operation was cancelled by a newer operation - this is expected
            return;
          }
          // Real errors are logged by PersistenceCoordinator
          // Re-throw would create unhandled rejection, so we silently handle
        });
      }
    }
  }

  /**
   * Update a task node with type-safe spoke field updates (Issue #709)
   *
   * Routes task-specific field updates (status, priority, dueDate, assignee) through
   * the type-safe update path that directly modifies the spoke table in the backend.
   *
   * This method provides end-to-end type safety for task updates:
   * - Frontend sends TaskNodeUpdate (not generic NodeUpdate)
   * - Backend updates spoke table fields directly (not via JSON properties)
   * - Returns TaskNode with updated fields and new version
   *
   * @param nodeId - Task node ID to update
   * @param update - TaskNodeUpdate with spoke fields to update
   * @param source - Source of the update (viewer, database, MCP)
   */
  updateTaskNode(
    nodeId: string,
    update: import('$lib/types').TaskNodeUpdate,
    source: UpdateSource
  ): void {
    const existingNode = this.nodes.get(nodeId);
    if (!existingNode) {
      console.warn(`[SharedNodeStore] Cannot update non-existent task node: ${nodeId}`);
      return;
    }

    if (existingNode.nodeType !== 'task') {
      console.warn(`[SharedNodeStore] updateTaskNode called on non-task node: ${nodeId} (type: ${existingNode.nodeType})`);
      return;
    }

    // Apply update optimistically to local state
    // Map TaskNodeUpdate fields to Node properties for local state
    const localChanges: Partial<import('$lib/types').Node> = {};
    if (update.status !== undefined) {
      // Status is stored as a flat field on TaskNode
      (localChanges as Record<string, unknown>)['status'] = update.status;
    }
    if (update.priority !== undefined) {
      (localChanges as Record<string, unknown>)['priority'] = update.priority;
    }
    if (update.dueDate !== undefined) {
      (localChanges as Record<string, unknown>)['dueDate'] = update.dueDate;
    }
    if (update.assignee !== undefined) {
      (localChanges as Record<string, unknown>)['assignee'] = update.assignee;
    }
    if (update.content !== undefined) {
      localChanges.content = update.content;
    }

    // Update local node optimistically
    const updatedNode = { ...existingNode, ...localChanges };
    this.nodes.set(nodeId, updatedNode);
    this.notifySubscribers(nodeId, updatedNode, source);

    // Persist to backend via type-safe task update
    const currentVersion = existingNode.version ?? 1;

    // Capture handle to catch cancellation errors
    const handle = PersistenceCoordinator.getInstance().persist(
      nodeId,
      async () => {
        try {
          const updatedTaskNode = await tauriCommands.updateTaskNode(
            nodeId,
            currentVersion,
            update
          );

          // Update local node with backend version
          const localNode = this.nodes.get(nodeId);
          if (localNode && updatedTaskNode) {
            localNode.version = updatedTaskNode.version;
            // Also update spoke fields from backend response
            // Use Object.assign to safely update fields that may not exist on Node interface
            Object.assign(localNode, {
              status: updatedTaskNode.status,
              priority: updatedTaskNode.priority,
              dueDate: updatedTaskNode.dueDate,
              assignee: updatedTaskNode.assignee
            });
            if (updatedTaskNode.content !== undefined) {
              localNode.content = updatedTaskNode.content;
            }
            this.nodes.set(nodeId, localNode);
          }
        } catch (dbError) {
          const error = dbError instanceof Error ? dbError : new Error(String(dbError));

          // Suppress expected errors in in-memory test mode
          if (shouldLogDatabaseErrors()) {
            console.error(
              `[SharedNodeStore] Task update failed for node ${nodeId}:`,
              error
            );
          }

          // Always track errors in test environment for verification
          this.trackErrorIfTesting(error);

          // Rollback the optimistic update
          this.nodes.set(nodeId, existingNode);
          this.notifySubscribers(nodeId, existingNode, source);

          throw error;
        }
      },
      {
        mode: 'immediate' // Task status updates should be immediate (not debounced)
      }
    );

    // Handle cancellation errors (expected when operations are superseded)
    handle.promise.catch((err) => {
      if (err instanceof OperationCancelledError) {
        return; // Expected - operation was cancelled by a newer operation
      }
    });
  }

  /**
   * Clear all nodes (for testing)
   */
  clearAll(): void {
    this.nodes.clear();
    this.versions.clear();
    this.pendingUpdates.clear();
    this.persistedNodeIds.clear();
    this.notifyAllSubscribers();
  }

  // ========================================================================
  // New Methods for BaseNodeViewer Migration (Issue #237)
  // ========================================================================

  /**
   * Load direct child nodes from database for a parent
   *
   * Note: This loads only direct children, not all descendants.
   * For recursive loading, use getDescendants().
   *
   * @param parentId - The parent node ID
   * @returns Array of direct child nodes loaded from database
   */
  async loadChildrenForParent(parentId: string): Promise<Node[]> {
    try {
      const nodes = await tauriCommands.getChildren(parentId);

      // Add nodes to store with database source
      // Database source type will automatically mark nodes as persisted (see determinePersistenceBehavior)
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };
      for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        this.setNode(node, databaseSource); // skipPersistence removed - database source handles it

        // CRITICAL FIX: Register parent-child edge in structureTree for browser mode
        // In Tauri mode, domain events populate structureTree automatically.
        // In browser mode (HTTP adapter), we must register edges manually here.
        // Use index as order since backend returns children in sorted order.
        structureTree.addInMemoryRelationship(parentId, node.id, i + 1);
      }

      return nodes;
    } catch (error) {
      // Suppress expected errors in in-memory test mode
      if (shouldLogDatabaseErrors()) {
        console.error(`[SharedNodeStore] Failed to load children for parent ${parentId}:`, error);
      }

      throw error;
    }
  }

  /**
   * Load entire children tree recursively from database for a parent
   *
   * This method uses getChildrenTree which returns nested NodeWithChildren structure.
   * It recursively flattens all nodes into the store and registers ALL parent-child
   * edges in the structureTree, enabling proper expand/collapse for nested hierarchies.
   *
   * CRITICAL FOR BROWSER MODE: In Tauri mode, domain events populate the
   * structureTree automatically. In browser mode (HTTP adapter), we must load
   * the entire tree upfront and register edges manually.
   *
   * @param parentId - The parent node ID to load tree for
   * @returns Array of ALL nodes (flattened) loaded from database
   */
  async loadChildrenTree(parentId: string): Promise<Node[]> {
    // Check if a load is already in progress for this parent
    const existingLoad = this.pendingTreeLoads.get(parentId);
    if (existingLoad) {
      return existingLoad;
    }

    // Create new load promise and track it
    const loadPromise = this.doLoadChildrenTree(parentId);
    this.pendingTreeLoads.set(parentId, loadPromise);

    try {
      const result = await loadPromise;
      return result;
    } finally {
      // Clean up tracking after load completes (success or failure)
      this.pendingTreeLoads.delete(parentId);
    }
  }

  private async doLoadChildrenTree(parentId: string): Promise<Node[]> {
    try {
      const tree = await tauriCommands.getChildrenTree(parentId);

      if (!tree) {
        return [];
      }

      const allNodes: Node[] = [];
      const allRelationships: Array<{parentId: string, childId: string, order: number}> = [];
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };

      // OPTIMIZATION: Add parent node itself to the store
      // This eliminates the need for a separate getNode() call in base-node-viewer
      const { children: _children, ...parentNodeFields } = tree;
      const parentNode: Node = parentNodeFields as Node;
      allNodes.push(parentNode);

      // Helper to recursively process NodeWithChildren and collect nodes + edges
      // OPTIMIZED: Collects all nodes first, then batch adds them
      const processNode = (nodeWithChildren: import('$lib/types').NodeWithChildren, nodeParentId: string, order: number) => {
        // Extract Node fields (exclude 'children' property)

        const { children, ...nodeFields } = nodeWithChildren;
        const node: Node = nodeFields as Node;

        // Collect node (don't add to store yet - batched later)
        allNodes.push(node);

        // Collect parent-child edge (don't add to structureTree yet)
        allRelationships.push({ parentId: nodeParentId, childId: node.id, order });

        // Recursively process children
        if (children && children.length > 0) {
          for (let i = 0; i < children.length; i++) {
            processNode(children[i], node.id, i + 1);
          }
        }
      };

      // Process all direct children of the parent
      if (tree.children && tree.children.length > 0) {
        for (let i = 0; i < tree.children.length; i++) {
          processNode(tree.children[i], parentId, i + 1);
        }
      }

      // OPTIMIZATION: Batch add all nodes at once (single notification cycle)
      if (allNodes.length > 0) {
        this.batchSetNodes(allNodes, databaseSource);
      }

      // Batch register all relationships to avoid effect loops
      // This triggers only ONE reactivity update instead of N updates
      // Filter out relationships that already exist to avoid duplicate detection overhead
      if (allRelationships.length > 0) {
        // OPTIMIZATION: Build Set of existing child IDs across all parents (one pass)
        // This avoids O(n²) complexity from repeated getChildren() calls in filter
        const existingChildIds = new Set<string>();
        const seenParents = new Set<string>();

        for (const rel of allRelationships) {
          if (!seenParents.has(rel.parentId)) {
            seenParents.add(rel.parentId);
            const children = structureTree.getChildren(rel.parentId);
            children.forEach(id => existingChildIds.add(id));
          }
        }

        // Filter using Set lookup (O(1) per check instead of O(n))
        const newRelationships = allRelationships.filter(rel =>
          !existingChildIds.has(rel.childId)
        );

        if (newRelationships.length > 0) {
          structureTree.batchAddRelationships(newRelationships);
        }
      }

      return allNodes;
    } catch (error) {
      // Suppress expected errors in in-memory test mode
      if (shouldLogDatabaseErrors()) {
        console.error(`[SharedNodeStore] Failed to load children tree for parent ${parentId}:`, error);
      }

      throw error;
    }
  }

  /**
   * Check if a node has been persisted to the database
   * @param nodeId - Node ID to check
   * @returns True if node exists in database, false if only in memory
   */
  isNodePersisted(nodeId: string): boolean {
    return this.persistedNodeIds.has(nodeId);
  }

  /**
   * Check if a node has a pending save operation
   * Delegates to PersistenceCoordinator
   *
   * @param nodeId - Node ID to check
   * @returns True if save is pending
   */
  hasPendingSave(nodeId: string): boolean {
    return PersistenceCoordinator.getInstance().isPending(nodeId);
  }

  /**
   * Wait for pending node saves to complete with timeout
   * Delegates to PersistenceCoordinator
   *
   * @param nodeIds - Array of node IDs to wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async waitForNodeSaves(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    return PersistenceCoordinator.getInstance().waitForPersistence(nodeIds, timeoutMs);
  }

  /**
   * Flush ALL pending saves and wait for completion.
   *
   * This ensures the entire pending operation queue is cleared before proceeding.
   * Use this for structural operations like moveNode that may depend on edges
   * created by any pending save.
   *
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async flushAllPendingSaves(timeoutMs = 5000): Promise<Set<string>> {
    return PersistenceCoordinator.getInstance().flushAll(timeoutMs);
  }

  /**
   * Get the current count of pending persistence operations.
   * Useful for debugging race conditions.
   */
  getPendingOperationsCount(): number {
    return PersistenceCoordinator.getInstance().getMetrics().pendingOperations;
  }

  /**
   * DEPRECATED: Ensure entire ancestor chain is persisted before persisting a child node
   *
   * This method is now a no-op. Parent/child relationships are managed via graph edges
   * in the backend, not via frontend foreign key tracking.
   *
   * @param _nodeId - Starting node ID to walk ancestors from (unused)
   */
  private async ensureAncestorChainPersisted(_nodeId: string): Promise<void> {
    // No-op: Backend handles parent/child relationships via graph edges
    return Promise.resolve();
  }

  /**
   * Validate node references before update
   *
   * @param nodeId - Node to validate
   * @returns Validation result with any errors
   *
   * @deprecated Structural changes (beforeSiblingId) are now handled via backend moveNode().
   * This method is kept for backward compatibility but only validates node existence.
   */
  async validateNodeReferences(
    nodeId: string
  ): Promise<{
    errors: string[];
  }> {
    const errors: string[] = [];

    // Validate node still exists
    if (!this.hasNode(nodeId)) {
      errors.push(`Node ${nodeId} not found (may have been deleted)`);
    }

    return { errors };
  }

  // ========================================================================
  // Phase 3: External Update Handling (MCP-Ready)
  // ========================================================================

  /**
   * Handle updates from external sources (MCP server, database sync, etc.)
   *
   * This method provides the integration point for future MCP server (#112).
   * It routes external updates through the same conflict detection and
   * synchronization pipeline as local edits.
   *
   * @param source - Source type: 'mcp-server', 'database', or 'external'
   * @param update - The node update to apply
   *
   * @example
   * // Future: When MCP server from #112 is ready
   * mcpServer.on('node:updated', (mcpUpdate) => {
   *   sharedStore.handleExternalUpdate('mcp-server', mcpUpdate);
   * });
   *
   * @example
   * // Current: Simulated MCP update for testing
   * const mcpUpdate = {
   *   nodeId: 'test-node',
   *   changes: { content: 'Updated by AI agent' },
   *   source: { type: 'mcp-server' as const, serverId: 'test-server' },
   *   timestamp: Date.now()
   * };
   * sharedStore.handleExternalUpdate('mcp-server', mcpUpdate);
   */
  handleExternalUpdate(
    sourceType: 'mcp-server' | 'database' | 'external',
    update: NodeUpdate
  ): void {
    // Validate the node exists
    if (!this.nodes.has(update.nodeId)) {
      console.warn(
        `[SharedNodeStore] External update for non-existent node: ${update.nodeId} from ${sourceType}`
      );
      return;
    }

    // Apply the update through standard pipeline
    // This ensures:
    // - Conflict detection happens
    // - All viewers are notified
    // - Metrics are tracked
    // - Events are emitted
    this.updateNode(update.nodeId, update.changes, update.source, {
      // External updates from database should skip persistence to avoid loops
      skipPersistence: sourceType === 'database',
      // MCP server updates should go through conflict detection
      skipConflictDetection: false
    });
  }

  // ========================================================================
  // Conflict Detection and Resolution
  // ========================================================================

  /**
   * Detect if an update conflicts with pending updates
   */
  private detectConflict(nodeId: string, incomingUpdate: NodeUpdate): Conflict | null {
    const pending = this.pendingUpdates.get(nodeId);
    if (!pending || pending.length === 0) {
      return null;
    }

    // Check for version mismatch
    if (
      incomingUpdate.previousVersion !== undefined &&
      this.versions.get(nodeId) !== incomingUpdate.previousVersion
    ) {
      // Version mismatch - concurrent edit detected
      const lastPending = pending[pending.length - 1];
      return {
        nodeId,
        localUpdate: lastPending,
        remoteUpdate: incomingUpdate,
        conflictType: 'version-mismatch',
        detectedAt: Date.now()
      };
    }

    // Check for concurrent edits (same field modified within time window)
    for (const pendingUpdate of pending) {
      const timeDiff = Math.abs(incomingUpdate.timestamp - pendingUpdate.timestamp);
      if (timeDiff < this.conflictWindowMs) {
        // Check if same fields were modified
        const incomingFields = new Set(Object.keys(incomingUpdate.changes));
        const pendingFields = new Set(Object.keys(pendingUpdate.changes));
        const overlap = [...incomingFields].some((field) => pendingFields.has(field));

        if (overlap) {
          // SPECIAL CASE: NodeType changes with content are not conflicts - they're pattern-triggered conversions
          // Allow nodeType updates to proceed even if content overlaps with pending update
          const isNodeTypeConversion = 'nodeType' in incomingUpdate.changes;
          if (isNodeTypeConversion) {
            // Allow this update - it's a pattern-triggered conversion, not a conflict
            continue;
          }

          return {
            nodeId,
            localUpdate: pendingUpdate,
            remoteUpdate: incomingUpdate,
            conflictType: 'concurrent-edit',
            detectedAt: Date.now()
          };
        }
      }
    }

    return null;
  }

  /**
   * Handle detected conflict
   */
  private handleConflict(conflict: Conflict): void {
    this.metrics.conflictCount++;

    // Get existing node for type-safe resolution
    const existingNode = this.nodes.get(conflict.nodeId);
    if (!existingNode) {
      console.warn(
        `[SharedNodeStore] Cannot resolve conflict for non-existent node: ${conflict.nodeId}`
      );
      return;
    }

    // Resolve using configured strategy
    const resolution = this.conflictResolver.resolve(conflict, existingNode);

    // Apply resolved state
    this.nodes.set(conflict.nodeId, resolution.resolvedNode);
    this.versions.set(conflict.nodeId, this.getNextVersion(conflict.nodeId));

    console.debug(`[SharedNodeStore] Conflict resolved for node: ${conflict.nodeId}, strategy: ${resolution.strategy}`);

    // Notify subscribers
    this.notifySubscribers(conflict.nodeId, resolution.resolvedNode, conflict.remoteUpdate.source);
  }

  /**
   * Set conflict resolver strategy
   */
  setConflictResolver(resolver: ConflictResolver): void {
    this.conflictResolver = resolver;
  }

  /**
   * Get current conflict resolver
   */
  getConflictResolver(): ConflictResolver {
    return this.conflictResolver;
  }

  /**
   * Set conflict detection window (in milliseconds)
   * Updates within this time window are considered potentially concurrent
   */
  setConflictWindow(ms: number): void {
    this.conflictWindowMs = ms;
  }

  // ========================================================================
  // Rollback Support (Optimistic Updates)
  // ========================================================================

  /**
   * Rollback a pending update (e.g., if database write fails)
   */
  rollbackUpdate(nodeId: string, updateToRollback: NodeUpdate): void {
    this.metrics.rollbackCount++;

    const pending = this.pendingUpdates.get(nodeId);
    if (!pending) return;

    // Remove the failed update from pending
    const index = pending.indexOf(updateToRollback);
    if (index > -1) {
      pending.splice(index, 1);
    }

    // Rollback to previous version
    const previousVersion = updateToRollback.previousVersion;
    if (previousVersion !== undefined) {
      this.versions.set(nodeId, previousVersion);
    }

    // Notify subscribers about rollback
    const currentNode = this.nodes.get(nodeId);
    if (currentNode) {
      this.notifySubscribers(nodeId, currentNode, updateToRollback.source);
    }

    console.debug(`[SharedNodeStore] Update rolled back for node: ${nodeId}`);
  }

  /**
   * Mark an update as persisted (remove from pending)
   */
  markUpdatePersisted(nodeId: string, update: NodeUpdate): void {
    const pending = this.pendingUpdates.get(nodeId);
    if (!pending) return;

    const index = pending.indexOf(update);
    if (index > -1) {
      pending.splice(index, 1);
    }

    // Clean up if no more pending updates
    if (pending.length === 0) {
      this.pendingUpdates.delete(nodeId);
    }
  }

  // ========================================================================
  // Subscription Management (Observer Pattern)
  // ========================================================================

  /**
   * Subscribe to changes for a specific node
   */
  subscribe(nodeId: string, callback: NodeChangeCallback): Unsubscribe {
    const subscription: Subscription = {
      id: `sub_${this.subscriptionIdCounter++}`,
      nodeId,
      callback,
      createdAt: Date.now(),
      callCount: 0
    };

    if (!this.subscriptions.has(nodeId)) {
      this.subscriptions.set(nodeId, new Set());
    }
    this.subscriptions.get(nodeId)!.add(subscription);
    this.metrics.subscriptionCount++;

    // Return unsubscribe function
    return () => {
      const subs = this.subscriptions.get(nodeId);
      if (subs) {
        subs.delete(subscription);
        if (subs.size === 0) {
          this.subscriptions.delete(nodeId);
        }
      }
      this.metrics.subscriptionCount--;
    };
  }

  /**
   * Subscribe to all node changes (wildcard)
   */
  subscribeAll(callback: NodeChangeCallback): Unsubscribe {
    const subscription: Subscription = {
      id: `sub_wildcard_${this.subscriptionIdCounter++}`,
      nodeId: null,
      callback,
      createdAt: Date.now(),
      callCount: 0
    };

    this.wildcardSubscriptions.add(subscription);
    this.metrics.subscriptionCount++;

    return () => {
      this.wildcardSubscriptions.delete(subscription);
      this.metrics.subscriptionCount--;
    };
  }

  /**
   * Notify subscribers of a node change
   */
  private notifySubscribers(nodeId: string, node: Node, source: UpdateSource): void {
    // Notify node-specific subscribers
    const subs = this.subscriptions.get(nodeId);
    if (subs) {
      for (const sub of subs) {
        try {
          sub.callback(node, source);
          sub.callCount++;
        } catch (error) {
          console.error(`[SharedNodeStore] Subscription callback error:`, error);
        }
      }
    }

    // Notify wildcard subscribers
    for (const sub of this.wildcardSubscriptions) {
      try {
        sub.callback(node, source);
        sub.callCount++;
      } catch (error) {
        console.error(`[SharedNodeStore] Wildcard subscription callback error:`, error);
      }
    }
  }

  /**
   * Notify all subscribers (e.g., on clear)
   */
  private notifyAllSubscribers(): void {
    // Notify all node-specific subscribers
    for (const [nodeId, subs] of this.subscriptions) {
      const node = this.nodes.get(nodeId);
      if (node) {
        for (const sub of subs) {
          try {
            sub.callback(node, { type: 'database', reason: 'store-cleared' });
          } catch (error) {
            console.error(`[SharedNodeStore] Subscription callback error:`, error);
          }
        }
      }
    }
  }

  /**
   * Determine the type of update based on which fields changed
   *
   * @param changes - Partial node data representing the changes
   * @returns 'structure' for hierarchy changes, 'metadata' for computed fields, 'content' otherwise
   */
  private determineUpdateType(changes: Partial<Node>): 'content' | 'structure' | 'metadata' {
    // Structural changes (hierarchy/ordering) are now handled via backend moveNode()
    // Frontend no longer tracks beforeSiblingId, so we skip structure detection

    // Metadata-only changes (computed fields that don't affect content)
    if (this.isMetadataOnlyUpdate(changes)) {
      return 'metadata';
    }

    return 'content';
  }

  /**
   * Check if an update only modifies metadata (computed fields)
   *
   * @param changes - Partial node data representing the changes
   * @returns true if only computed/derived fields changed
   */
  private isMetadataOnlyUpdate(changes: Partial<Node>): boolean {
    // Currently only mentions are metadata-only (computed from content)
    // Future: Could include other computed fields (tags, backlinks, etc.)
    return 'mentions' in changes && Object.keys(changes).length === 1;
  }

  // ========================================================================
  // Performance Metrics
  // ========================================================================

  /**
   * Get performance metrics
   */
  getMetrics(): StoreMetrics {
    return { ...this.metrics };
  }

  /**
   * Reset metrics (for testing)
   */
  resetMetrics(): void {
    this.metrics = {
      updateCount: 0,
      avgUpdateTime: 0,
      maxUpdateTime: 0,
      subscriptionCount: this.metrics.subscriptionCount, // Keep subscription count
      conflictCount: 0,
      rollbackCount: 0
    };
  }

  /**
   * Record operation timing
   */
  private recordMetric(duration: number): void {
    const count = this.metrics.updateCount;
    const currentAvg = this.metrics.avgUpdateTime;
    this.metrics.avgUpdateTime = (currentAvg * (count - 1) + duration) / count;
    this.metrics.maxUpdateTime = Math.max(this.metrics.maxUpdateTime, duration);
  }

  // ========================================================================
  // Version Management
  // ========================================================================

  /**
   * Get next version number for a node
   */
  private getNextVersion(nodeId: string): number {
    const current = this.versions.get(nodeId) || 0;
    return current + 1;
  }

  /**
   * Get current version of a node
   */
  getVersion(nodeId: string): number {
    return this.versions.get(nodeId) || 0;
  }

  // ========================================================================
  // Atomic Batch Updates
  // ========================================================================

  /**
   * Start an atomic batch update for a node
   * All subsequent updates for this nodeId will be accumulated until commitBatch()
   *
   * Use this for pattern conversions where content + nodeType must persist together:
   * - Quote blocks: content change + nodeType change must be atomic
   * - Code blocks: content change + nodeType change must be atomic
   * - Ordered lists: content change + nodeType change must be atomic
   *
   * @param nodeId - Node to batch updates for
   * @param timeoutMs - Auto-commit timeout in ms (default: DEFAULT_BATCH_TIMEOUT_MS = 2000ms)
   * @returns Batch ID for tracking
   *
   * @example
   * ```typescript
   * const batchId = store.startBatch(nodeId);
   * store.addToBatch(nodeId, { content: '> Quote text' });
   * store.addToBatch(nodeId, { nodeType: 'quote-block' });
   * store.commitBatch(nodeId); // Atomically persists both changes
   * ```
   */
  startBatch(nodeId: string, timeoutMs = DEFAULT_BATCH_TIMEOUT_MS): string {
    // Cancel existing batch if any (ensures clean state)
    this.cancelBatch(nodeId);

    // CRITICAL: Cancel any pending non-batched persistence operations
    // This prevents race between old debounced updates and new batch
    PersistenceCoordinator.getInstance().cancelPending(nodeId);

    // Use counter-based batch ID to prevent timing collisions
    // (Date.now() can return same value for rapid successive calls)
    const batchId = `batch-${nodeId}-${this.batchIdCounter++}`;
    const createdAt = Date.now();

    // Auto-commit after timeout to prevent abandoned batches
    const timeout = setTimeout(() => {
      console.warn('[SharedNodeStore] Auto-committing batch after inactivity timeout', {
        batchId,
        nodeId,
        timeoutMs,
        age: Date.now() - createdAt
      });
      this.commitBatch(nodeId);
    }, timeoutMs);

    this.activeBatches.set(nodeId, {
      nodeId,
      changes: {},
      batchId,
      createdAt,
      timeout,
      timeoutMs
    });

    return batchId;
  }

  /**
   * Add changes to the active batch for a node
   * Changes are accumulated and merged (later changes override earlier ones)
   * Updates in-memory state immediately (optimistic update)
   *
   * @param nodeId - Node to update
   * @param changes - Partial node changes to add to batch
   *
   * @example
   * ```typescript
   * store.startBatch(nodeId);
   * store.addToBatch(nodeId, { content: '1. ' });         // First change
   * store.addToBatch(nodeId, { nodeType: 'ordered-list' }); // Second change
   * store.commitBatch(nodeId); // Both persist atomically
   * ```
   */
  addToBatch(nodeId: string, changes: Partial<Node>): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      console.warn('[SharedNodeStore] Attempted to add to non-existent batch', {
        nodeId,
        changes: Object.keys(changes)
      });
      return;
    }

    // Merge changes into batch (later changes override)
    Object.assign(batch.changes, changes);

    // Update in-memory state immediately (optimistic)
    const currentNode = this.nodes.get(nodeId);
    if (currentNode) {
      const updatedNode = { ...currentNode, ...changes };
      this.nodes.set(nodeId, updatedNode);

      // Notify subscribers of optimistic update
      this.notifySubscribers(nodeId, updatedNode, { type: 'viewer', viewerId: 'batch' });
    }

    // Reset timeout to extend batch lifetime while user is actively making changes
    // This ensures batch only commits after true inactivity (no changes for N seconds)
    this.resetBatchTimeout(nodeId);
  }

  /**
   * Commit an active batch atomically
   * Runs placeholder detection on final state and persists if not a placeholder
   *
   * Edge case handling:
   * - If node was previously persisted and becomes a placeholder (user deleted content),
   *   still persist to update database with empty/placeholder state
   *
   * @param nodeId - Node whose batch to commit
   */
  commitBatch(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      return; // No batch active, nothing to commit
    }

    // CRITICAL: Clear timeout and remove batch FIRST (ensures cleanup even on error)
    // This prevents memory leaks if persistBatchedChanges() throws
    clearTimeout(batch.timeout);
    this.activeBatches.delete(nodeId);

    try {
      // Get final node state after all batch changes
      const finalNode = this.nodes.get(nodeId);
      if (!finalNode) {
        console.warn('[SharedNodeStore] Batch commit aborted - node not found', {
          nodeId,
          batchId: batch.batchId
        });
        return;
      }

      // Nothing to persist if batch has no changes
      if (Object.keys(batch.changes).length === 0) {
        return;
      }

      // Issue #479: Always persist batched changes - even blank/syntax-only nodes
      // Real nodes (created by user actions) should always be persisted
      // The viewer-local placeholder never enters batch system
      this.persistBatchedChanges(nodeId, batch.changes, finalNode);
    } catch (error) {
      console.error('[SharedNodeStore] Batch commit error', {
        nodeId,
        batchId: batch.batchId,
        error
      });
      // Re-throw to surface to caller, but cleanup is already done
      throw error;
    }
  }

  /**
   * Cancel an active batch without persisting
   * Used when batch should be abandoned (e.g., node deleted during batch)
   *
   * @param nodeId - Node whose batch to cancel
   */
  cancelBatch(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (batch) {
      clearTimeout(batch.timeout);
      this.activeBatches.delete(nodeId);
    }
  }

  /**
   * Commit all active batches globally
   * Used when component unmounts to ensure all pending batched changes are saved
   */
  commitAllBatches(): void {
    const nodeIds = Array.from(this.activeBatches.keys());
    console.log('[SharedNodeStore] Committing all batches:', nodeIds.length, 'active');
    for (const nodeId of nodeIds) {
      this.commitBatch(nodeId);
    }
  }

  /**
   * Reset the auto-commit timeout for an active batch
   * Extends the batch lifetime when user continues making changes
   *
   * This implements "true inactivity" timeout:
   * - Timer resets on every change (content, nodeType, metadata, etc.)
   * - Batch only commits after N seconds of NO activity
   * - Prevents premature commits while user is actively typing
   *
   * @param nodeId - Node whose batch timeout to reset
   *
   * @example
   * ```typescript
   * store.startBatch(nodeId); // Start with default timeout (2s)
   * // ... user types ...
   * store.addToBatch(nodeId, { content: 'new' }); // Resets timeout to 2s
   * // ... user types more ...
   * store.addToBatch(nodeId, { content: 'newer' }); // Resets timeout to 2s again
   * // ... after 2s of no activity, auto-commit fires
   * ```
   */
  private resetBatchTimeout(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      return; // No batch active
    }

    // Clear existing timeout
    clearTimeout(batch.timeout);

    // Create new timeout with same duration
    const timeout = setTimeout(() => {
      this.commitBatch(nodeId);
    }, batch.timeoutMs);

    // Update batch with new timeout (keep other properties)
    batch.timeout = timeout;
  }

  /**
   * Persist batched changes atomically
   * Delegates to existing persistence infrastructure
   *
   * @param nodeId - Node to persist
   * @param changes - Accumulated changes from batch
   * @param finalNode - Final node state after batch
   */
  private persistBatchedChanges(nodeId: string, changes: Partial<Node>, finalNode: Node): void {
    const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);

    // Use PersistenceCoordinator for coordinated persistence
    // Sibling ordering is now managed via fractional position IDs in the backend
    // No frontend foreign key dependency tracking needed for beforeSiblingId
    const dependencies: Array<string | (() => Promise<void>)> = [];

    // Persist with immediate mode (batches should not be debounced)
    PersistenceCoordinator.getInstance().persist(
      nodeId,
      async () => {
        try {
          // RACE CONDITION HANDLING:
          // ========================
          // SCENARIO: User types "> text" in text node
          // t=0ms:    Content "> " queued for debounced persistence (500ms delay)
          // t=200ms:  Pattern detected → startBatch() called
          // t=300ms:  User continues typing → batched updates accumulate
          // t=500ms:  Debounced persistence fires → node persisted via old path (race!)
          // t=2200ms: Batch commits → tries CREATE but node already exists
          //
          // SOLUTION: Try CREATE first (standard case), but if it fails with UNIQUE constraint,
          // fall back to UPDATE with batched changes to fix the race
          //
          // STRATEGY: Try UPDATE first if we know node is persisted, otherwise CREATE
          if (isPersistedToDatabase) {
            // Get current version for optimistic concurrency control
            const currentVersion = finalNode.version ?? 1;

            // CRITICAL: Capture updated node to get new version from backend
            // This prevents version conflicts on subsequent updates
            const updatedNodeFromBackend = await tauriCommands.updateNode(nodeId, currentVersion, changes);

            // Update local node with backend version
            const localNode = this.nodes.get(nodeId);
            if (localNode && updatedNodeFromBackend) {
              localNode.version = updatedNodeFromBackend.version;
              this.nodes.set(nodeId, localNode);
            }
          } else {
            // Try CREATE, but handle race condition where old path persisted first
            try {
              await tauriCommands.createNode(finalNode);
              this.persistedNodeIds.add(nodeId);

              // CRITICAL: Fetch the created node to get its version from backend
              // This prevents version conflicts on subsequent updates
              const createdNode = await tauriCommands.getNode(nodeId);
              if (createdNode) {
                finalNode.version = createdNode.version;
                this.nodes.set(nodeId, finalNode); // Update local node with backend version
              }
            } catch (createError) {
              // If CREATE fails (node already exists from race), try UPDATE with batched changes
              if (
                createError instanceof Error &&
                (createError.message.includes('UNIQUE constraint') ||
                  createError.message.includes('already exists'))
              ) {
                // Race detected: Old debounced path persisted before batch started
                // Update with batched changes to fix inconsistent state
                const currentVersion = finalNode.version ?? 1;
                const updatedNodeFromBackend = await tauriCommands.updateNode(nodeId, currentVersion, changes);
                this.persistedNodeIds.add(nodeId);

                // Update local node with backend version
                const localNode = this.nodes.get(nodeId);
                if (localNode && updatedNodeFromBackend) {
                  localNode.version = updatedNodeFromBackend.version;
                  this.nodes.set(nodeId, localNode);
                }
              } else {
                throw createError;
              }
            }
          }
        } catch (dbError) {
          const error = dbError instanceof Error ? dbError : new Error(String(dbError));

          // Suppress expected errors in in-memory test mode
          if (shouldLogDatabaseErrors()) {
            console.error(`[SharedNodeStore] Batch persistence failed for node ${nodeId}:`, error);
          }

          // Always track errors in test environment for verification
          this.trackErrorIfTesting(error);

          throw error;
        }
      },
      {
        mode: 'immediate', // Batches are already accumulated, persist immediately
        dependencies: dependencies.length > 0 ? dependencies : undefined
      }
    );
  }

  // ========================================================================
  // Snapshot/Restore for Optimistic Rollback
  // ========================================================================

  /**
   * Take a snapshot of all nodes for optimistic rollback
   *
   * Creates a deep copy of the current node state that can be restored
   * if a backend operation fails. Used by OptimisticOperationManager.
   *
   * @returns Deep copy of all nodes as a Map
   */
  snapshot(): Map<string, Node> {
    const snapshotMap = new Map<string, Node>();
    for (const [nodeId, node] of this.nodes) {
      // Deep copy each node to prevent reference mutations
      snapshotMap.set(nodeId, { ...node });
    }
    return snapshotMap;
  }

  /**
   * Restore all nodes from a snapshot (rollback on error)
   *
   * Replaces the current node state with the snapshot state.
   * Used by OptimisticOperationManager when backend operations fail.
   *
   * @param snapshotMap - Previously captured snapshot to restore
   */
  restore(snapshotMap: Map<string, Node>): void {
    // Clear current nodes and restore from snapshot
    this.nodes.clear();
    for (const [nodeId, node] of snapshotMap) {
      this.nodes.set(nodeId, node);
    }

    // Notify all subscribers about the restore
    this.notifyAllSubscribers();
  }

  // ========================================================================
  // Test Utilities
  // ========================================================================

  /**
   * Check if there are pending database writes
   * Used by tests to wait for all writes to complete
   * Delegates to PersistenceCoordinator
   */
  hasPendingWrites(): boolean {
    const metrics = PersistenceCoordinator.getInstance().getMetrics();
    return metrics.pendingOperations > 0;
  }

  /**
   * Flush all pending persistence operations immediately.
   * Used on window close to prevent data loss.
   *
   * This will:
   * 1. Commit any active batches
   * 2. Execute all debounced persistence operations immediately
   *
   * @returns Promise that resolves when all pending operations complete
   */
  async flushAllPending(): Promise<void> {
    // First, commit all active batches
    this.commitAllBatches();

    // Then flush all pending persistence operations
    await PersistenceCoordinator.getInstance().flushPending();
  }

  /**
   * Get test errors (only populated in test environment)
   * Used by tests to verify database operations succeeded
   */
  getTestErrors(): Error[] {
    return [...this.testErrors];
  }

  /**
   * Track error in test environment for verification
   * Only adds errors when NODE_ENV='test'
   *
   * @param error - Error to track for test verification
   * @private
   */
  private trackErrorIfTesting(error: Error): void {
    if (isTestEnvironment()) {
      this.testErrors.push(error);
    }
  }

  /**
   * Clear test errors
   * Should be called at the start of each test for isolation
   */
  clearTestErrors(): void {
    this.testErrors = [];
  }

  /**
   * Reset store state (for testing only)
   * @internal
   */
  __resetForTesting(): void {
    this.nodes.clear();
    this.persistedNodeIds.clear();
    this.subscriptions.clear();
    this.wildcardSubscriptions.clear();
    this.pendingUpdates.clear();
    this.versions.clear();
    this.testErrors = [];

    // Cancel all active batches
    for (const [nodeId] of this.activeBatches) {
      this.cancelBatch(nodeId);
    }
    this.activeBatches.clear();

    this.metrics = {
      updateCount: 0,
      avgUpdateTime: 0,
      maxUpdateTime: 0,
      subscriptionCount: 0,
      conflictCount: 0,
      rollbackCount: 0
    };
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

/**
 * Singleton instance for application-wide use
 */
export const sharedNodeStore = SharedNodeStore.getInstance();

/**
 * Default export
 */
export default SharedNodeStore;
