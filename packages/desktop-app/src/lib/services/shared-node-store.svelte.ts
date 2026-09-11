/**
 * SharedNodeStore - Singleton Reactive Store for Multi-Viewer Support
 *
 * - Single source of truth for all node data (Svelte 5 $state)
 * - Observer pattern for viewer subscriptions
 * - Real-time synchronization across multiple viewers
 * - Optimistic updates with rollback
 * - Performance tracking and metrics
 *
 * Architecture:
 * - Singleton pattern ensures single shared store
 * - Multiple ReactiveNodeService instances read from same store
 * - Per-viewer UI state (expand/collapse, focus) stored separately
 * - Database writes serialized per-node by SimplePersistenceCoordinator
 */

import { SvelteMap } from 'svelte/reactivity';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { requiresAtomicBatching } from '$lib/utils/placeholder-detection';
import { shouldLogDatabaseErrors, isTestEnvironment } from '$lib/utils/test-environment';
import { backendAdapter } from './backend-adapter';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { isVersionConflict, isSubtreeAccessDenied } from '$lib/types/errors';
import { showSubtreeAccessDenied } from './subtree-access-denied.svelte';
import { isValidDateId } from '$lib/types/date-node';
import { createLogger } from '$lib/utils/logger';
import { getPendingMoveOperation } from './pending-operations';
import { onDaemonReconnect } from './daemon-status';
import { focusManager } from './focus-manager.svelte';
import type { Node } from '$lib/types';
import type { NodeReference } from '$lib/types/node';
import type { TaskNode } from '$lib/types/task-node';
import type { InsertPosition } from '$lib/services/backend-adapter';
import type {
  NodeUpdate,
  UpdateSource,
  NodeChangeCallback,
  Unsubscribe,
  StoreMetrics,
  UpdateOptions
} from '$lib/types/update-protocol';
import {
  conflictNotifications,
  type ConflictNotification
} from '$lib/stores/conflict-notifications.svelte';
import { normalizeNodeData, deepMergeProperties, promoteTypedFields } from './node-normalize';
import { decideRemoteUpdate, shouldSkipStaleAiChatUpdate } from './remote-update-policy';

const CONFLICT_MESSAGE: Record<ConflictNotification['conflictType'], string> = {
  'version-mismatch': 'Your edit conflicted with a remote change',
  'deleted-node': 'The node you edited was deleted by another pane',
  'child-transfer-failure': "Changes couldn't be saved. Please try again.",
  'write-failure': "Your change couldn't be saved. Please check your connection.",
  // Surfaced by the app-shell startup check (ADR-068 conflict journal) with
  // its own count-aware message; this entry only keeps the map exhaustive
  // over the union.
  'conflict-journal': 'An unresolved conflict is waiting for review'
};

const log = createLogger('SharedNodeStore');

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

const coordLog = createLogger('PersistenceCoordinator');

/** Pending operation to run after current execution completes */
interface QueuedOperation {
  operation: () => Promise<void>;
  options: { mode: 'immediate' | 'debounce'; dependencies?: Array<string | (() => Promise<void>)> };
  resolve: () => void;
  reject: (error: Error) => void;
  /** The queued write's own completion promise, settled by resolve/reject above. */
  promise: Promise<void>;
}

// Exported so tests can exercise the coordinator's supersede/settlement
// behavior directly, without going through SharedNodeStore's full update
// pipeline (backend RPC mocking, node-store bookkeeping, etc.).
export class SimplePersistenceCoordinator {
  private static instance: SimplePersistenceCoordinator | null = null;
  private pendingOperations = new Map<string, PendingOperation>();
  private executingOperations = new Set<string>(); // Track in-flight operations
  // Track nodes that need re-persistence after current operation completes
  // Stores the QUEUED operation so DELETE isn't overwritten by UPDATE re-run
  private queuedOperations = new Map<string, QueuedOperation>();
  private readonly DEBOUNCE_MS = 500;
  private operationCounter = 0; // For tracking operation IDs

  // Private: enforces the singleton. Exporting the class (for direct testing)
  // would otherwise let external code `new` a second, uncoordinated instance
  // whose maps are disjoint from the real one — defeating the per-node
  // serial-writer guarantee. Always go through getInstance()/resetInstance().
  private constructor() {}

  static getInstance(): SimplePersistenceCoordinator {
    if (!SimplePersistenceCoordinator.instance) {
      SimplePersistenceCoordinator.instance = new SimplePersistenceCoordinator();
    }
    return SimplePersistenceCoordinator.instance;
  }

  /**
   * Audited for the same defect class `cancelPending()` and the queued-
   * operation overwrite had before they were fixed to settle a superseded
   * write's promise before dropping it (see `OperationCancelledError`
   * usage above): this drops the whole singleton — including any
   * outstanding `pendingOperations`/`queuedOperations` entries and their
   * live `setTimeout` timers — without settling their promises at all.
   * Left as-is rather than applying the same fix here: every call site of
   * `resetInstance()` (this one and `SharedNodeStore.resetInstance()`,
   * which delegates to it) is test setup/teardown only — no production
   * code path calls it. A caller awaiting a `persist()` handle across a
   * `resetInstance()` call only happens today in a test that's actively
   * tearing down its own fixture, where an unsettled promise is
   * consequence-free (the test process/module state resets right after).
   * If a real (non-test) call path is ever added, apply the same
   * settle-before-drop fix used elsewhere in this class first.
   */
  static resetInstance(): void {
    SimplePersistenceCoordinator.instance = null;
  }

  persist(
    nodeId: string,
    operation: () => Promise<void>,
    options: {
      mode: 'immediate' | 'debounce';
      dependencies?: Array<string | (() => Promise<void>)>;
    } = { mode: 'debounce' }
  ): { promise: Promise<void> } {
    const opId = ++this.operationCounter;
    const shortNodeId = nodeId.substring(0, 8);

    // Check if an operation is currently executing for this node
    const isExecuting = this.executingOperations.has(nodeId);
    const hasPending = this.pendingOperations.has(nodeId);

    coordLog.debug(
      `[op#${opId}] persist() called for ${shortNodeId}: mode=${options.mode}, ` +
        `hasPending=${hasPending}, isExecuting=${isExecuting}`
    );

    // If an operation is already executing for this node, collapse the new
    // operation into a single latest-wins pending write. It runs immediately
    // after the in-flight write's version confirmation lands (see the
    // `finally` block below) — never re-debounced, never re-fired per RPC
    // round-trip. This makes a conflict-with-self structurally impossible:
    // the queued closure always re-reads the version only after the prior
    // write's `localNode.version` write-back has happened.
    if (isExecuting) {
      // Queue the new operation - it supersedes any previously queued operation
      // (single-slot Map so DELETE isn't lost behind a re-run UPDATE, and a
      // burst of keystrokes collapses to the single latest edit).
      //
      // CRITICAL: Reject any previously-queued entry's promise before it's
      // overwritten below. Without this, a burst of keystrokes during an
      // in-flight write leaves one never-settled promise per superseded
      // queued edit — mirrors clearQueued's settlement rule.
      const previouslyQueued = this.queuedOperations.get(nodeId);
      if (previouslyQueued) {
        previouslyQueued.reject(new OperationCancelledError('Superseded by a newer write'));
      }
      let queuedResolve: () => void = () => {};
      let queuedReject: (error: Error) => void = () => {};
      const queuedPromise = new Promise<void>((res, rej) => {
        queuedResolve = res;
        queuedReject = rej;
      });
      this.queuedOperations.set(nodeId, {
        operation,
        options,
        resolve: queuedResolve,
        reject: queuedReject,
        promise: queuedPromise
      });
      // Also register a pendingOperations placeholder so flush/wait helpers
      // (which only look at pendingOperations) see this node as outstanding
      // even though its write is collapsed behind an in-flight RPC rather
      // than sitting on a debounce timer. `operation()` here does NOT force
      // early execution — it just resolves once the coordinator's own
      // finally-chain runs the queued write after confirmation lands.
      // resolve/reject below are dead no-ops: real settlement flows through
      // `promise` (== queuedPromise), settled via queued.resolve/reject.
      this.pendingOperations.set(nodeId, {
        nodeId,
        operation: () => queuedPromise,
        resolve: () => {},
        reject: () => {},
        promise: queuedPromise,
        timeoutId: setTimeout(() => {}, 0)
      });
      coordLog.debug(
        `[op#${opId}] operation already executing for ${shortNodeId}, collapsed into latest-wins pending write (mode=${options.mode})`
      );
      return { promise: queuedPromise };
    }

    // Cancel existing pending operation for this node (only if not executing)
    this.cancelPending(nodeId, opId);

    let resolve: () => void = () => {};
    let reject: (error: Error) => void = () => {};
    const promise = new Promise<void>((res, rej) => {
      resolve = res;
      reject = rej;
    });

    const runOperation = async (
      op: () => Promise<void>,
      deps: Array<string | (() => Promise<void>)> | undefined,
      onDone: () => void,
      onError: (error: Error) => void
    ) => {
      // Mark as executing
      this.executingOperations.add(nodeId);
      coordLog.debug(`[op#${opId}] executeOperation() starting for ${shortNodeId}`);

      try {
        // Wait for dependencies if any
        if (deps) {
          for (const dep of deps) {
            try {
              if (typeof dep === 'function') {
                await dep();
              } else {
                // Wait for dependent node to finish
                const pending = this.pendingOperations.get(dep);
                if (pending) {
                  await pending.promise;
                }
              }
            } catch (depError) {
              // The dependency (another node's persist operation, or this
              // lambda dependency itself) failed or was cancelled — this
              // operation's own `op()` below never even got a chance to
              // run. Wrap it so every `.catch` site downstream can tell
              // that apart from being personally cancelled — see
              // `DependencyFailedError`'s doc comment for why that
              // distinction matters (a raw propagated
              // `OperationCancelledError` here would otherwise be
              // misclassified by every `.catch(err => err instanceof
              // OperationCancelledError ...)` site as "expected, ignore
              // it," silently eating this write with nothing left to
              // retry it).
              const cause = depError instanceof Error ? depError : new Error(String(depError));
              throw new DependencyFailedError(
                typeof dep === 'function' ? '<function dependency>' : dep,
                cause
              );
            }
          }
        }

        await op();
        coordLog.debug(`[op#${opId}] executeOperation() completed for ${shortNodeId}`);
        onDone();
      } catch (error) {
        const err = error instanceof Error ? error : new Error(String(error));
        coordLog.debug(`[op#${opId}] executeOperation() failed for ${shortNodeId}: ${err}`);
        onError(err);
      } finally {
        this.pendingOperations.delete(nodeId);

        // Check for a queued operation BEFORE clearing executingOperations so
        // that hasPending() returns true with no gap. A WatchNodes setNode
        // arriving between "execution done" and "queued op taking over" would
        // otherwise see hasPending=false and clobber the optimistic store.
        const queued = this.queuedOperations.get(nodeId);
        if (queued) {
          this.queuedOperations.delete(nodeId);
          // Re-register in pendingOperations immediately so hasPending() stays
          // true until the queued write actually starts below. `promise` is
          // the queued write's OWN completion promise (already registered at
          // queue time) so waitForPersistence()/flush callers awaiting
          // `pending.promise` block on the actual queued write, not resolve
          // prematurely.
          // resolve/reject below are dead no-ops: real settlement flows
          // through `promise` (== queued.promise), settled via runOperation's
          // onDone/onError, which are queued.resolve/reject.
          this.pendingOperations.set(nodeId, {
            nodeId,
            operation: () =>
              runOperation(
                queued.operation,
                queued.options.dependencies,
                queued.resolve,
                queued.reject
              ),
            resolve: () => {},
            reject: () => {},
            promise: queued.promise,
            timeoutId: setTimeout(() => {}, 0)
          });
          coordLog.debug(
            `[op#${opId}] queued operation taking over for ${shortNodeId} (mode=${queued.options.mode})`
          );
        }

        this.executingOperations.delete(nodeId);

        if (queued) {
          // Run the queued write now that this write's version confirmation
          // has landed — deferred via microtask (not setTimeout/debounce) to
          // avoid unbounded stack growth while still running as soon as
          // possible after confirmation, never re-debounced.
          void Promise.resolve().then(() => {
            void runOperation(
              queued.operation,
              queued.options.dependencies,
              queued.resolve,
              queued.reject
            );
          });
        }
      }
    };

    const executeOperation = () => runOperation(operation, options.dependencies, resolve, reject);

    if (options.mode === 'immediate') {
      coordLog.debug(`[op#${opId}] scheduling IMMEDIATE for ${shortNodeId}`);
      const pending: PendingOperation = {
        nodeId,
        // Store the wrapper that tracks executingOperations, not the raw operation
        // This ensures flushAndWaitForNodes() properly tracks execution state
        operation: executeOperation,
        timeoutId: setTimeout(() => {}, 0),
        promise,
        resolve,
        reject
      };
      this.pendingOperations.set(nodeId, pending);
      executeOperation();
    } else {
      coordLog.debug(
        `[op#${opId}] scheduling DEBOUNCED (${this.DEBOUNCE_MS}ms) for ${shortNodeId}`
      );
      const timeoutId = setTimeout(executeOperation, this.DEBOUNCE_MS);
      const pending: PendingOperation = {
        nodeId,
        // Store the wrapper that tracks executingOperations, not the raw operation
        // This ensures flushAndWaitForNodes() properly tracks execution state
        operation: executeOperation,
        timeoutId,
        promise,
        resolve,
        reject
      };
      this.pendingOperations.set(nodeId, pending);
    }

    return { promise };
  }

  cancelPending(nodeId: string, opId?: number): void {
    const pending = this.pendingOperations.get(nodeId);
    if (pending) {
      const shortNodeId = nodeId.substring(0, 8);
      const isExecuting = this.executingOperations.has(nodeId);
      coordLog.debug(
        `[op#${opId ?? '?'}] cancelPending() for ${shortNodeId}: ` +
          `isExecuting=${isExecuting} (cancel ${isExecuting ? 'INEFFECTIVE' : 'effective'})`
      );
      clearTimeout(pending.timeoutId);
      if (!isExecuting) {
        // Settle the superseded write's promise before dropping the entry —
        // mirrors clearQueued's rule. Without this, a debounced write
        // cancelled by a newer one (e.g. via a fresh persist() call or
        // startBatch()) leaves pending.promise unsettled forever.
        //
        // Only when NOT executing: while a write is in flight, this entry's
        // resolve/reject either belong to that real RPC (settling here would
        // report a false "cancelled" to any awaiter, then get silently
        // no-op'd when the real outcome lands — masking a genuine failure)
        // or are the dead no-op placeholder for a write collapsed behind it
        // (its real promise lives in queuedOperations, untouched here, and
        // still runs and settles normally once the in-flight write's finally
        // block dispatches it). Either way, settling is a no-op or a lie —
        // leave it to the real completion path.
        pending.reject(new OperationCancelledError('Superseded by a newer write'));
      }
      this.pendingOperations.delete(nodeId);
    }
  }

  /**
   * Clear any queued operation for a node (e.g., after OCC conflict to prevent stale retries)
   *
   * CRITICAL: Must settle the queued op's promise and drop its pendingOperations
   * placeholder here. Both were registered at queue time (see `persist()`), and
   * without this, discarding the queue entry mid-flight leaves that promise
   * unsettled forever and hasPending(nodeId) stuck `true` — silently blocking
   * database broadcasts for this node and hanging any flush/wait call on it.
   */
  clearQueued(nodeId: string): void {
    const queued = this.queuedOperations.get(nodeId);
    if (queued) {
      coordLog.debug(`Cleared queued operation for ${nodeId.substring(0, 8)} (OCC conflict)`);
      this.queuedOperations.delete(nodeId);
      queued.reject(
        new OperationCancelledError('Queued write cancelled: prior write hit an OCC conflict')
      );
      const pending = this.pendingOperations.get(nodeId);
      if (pending && pending.promise === queued.promise) {
        this.pendingOperations.delete(nodeId);
      }
    }
  }

  isPending(nodeId: string): boolean {
    return this.pendingOperations.has(nodeId);
  }

  isExecuting(nodeId: string): boolean {
    return this.executingOperations.has(nodeId);
  }

  /**
   * True only when a genuinely different write is collapsed behind this
   * node's currently-executing one (see `persist()`'s `isExecuting` branch).
   * Unlike `isPending`/`isExecuting`/`hasPending`, this is never true purely
   * because of an operation's OWN bookkeeping — a write is never routed
   * through the `isExecuting` queueing branch for itself, only a second,
   * later `persist()` call for the same node arriving while it runs. Callers
   * that need to distinguish "something else is genuinely queued" from "I'm
   * reading my own not-yet-cleared state" (e.g. an OCC-conflict handler
   * deciding whether a fallback resync is safe to apply) must use this
   * instead of `hasPending()`/`isPending()`/`isExecuting()`, and must read it
   * BEFORE calling `clearQueued()`, which erases the evidence this checks.
   */
  isQueued(nodeId: string): boolean {
    return this.queuedOperations.has(nodeId);
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
      // Only START the operation if it is not already in flight. Without this
      // guard, a debounced save whose timeout fired just before window-close
      // (so it is mid-RPC, tracked in executingOperations) would be executed a
      // SECOND time here — an OCC conflict or duplicate create at the most
      // data-loss-sensitive moment. Mirror flushAndWaitForNodes: skip the
      // re-execute, but still await the in-flight promise either way.
      if (!this.executingOperations.has(nodeId)) {
        pending
          .operation()
          .then(
            () => pending.resolve(),
            (error) => pending.reject(error instanceof Error ? error : new Error(String(error)))
          )
          .finally(() => {
            this.pendingOperations.delete(nodeId);
          });
      }
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
            new Promise<void>((_, reject) =>
              setTimeout(() => reject(new Error('Timeout')), timeoutMs)
            )
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
        // Clear the debounce timeout to prevent it from firing
        clearTimeout(pending.timeoutId);

        // Only start the operation if it's not already executing
        // This prevents double-execution when the timeout fires just before clearTimeout
        if (!this.executingOperations.has(nodeId)) {
          // Start the operation now
          pending
            .operation()
            .then(
              () => pending.resolve(),
              (error) => pending.reject(error instanceof Error ? error : new Error(String(error)))
            )
            .finally(() => {
              this.pendingOperations.delete(nodeId);
            });
        }

        // Wait for completion with timeout (whether we started it or it was already running)
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
   * Returns true when a persistence operation for this node is either
   * pending (debounced and not yet fired), executing (in-flight RPC), or
   * queued behind an executing one. Used by `SharedNodeStore.setNode()` to
   * recognise "the user has unsaved local changes for this node" and skip
   * a daemon-broadcast apply that would otherwise clobber them.
   */
  hasPending(nodeId: string): boolean {
    return (
      this.pendingOperations.has(nodeId) ||
      this.executingOperations.has(nodeId) ||
      this.queuedOperations.has(nodeId)
    );
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
    // Include nodes that are mid-RPC (executingOperations) or that have a
    // collapsed latest-wins write waiting behind one (queuedOperations), not
    // just nodes with a debounce timer still pending. Otherwise a node whose
    // write is in flight — with a serial-writer follow-up queued behind it —
    // is invisible to this snapshot and flushAll() returns before either
    // write actually lands.
    const allNodeIds = new Set<string>([
      ...this.pendingOperations.keys(),
      ...this.executingOperations,
      ...this.queuedOperations.keys()
    ]);
    if (allNodeIds.size === 0) {
      return new Set();
    }
    return this.flushAndWaitForNodes(Array.from(allNodeIds), timeoutMs);
  }
}

// All production call sites in this file go through this alias rather than
// the class name directly — kept as-is (not worth a ~20-call-site rename) now
// that SimplePersistenceCoordinator is separately exported for direct testing.
const PersistenceCoordinator = SimplePersistenceCoordinator;

// Simple error class for cancelled operations
export class OperationCancelledError extends Error {
  constructor(message = 'Operation cancelled') {
    super(message);
    this.name = 'OperationCancelledError';
  }
}

/**
 * Thrown by `runOperation`'s dependency-wait loop (see `persist()`'s
 * `dependencies` option) when a dependency — another node's persist
 * operation, or an arbitrary lambda dependency — fails or is cancelled
 * before this operation's own `op()` ever runs.
 *
 * Deliberately NOT a subclass of `OperationCancelledError`, even though the
 * most common cause is a dependency being cancelled (whose promise rejects
 * with exactly that type): every `persist()` caller's `.catch(err => ...)`
 * treats `err instanceof OperationCancelledError` as "I was personally
 * cancelled — a newer write for the SAME node supersedes me, ignore this."
 * That's true when `cancelPending()`/`clearQueued()`/the queue-overwrite
 * path rejects THIS operation's own promise directly. It is NOT true here:
 * this operation was never itself cancelled, and nothing else is going to
 * retry its write — a DIFFERENT node it depended on didn't complete, so its
 * own backend RPC never even had a chance to fire. Without a distinct type,
 * a propagated dependency cancellation would be misclassified by every one
 * of those `.catch` sites as "expected, ignore it" and silently swallowed.
 *
 * `cause` preserves the original error (which may itself be an
 * `OperationCancelledError`, an OCC/version-conflict error, or any other
 * write failure) for callers that want to inspect it; `.catch` sites that
 * don't care can just treat any `DependencyFailedError` as a generic
 * failure to surface, the same way they already treat any other non-
 * cancellation error.
 */
export class DependencyFailedError extends Error {
  constructor(
    public readonly dependencyId: string,
    public readonly cause: Error
  ) {
    super(`Dependency "${dependencyId}" failed or was cancelled: ${cause.message}`);
    this.name = 'DependencyFailedError';
  }
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
 * The type-specific (plus content) fields `updateTaskNode()` can write.
 * Used to key the per-field write-sequence tracking that guards against
 * same-field concurrent writes transiently clobbering each other — see
 * `bumpTaskFieldSeq()`'s doc comment.
 */
type TaskFieldName =
  | 'status'
  | 'priority'
  | 'dueDate'
  | 'startedAt'
  | 'completedAt'
  | 'content';

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

  // Core node storage. SvelteMap tracks reads/writes at per-key granularity, so a mutation to
  // one node only invalidates $derived/$effect consumers that read that specific node.
  nodes = new SvelteMap<string, Node>();

  // Track which nodes have been persisted to database
  // Avoids querying database on every update to check existence
  private persistedNodeIds = new Set<string>();

  // NOTE: childrenCache and parentsCache REMOVED
  // Hierarchy is now managed by ReactiveStructureTree (domain events)
  // Use structureTree.getChildren() and structureTree.getParent() instead

  // Subscriptions for change notifications
  private subscriptions = new Map<string, Set<Subscription>>();
  private wildcardSubscriptions = new Set<Subscription>();
  private subscriptionIdCounter = 0;

  // Batch ID counter for unique batch identification
  private batchIdCounter = 0;

  // Pending operations (optimistic updates)
  private pendingUpdates = new Map<string, NodeUpdate[]>();

  // Performance metrics
  private metrics: StoreMetrics = {
    updateCount: 0,
    avgUpdateTime: 0,
    maxUpdateTime: 0,
    subscriptionCount: 0,
    rollbackCount: 0
  };

  // Version tracking for optimistic concurrency
  private versions = new Map<string, number>();

  // Per-node, per-field write sequence counters for `updateTaskNode()`. See
  // `bumpTaskFieldSeq()`'s doc comment for what this closes.
  private taskFieldWriteSeq = new Map<string, Map<TaskFieldName, number>>();

  /**
   * Bump and return the write-sequence number for a single task field on a
   * node. `updateTaskNode()` calls this once per field its own `update`
   * touches, at optimistic-apply time (synchronously, before the RPC is
   * even queued) and captures the returned number.
   *
   * When that write's RPC later resolves, it re-reads the CURRENT sequence
   * for the same field via `getTaskFieldSeq()` before applying the response's
   * confirmed value for that field. If a second `updateTaskNode()` call for
   * the SAME field raced in while the first write's RPC was still in
   * flight, it bumped the sequence again — so the first write's captured
   * number is now stale, and it must skip applying its (now-outdated)
   * confirmed value for that one field rather than clobber the newer
   * write's optimistic value. The newer write's own RPC (queued behind the
   * first — `PersistenceCoordinator` serializes real RPCs per node) applies
   * its own confirmed value when it resolves, so the store still converges;
   * this only closes the transient window where the wrong value would
   * otherwise be visible in between.
   *
   * Field-scoped (not node-scoped) so a same-field race on `status` doesn't
   * suppress an unrelated, non-racing field like `priority` from applying
   * its own confirmed value.
   */
  private bumpTaskFieldSeq(nodeId: string, field: TaskFieldName): number {
    let fields = this.taskFieldWriteSeq.get(nodeId);
    if (!fields) {
      fields = new Map();
      this.taskFieldWriteSeq.set(nodeId, fields);
    }
    const next = (fields.get(field) ?? 0) + 1;
    fields.set(field, next);
    return next;
  }

  /** Current write-sequence number for a task field. See `bumpTaskFieldSeq()`. */
  private getTaskFieldSeq(nodeId: string, field: TaskFieldName): number {
    return this.taskFieldWriteSeq.get(nodeId)?.get(field) ?? 0;
  }

  // ------------------------------------------------------------------------
  // Reconnect staleness tracking
  //
  // The desktop app's WatchNodes bridge (`watcher.rs`) reconnects with backoff
  // on any daemon disruption (crash, restart, transient h2 error) but opens a
  // fresh live-forward stream with no catch-up replay — any node:created /
  // node:updated / node:deleted events that would have landed during the
  // outage are gone, not merely delayed. A node already cached before the
  // outage is never told it might be missing an update, so it renders
  // whatever it last held (in the worst case, a fresh AI chat conversation
  // cached before its messages were appended) until some unrelated write
  // happens to touch it again.
  //
  // `reconnectGeneration` is bumped once per `onDaemonReconnect` firing (i.e.
  // once per observed outage window). `nodeGeneration` records, per node, the
  // generation it was last written under. A node whose recorded generation is
  // behind the current one may have missed updates and is treated as
  // possibly-stale until the next write refreshes it — see `isPossiblyStale`.
  // ------------------------------------------------------------------------
  private reconnectGeneration = 0;
  private nodeGeneration = new Map<string, number>();

  // ------------------------------------------------------------------------
  // Reachability tracking & eviction (multi-tab memory)
  //
  // SharedNodeStore is a single global cache shared by every open tab/pane;
  // nothing previously removed a node once loaded, so a long session that
  // visits many documents accumulates all of them in memory forever. These
  // members let `navigation.svelte.ts` push the current set of open tabs'
  // document roots on every tab-state change (open, close, content change,
  // session restore) so a cached node that no open tab/pane can reach any
  // longer becomes eligible for eviction after a short inactivity window —
  // not immediately, to avoid thrashing on quick tab switches or an
  // accidental close-and-reopen.
  //
  // Reachability is computed, not separately bookkept per node: a node is
  // reachable when walking its `structureTree` ancestor chain reaches a node
  // id present in `openDocumentRootIds`. This reuses the parent index every
  // viewer's data-loading path already populates (loadChildrenForParent /
  // loadChildrenTree) instead of requiring a second, parallel per-node
  // membership map that could drift from it.
  // ------------------------------------------------------------------------

  /** Root node ids of every currently open tab/pane, as last reported by
   * `navigation.svelte.ts`. See `isReachable` and `hasOpenDocumentReport`. */
  private openDocumentRootIds = new Set<string>();

  /**
   * True once `updateOpenDocumentRoots` has been called at least once.
   * Distinguishes "no report yet" (eviction dormant — see `isReachable`)
   * from "reported, and the open set happens to be empty" (e.g. every tab
   * just closed) — both leave `openDocumentRootIds` empty, but only the
   * latter is real information that nothing is reachable.
   */
  private hasOpenDocumentReport = false;

  /** nodeId -> scheduled eviction timer, for a node currently unreachable
   * from every open tab/pane and waiting out `evictionInactivityMs`. */
  private evictionTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /**
   * How long a node must stay unreachable from every open tab/pane before
   * it is evicted. Long enough that ordinary quick tab-switching or an
   * accidental close-and-reopen never trips it, short enough to reclaim
   * memory well within a single working session. Overridable in tests via
   * `__setEvictionInactivityMsForTesting` so eviction tests don't wait 30
   * real seconds.
   */
  private evictionInactivityMs = 30_000;

  /**
   * Replace the set of open-tab/pane document roots and re-evaluate every
   * cached node's reachability against it. Called by `navigation.svelte.ts`
   * with the FULL current list (not a delta) on every tab-state mutation, so
   * a node whose last open tab just closed drops out of reachability here,
   * and a node reopened before its eviction timer fired has that timer
   * cancelled by the same sweep. No-ops when the set is unchanged (the
   * common case — most nav interactions, e.g. resizing a pane or switching
   * the active tab, don't change which documents are open) to avoid sweeping
   * every cached node on every such interaction.
   */
  updateOpenDocumentRoots(rootNodeIds: Iterable<string>): void {
    const next = new Set(rootNodeIds);
    const isFirstReport = !this.hasOpenDocumentReport;
    this.hasOpenDocumentReport = true;

    if (!isFirstReport && next.size === this.openDocumentRootIds.size) {
      let unchanged = true;
      for (const id of next) {
        if (!this.openDocumentRootIds.has(id)) {
          unchanged = false;
          break;
        }
      }
      if (unchanged) return;
    }
    this.openDocumentRootIds = next;
    this.reconcileEvictionCandidates();
  }

  /**
   * Sweep every currently-cached node: schedule eviction for one that just
   * became unreachable (unless already scheduled), and cancel any pending
   * eviction for one that just became reachable again.
   */
  private reconcileEvictionCandidates(): void {
    for (const nodeId of this.nodes.keys()) {
      if (this.isReachable(nodeId)) {
        this.cancelPendingEviction(nodeId);
      } else if (!this.evictionTimers.has(nodeId)) {
        this.scheduleEviction(nodeId);
      }
    }
  }

  /**
   * True when `nodeId` — or an ancestor reached by walking `structureTree`
   * parent edges — is the root document of a currently open tab/pane.
   *
   * Before the first `updateOpenDocumentRoots` report (i.e. every caller
   * that isn't `navigation.svelte.ts` — most unit tests included), the
   * reachable-roots set is empty and this always returns true: eviction
   * stays fully dormant until navigation actually starts reporting real tab
   * state, rather than evicting nodes no caller ever declared "open".
   */
  private isReachable(nodeId: string): boolean {
    if (!this.hasOpenDocumentReport) return true;
    if (!structureTree) return true;

    let current: string | null = nodeId;
    const visited = new Set<string>();
    while (current !== null) {
      if (this.openDocumentRootIds.has(current)) return true;
      if (visited.has(current)) return false; // defensive cycle guard
      visited.add(current);
      current = structureTree.getParent(current);
    }
    return false;
  }

  private scheduleEviction(nodeId: string): void {
    const timer = setTimeout(() => this.attemptEviction(nodeId), this.evictionInactivityMs);
    this.evictionTimers.set(nodeId, timer);
  }

  private cancelPendingEviction(nodeId: string): void {
    const timer = this.evictionTimers.get(nodeId);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.evictionTimers.delete(nodeId);
    }
  }

  /**
   * Fires after a node has sat unreachable for `evictionInactivityMs`.
   * Re-verifies both eviction constraints before actually dropping it, since
   * either can have changed since the timer was scheduled:
   *
   * - Reachability: `updateOpenDocumentRoots` cancels this timer as soon as
   *   a report makes the node reachable again, but re-checking here is a
   *   cheap final guard against ordering surprises.
   * - Pending write: never evict a node with an unflushed, in-flight, or
   *   queued persistence operation (PersistenceCoordinator.hasPending), or
   *   an uncommitted atomic batch (activeBatches) — losing an unsaved edit
   *   is strictly worse than a delayed eviction. Re-schedule rather than
   *   drop candidacy so a long-running write doesn't permanently pin the
   *   node in memory once it finally settles.
   */
  private attemptEviction(nodeId: string): void {
    this.evictionTimers.delete(nodeId);

    if (this.isReachable(nodeId)) return; // reopened — nothing to do

    if (PersistenceCoordinator.getInstance().hasPending(nodeId) || this.activeBatches.has(nodeId)) {
      this.scheduleEviction(nodeId);
      return;
    }

    this.evictNode(nodeId);
  }

  /**
   * Drop every per-node bookkeeping entry for an unreachable, fully-settled
   * node — from `nodes` itself plus every other per-node map this file
   * maintains (mirrors the set `deleteNode()`/`clearAll()` already clean
   * up). Purely a local cache decision: the node still exists in the
   * database, just no longer cached in memory — the next `ensureNode` for
   * it fetches fresh.
   */
  private evictNode(nodeId: string): void {
    this.nodesDelete(nodeId);
    this.versions.delete(nodeId);
    this.pendingUpdates.delete(nodeId);
    this.persistedNodeIds.delete(nodeId);
    this.taskFieldWriteSeq.delete(nodeId);
    this.resyncingNodes.delete(nodeId);
    this.resyncQueued.delete(nodeId);
    this.inFlightEnsures.delete(nodeId);
    log.debug(`Evicted unreachable node from cache: ${nodeId}`);
  }

  /** Test-only: override the inactivity threshold so eviction tests don't
   * need to wait out the real 30-second production default. */
  __setEvictionInactivityMsForTesting(ms: number): void {
    this.evictionInactivityMs = ms;
  }

  /** Test-only: number of nodes currently unreachable and waiting out their
   * inactivity window before eviction. */
  __getPendingEvictionCountForTesting(): number {
    return this.evictionTimers.size;
  }

  /**
   * Decide whether the persistence path should clear a CREATE's
   * `InsertPosition.After` hint as "stale" before talking to the backend.
   *
   * `structureTree` is the authoritative source for parent-child relationships.
   * Parent is derived from `structureTree.getParent(nodeId)` at CREATE time.
   *
   * Returns `true` only when `structureTree` reports a parent for the
   * sibling AND that parent disagrees with the new node's
   * `currentParentId`. If `structureTree` has no opinion (`null`), the
   * hint is preserved and the backend's own retry loop handles it —
   * silently clearing a valid hint here is what produced
   * the drop-to-the-top behavior.
   *
   * If `structureTree.getParent` returns `null` we emit a debug log so
   * the frequency of "tree not yet populated at persistence time" is
   * observable; a high rate suggests the persistence call is racing the
   * structureTree population (different code path bug, not this one).
   */
  shouldClearStaleInsertAfter(
    siblingId: string,
    currentParentId: string | null | undefined
  ): boolean {
    const siblingActualParent = structureTree.getParent(siblingId);
    if (siblingActualParent === null) {
      log.debug(
        `shouldClearStaleInsertAfter: structureTree has no parent for sibling ${siblingId.substring(0, 8)} — preserving hint, backend will validate`
      );
      return false;
    }
    return siblingActualParent !== (currentParentId ?? null);
  }

  /**
   * Returns the version the next UpdateNode RPC would send for this node.
   *
   * Before ADR-026's C5 extension, this also consulted a server-confirmed-version cache
   * the skip-while-editing guard populated for a broadcast plausibly
   * classified as this client's own echo. That classification no longer
   * exists (the daemon suppresses echoes before they reach the frontend at
   * all — see `remote-update-policy.ts`), so every database-sourced
   * broadcast to an actively-edited node is now always treated as foreign:
   * this simply reads the local node's own version.
   */
  computeOccVersionForUpdate(nodeId: string): number {
    return this.nodes.get(nodeId)?.version ?? 1;
  }

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

  // Track nodes currently being resynced to prevent concurrent resync operations
  private resyncingNodes = new Set<string>();

  // Nodes with a follow-up resync requested while one was already in flight
  // for them — see resyncNodeFromServer()'s idempotency guard.
  private resyncQueued = new Set<string>();

  /**
   * Monotonic database generation. ADR-053 ("One Daemon, Multiple Local
   * Databases") lets the desktop hot-swap the active database; `clearAll()`
   * (invoked by the switch) bumps this counter so any read dispatched against
   * the previous database — whose promise resolves *after* the swap — is
   * detectable as stale and dropped, instead of populating the now-active store
   * with the previous database's rows (orphans unreferenced by the new tree
   * that would otherwise surface via global search / mention resolution until
   * the next reload). Fetch-then-write paths capture `currentEpoch()` before
   * awaiting the daemon and re-check it before applying the result.
   */
  private databaseEpoch = 0;

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
    PersistenceCoordinator.resetInstance();
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

  private nodesSet(nodeId: string, node: Node): void {
    // `SvelteMap.set()` only increments a key's reactive signal when the new
    // value is a DIFFERENT reference than what's already stored (reference
    // equality, not a deep comparison) — see svelte/src/reactivity/map.js.
    // Several callers in this file mutate the CURRENT node object in place
    // (e.g. `Object.assign(localNode, ...)`) and then re-set that same
    // reference here, intending to "confirm" a value once a write's RPC
    // resolves. Passed straight through, that re-set is a silent no-op for
    // reactivity: every `$derived`/`$effect` consumer already holding that
    // reference (from an earlier read) never re-runs, so a confirmed value
    // that differs from what an in-between optimistic write showed never
    // reaches the UI — exactly the "stale until remount" Kanban symptom this
    // fixes. Always store a shallow-copied object so `set()` sees a genuine
    // reference change and signals every consumer, regardless of whether the
    // caller mutated in place or built a fresh object.
    this.nodes.set(nodeId, { ...node });
    // Any write — database broadcast, optimistic local edit, or a staleness
    // refetch resolving — is fresh-as-of-now evidence for this node, so it no
    // longer needs re-confirming against the current reconnect generation.
    this.nodeGeneration.set(nodeId, this.reconnectGeneration);
  }

  private nodesDelete(nodeId: string): void {
    this.nodes.delete(nodeId);
    this.nodeGeneration.delete(nodeId);
  }

  private nodesClear(): void {
    this.nodes.clear();
    this.nodeGeneration.clear();
  }

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
    return childIds.map((id) => this.nodes.get(id)).filter((n): n is Node => n !== undefined);
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
   * Get parent ID for a node, delegating to structureTree as the single source of truth.
   */
  getParentId(nodeId: string): string | null {
    if (!structureTree) return null;
    return structureTree.getParent(nodeId);
  }

  /**
   * Check if a node exists
   */
  hasNode(nodeId: string): boolean {
    return this.nodes.has(nodeId);
  }

  /**
   * True if `nodeId` is cached but was last written before the most recent
   * daemon reconnect — i.e. it may be missing a `WatchNodes` update the
   * outage window dropped (see `markPossiblyStaleAfterReconnect`). Callers
   * that hydrate a node for display (`ensureNode`, `pane-content.svelte`)
   * use this to re-confirm the cache against the backend instead of trusting
   * a cache entry that predates the gap. Returns `false` for an uncached
   * node — "stale" only describes data we are already holding.
   */
  isPossiblyStale(nodeId: string): boolean {
    if (!this.nodes.has(nodeId)) return false;
    return (this.nodeGeneration.get(nodeId) ?? 0) < this.reconnectGeneration;
  }

  /**
   * Called once per observed daemon reconnect (see the module-level
   * `onDaemonReconnect` wiring below). Does not evict or touch any cached
   * node — an already-open viewer keeps rendering its last-known content
   * with no flicker — it only advances the generation counter so
   * `isPossiblyStale` starts reporting `true` for every node cached before
   * this point, until each is individually re-confirmed.
   */
  markPossiblyStaleAfterReconnect(): void {
    this.reconnectGeneration++;
  }

  /**
   * Get node count
   */
  getNodeCount(): number {
    return this.nodes.size;
  }

  /**
   * Cache-first node fetch. Returns the in-memory node if present; otherwise
   * fetches from the backend, stores it, and returns it. Returns undefined if
   * the backend returns null (node does not exist or was deleted).
   *
   * Special case: date nodes are virtual — they are created lazily in the backend
   * when their first child is saved. A brand-new date node that has never been
   * persisted will return null from the backend. Synthesize a minimal in-memory
   * node so pane-content does not mistake it for a deleted node and close the tab
   * (mirrors the same logic in doLoadChildrenTree).
   *
   * Called by pane-content before mounting any viewer so every viewer mounts
   * with the guarantee that sharedNodeStore.getNode(nodeId) is defined.
   */
  /**
   * Load a node into the store on demand, returning it (or a virtual date node,
   * or undefined). Concurrent calls for the same id are de-duplicated: a page with
   * many `[[id]]` references to the same uncached node issues ONE backend fetch,
   * not one per reference. The in-flight promise is tracked by id and cleared when
   * it settles (after which the node is cached, so later calls hit the cache).
   *
   * Cache-first, but not cache-only: a cache entry that predates the most
   * recent daemon reconnect (`isPossiblyStale`) is re-fetched rather than
   * trusted, since a `WatchNodes` outage can silently drop the update that
   * would have kept it current. The stale entry stays visible/returned by
   * `getNode` for any reader until this fetch resolves and overwrites it —
   * no eviction, no flicker.
   */
  async ensureNode(nodeId: string): Promise<Node | undefined> {
    const cached = this.nodes.get(nodeId);
    if (cached && !this.isPossiblyStale(nodeId)) return cached;

    const existing = this.inFlightEnsures.get(nodeId);
    if (existing) return existing;

    const inFlight = this.fetchAndCacheNode(nodeId).finally(() => {
      this.inFlightEnsures.delete(nodeId);
    });
    this.inFlightEnsures.set(nodeId, inFlight);
    return inFlight;
  }

  /** In-flight `ensureNode` fetches, keyed by node id (see `ensureNode`). */
  private inFlightEnsures = new Map<string, Promise<Node | undefined>>();

  private async fetchAndCacheNode(nodeId: string): Promise<Node | undefined> {
    const epoch = this.databaseEpoch;
    const fetched = await backendAdapter.getNode(nodeId);
    // ADR-053: the active database switched while this read was in flight — the
    // fetched row belongs to the previous database, so drop it instead of
    // populating the now-active store.
    if (this.databaseEpoch !== epoch) return undefined;
    if (fetched) {
      this.setNode(fetched, { type: 'database', reason: 'ensure-node' });
      return fetched;
    }

    if (isValidDateId(nodeId)) {
      const now = new Date().toISOString();
      const virtualDateNode: Node = {
        id: nodeId,
        nodeType: 'date',
        content: '',
        version: 0,
        createdAt: now,
        modifiedAt: now,
        properties: {}
      };
      // database source prevents determinePersistenceBehavior from triggering an unwanted write.
      this.setNode(virtualDateNode, { type: 'database', reason: 'virtual-date-node' });
      return virtualDateNode;
    }

    return undefined;
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

    // Handle isComputedField flag - automatically set skipPersistence
    if (options.isComputedField) {
      options = {
        ...options,
        skipPersistence: true
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

    // Whether this call actually performed an update. The `finally` below
    // records timing metrics, but a call that returns early performed no work
    // and must not contribute a sample — see `recordMetric`.
    let didUpdate = false;

    try {
      // Get existing node
      const existingNode = this.nodes.get(nodeId);
      if (!existingNode) {
        log.warn(`Cannot update non-existent node: ${nodeId}`);
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

      // Apply update optimistically.
      // A namespaced `properties` patch is deep-merged (so a partial write
      // doesn't drop sibling keys) and its type-specific fields are promoted to
      // the top level immediately — mirroring what the backend's
      // `node_to_typed_value` does on the round-trip response. Without this,
      // viewers reading top-level fields (e.g. ai-chat's `model`/`messages`)
      // stay stale until the RPC resolves, making the UI appear to hang.
      const mergedProperties = changes.properties
        ? options.replaceProperties
          ? changes.properties
          : deepMergeProperties(existingNode.properties, changes.properties, existingNode.nodeType)
        : existingNode.properties;
      const promotedFields = changes.properties
        ? promoteTypedFields(existingNode.nodeType, changes.properties, mergedProperties)
        : {};
      const updatedNode: Node = {
        ...existingNode,
        ...changes,
        ...(changes.properties ? { properties: mergedProperties } : {}),
        ...promotedFields,
        modifiedAt: new Date().toISOString()
      };

      this.nodesSet(nodeId, updatedNode);
      this.versions.set(nodeId, update.version!);

      // Track pending update for potential rollback
      if (!this.pendingUpdates.has(nodeId)) {
        this.pendingUpdates.set(nodeId, []);
      }
      this.pendingUpdates.get(nodeId)!.push(update);

      // Notify subscribers
      this.notifySubscribers(nodeId, updatedNode, source);

      log.debug(`Node updated: ${nodeId}, type: ${this.determineUpdateType(changes)}`);

      // Update metrics
      this.metrics.updateCount++;
      didUpdate = true;

      // Phase 2.4: Persist to database (unless skipped)
      // IMPORTANT: For viewer-sourced updates:
      // - Structural changes persist immediately
      // - Content changes persist in debounced mode
      // This ensures hierarchy operations work while debouncing rapid typing
      const persistBehavior = this.determinePersistenceBehavior(source, options, changes);
      if (persistBehavior.shouldPersist) {
        // All real nodes (even blank) should be persisted
        // FOREIGN KEY validation is handled by persistence coordinator dependencies
        // Structural changes (sibling ordering) are now handled via backend moveNode()

        // Smart routing via plugin system supports type-specific properties
        // Type-specific updaters route to node-specific methods (updateTaskNode, etc.)
        // The persistence whitelist now includes type-specific property changes
        const isStructuralChange = false; // Structural changes now handled via backend moveNode()
        const isContentChange = 'content' in changes;
        // A VALUE comparison, not mere presence: `updateNodeContent` always
        // bundles the current (unchanged) nodeType alongside content on every
        // keystroke (see reactive-node-service.svelte.ts), so an in-flight
        // slash-command type conversion can't race a content update and get
        // silently reverted. Treating that presence alone as "changing type"
        // forced immediate (non-debounced) persistence on every keystroke,
        // which raced the broadcast against the next keystroke and produced
        // spurious "conflicted with a remote change" toasts + content
        // corruption under fast typing. Only a genuine type change should
        // skip the debounce.
        const isNodeTypeChange =
          'nodeType' in changes && changes.nodeType !== existingNode.nodeType;
        const isPropertyChange = 'properties' in changes;
        // Check for type-specific property changes (status, priority, dueDate, etc.)
        // These are persisted via type-specific updaters registered in the plugin system
        const currentNode = this.nodes.get(nodeId);
        const hasTypeUpdater = currentNode?.nodeType
          ? pluginRegistry.hasNodeUpdater(currentNode.nodeType)
          : false;
        const isTypeSpecificChange =
          hasTypeUpdater &&
          ('status' in changes ||
            'priority' in changes ||
            'dueDate' in changes ||
            'startedAt' in changes ||
            'completedAt' in changes);
        const shouldPersist =
          source.type !== 'viewer' ||
          isStructuralChange ||
          isContentChange ||
          isNodeTypeChange ||
          isPropertyChange ||
          isTypeSpecificChange;

        // Do NOT check isPlaceholder here - that's a UI-only concept
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
          // CRITICAL: For content updates, we must read CURRENT state at execution time,
          // not the stale state captured when persist() was called.
          // This prevents race conditions where rapid typing causes earlier states to overwrite later ones.
          const changedFields = Object.keys(changes);
          // Capture non-content fields (e.g. nodeType) at schedule time.
          // Content is intentionally re-read at execute time to get latest typed value,
          // but nodeType must be captured now — SSE can overwrite the store before execution.
          const capturedNonContentFields: Record<string, unknown> = {};
          for (const field of changedFields) {
            if (field !== 'content') {
              capturedNonContentFields[field] = (changes as unknown as Record<string, unknown>)[
                field
              ];
            }
          }
          // Captured at schedule time alongside the other options above —
          // `options` itself doesn't change, but naming it here keeps it next
          // to the rest of what this closure reads from the outer scope.
          const onPersistError = options.onPersistError;
          const onPersistSuccess = options.onPersistSuccess;

          // Set from inside the closure below when an OCC (version-conflict)
          // error has already raised its own specific `version-mismatch`
          // notification, so the outer `handle.promise.catch()` can skip
          // piling a second, generic `write-failure` one on top. Can't just
          // re-check `isVersionConflict` on the outer catch's `err`: the
          // closure re-throws `error` (`dbError instanceof Error ? dbError :
          // new Error(String(dbError))`), which for a plain-object
          // CommandError (the real shape errors cross the Tauri/gRPC
          // boundary in — see `isVersionConflict`'s own doc comment) is NOT
          // `instanceof Error`, so it gets wrapped in a fresh generic `Error`
          // that has lost the `.code`/`.conflictData` shape entirely by the
          // time it reaches the outer catch. Same fix `deleteNode()` already
          // applies for its analogous `subtreeAccessDeniedAlreadyNotified`.
          let occConflictAlreadyNotified = false;

          const handle = PersistenceCoordinator.getInstance().persist(
            nodeId,
            async () => {
              try {
                // All real nodes (even blank) should be persisted
                // No placeholder checks here - viewer-local placeholder never enters this code path

                // Check if node has been persisted - use in-memory tracking to avoid database query
                const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);

                if (isPersistedToDatabase) {
                  // CRITICAL: Read current node state at execution time, not capture time
                  // This ensures we persist the latest content, not stale content from when persist() was called
                  let currentNode = this.nodes.get(nodeId);
                  if (!currentNode) {
                    log.warn(
                      `Node ${nodeId} no longer exists in store, skipping update persistence`
                    );
                    return;
                  }

                  // CRITICAL: Wait for any pending move operation to complete before UPDATE.
                  // Move operations (indent/outdent) increment the version in the backend.
                  // If we UPDATE before the move completes, we'll have a version mismatch.
                  const pendingMove = getPendingMoveOperation(nodeId);
                  if (pendingMove) {
                    log.debug(
                      `[UPDATE] Waiting for pending move operation on ${nodeId.substring(0, 8)}`
                    );
                    await pendingMove;
                    // Re-read current node to get updated version after move
                    const refreshedNode = this.nodes.get(nodeId);
                    if (refreshedNode) {
                      currentNode = refreshedNode;
                    }
                  }

                  // Build updatePayload: content from current store state (latest typed value),
                  // all other fields from captured values at schedule time (immune to SSE overwrites).
                  const updatePayload: Record<string, unknown> = { ...capturedNonContentFields };
                  if (changedFields.includes('content')) {
                    updatePayload['content'] = currentNode.content;
                  }

                  // Get current node version for optimistic concurrency control
                  const currentVersion = currentNode?.version ?? 1;

                  // Debug: Log version being sent
                  const shortNodeId = nodeId.substring(0, 8);
                  const contentPreview =
                    'content' in updatePayload
                      ? `"${String(updatePayload.content).substring(0, 20)}"`
                      : '(no content)';
                  log.debug(
                    `[UPDATE] ${shortNodeId}: sending version=${currentVersion}, ` +
                      `content=${contentPreview}`
                  );

                  try {
                    // Smart routing via plugin system
                    // Type-specific updaters route to node-specific methods (updateTaskNode, etc.)
                    // Generic updater falls back to node properties JSON update
                    //
                    // CRITICAL: Don't use type-specific updater when nodeType is CHANGING
                    // Type-specific updaters are only for type-specific property updates on nodes
                    // that are ALREADY of that type. Node type changes must go through generic path.
                    const nodeType = currentNode?.nodeType;
                    const isNodeTypeChanging = 'nodeType' in updatePayload;
                    const typeUpdater =
                      nodeType && !isNodeTypeChanging
                        ? pluginRegistry.getNodeUpdater(nodeType)
                        : null;

                    let updatedNodeFromBackend: Node | null = null;

                    if (typeUpdater) {
                      // Type-specific path → node-specific properties update
                      // The plugin updater handles mapping changes to type-specific fields
                      log.debug(`Using type-specific updater for ${nodeType}`);
                      updatedNodeFromBackend = await typeUpdater.update(
                        nodeId,
                        currentVersion,
                        updatePayload
                      );
                    } else {
                      // Generic path → hub table properties JSON update
                      // CRITICAL: Capture updated node to get new version from backend
                      // This prevents version conflicts on subsequent updates
                      updatedNodeFromBackend = await backendAdapter.updateNode(
                        nodeId,
                        currentVersion,
                        updatePayload
                      );
                    }

                    // Update local node with the backend's version AND typed
                    // fields. `node_to_typed_value` (the backend's single
                    // flattening authority) promotes type-specific fields —
                    // ai-chat's `provider`/`model`, task's `status`/`priority`,
                    // etc. — from the namespaced storage shape to genuinely
                    // top-level fields on this response. `updatePayload` above
                    // sends the UN-flattened `{ properties: {...} }` shape
                    // (matching storage, not the wire contract), so viewers
                    // reading those top-level fields directly (e.g.
                    // AiChatNodeViewer's `node?.provider`) never saw them
                    // become defined until a later daemon broadcast happened
                    // to re-hydrate the node via `setNode()`. Spread the
                    // response's fields over the local node so every
                    // type-specific top-level field is corrected immediately,
                    // but skip any field (besides `version`) whose local value
                    // has already moved on from `currentNode` — the pre-RPC
                    // snapshot this write's request was actually built from —
                    // UNLESS this write's own request is what's changing that
                    // field. A user (or another in-flight write, e.g. a second
                    // Kanban move on the same node fired before this RPC
                    // resolved) may have changed a field this write never
                    // touched while this request was in flight; blindly
                    // `Object.assign`-ing the full response would stamp that
                    // field back to this write's own stale pre-request value,
                    // clobbering the newer one (mirrors the equivalent,
                    // already-fixed clobber class in `updateTaskNode()`'s
                    // success handler).
                    const localNode = this.nodes.get(nodeId);
                    if (localNode && updatedNodeFromBackend) {
                      const oldVersion = localNode.version;
                      const localContent = localNode.content;
                      // Only take the backend's properties if it actually
                      // returned some — some type-specific responses (e.g.
                      // TaskNode, or a task-updater response for a
                      // properties-only change it doesn't map) carry no
                      // `properties` field at all, and `undefined` must never
                      // clobber a defined local value.
                      const localHasMovedOn =
                        localNode.properties !== currentNode.properties &&
                        JSON.stringify(localNode.properties) !==
                          JSON.stringify(currentNode.properties);
                      const localProperties =
                        localHasMovedOn || updatedNodeFromBackend.properties === undefined
                          ? localNode.properties
                          : updatedNodeFromBackend.properties;
                      const titleChanged =
                        updatedNodeFromBackend.title !== undefined &&
                        updatedNodeFromBackend.title !== localNode.title;
                      // Scope the patch: every field in the response except
                      // `content`/`properties` (handled above) and `version`
                      // (always applied — every response's version is the
                      // latest authoritative one, needed for the NEXT write's
                      // OCC check regardless of which fields it touches),
                      // applied only if this write's own request asked to
                      // change it OR the local node's current value for that
                      // field still matches the pre-RPC snapshot (nothing
                      // else moved it on in the meantime).
                      //
                      // The `changedFields.includes(key)` branch always
                      // applies THIS write's own response value for a field
                      // it changed, even if something else has since moved
                      // that field on further. That's safe only because
                      // `PersistenceCoordinator` (`persist()` above) executes
                      // at most one real RPC per node at a time — a second
                      // write for the same node while this one is executing
                      // collapses into a single queued slot and only starts
                      // once this write's response has already been applied.
                      // A future change that let two RPCs for the same node
                      // race concurrently would need this branch to also
                      // check the snapshot, not just field ownership.
                      const scopedFields: Record<string, unknown> = {};
                      const localRec = localNode as unknown as Record<string, unknown>;
                      const currentRec = currentNode as unknown as Record<string, unknown>;
                      for (const [key, value] of Object.entries(updatedNodeFromBackend)) {
                        if (key === 'content' || key === 'properties' || key === 'version') {
                          continue;
                        }
                        if (
                          changedFields.includes(key) ||
                          localRec[key] === currentRec[key]
                        ) {
                          scopedFields[key] = value;
                        }
                      }
                      Object.assign(localNode, scopedFields, {
                        content: localContent,
                        properties: localProperties,
                        version: updatedNodeFromBackend.version
                      });
                      this.nodesSet(nodeId, localNode);
                      // Notify subscribers if title changed (e.g. title_template recomputed)
                      if (titleChanged) {
                        this.notifySubscribers(nodeId, localNode, source);
                      }
                      log.debug(
                        `[UPDATE] ${shortNodeId}: success, version ${oldVersion} -> ${updatedNodeFromBackend.version}`
                      );
                    }
                  } catch (updateError) {
                    // If UPDATE fails because node doesn't exist, try CREATE instead
                    // This handles cases where persistedNodeIds is out of sync (page reload, database reset)
                    // Match various error message formats for "node not found"
                    const errorMsg =
                      updateError instanceof Error ? updateError.message.toLowerCase() : '';
                    const isNodeNotFound =
                      errorMsg.includes('not found') ||
                      errorMsg.includes('does not exist') ||
                      errorMsg.includes('nodenotfound');

                    if (updateError instanceof Error && isNodeNotFound) {
                      log.warn(
                        `Node ${nodeId} not found in database, creating instead of updating (error: ${updateError.message})`
                      );
                      const updateFallbackInput: import('$lib/services/backend-adapter').CreateNodeInput =
                        {
                          id: currentNode.id,
                          nodeType: currentNode.nodeType,
                          content: currentNode.content,
                          properties: currentNode.properties,
                          mentions: currentNode.mentions,
                          parentId: this.getParentId(nodeId),
                          insertPosition: null
                        };
                      await backendAdapter.createNode(updateFallbackInput);
                      this.persistedNodeIds.add(nodeId); // Now it's persisted
                    } else {
                      // Re-throw other errors
                      throw updateError;
                    }
                  }
                } else {
                  // Node doesn't exist yet (was a placeholder or new node)
                  // CRITICAL: Read current node state at execution time
                  const currentNode = this.nodes.get(nodeId);
                  if (!currentNode) {
                    log.warn(
                      `Node ${nodeId} no longer exists in store, skipping create persistence`
                    );
                    return;
                  }
                  const updatePathCreateInput: import('$lib/services/backend-adapter').CreateNodeInput =
                    {
                      id: currentNode.id,
                      nodeType: currentNode.nodeType,
                      content: currentNode.content,
                      properties: currentNode.properties,
                      mentions: currentNode.mentions,
                      parentId: this.getParentId(nodeId),
                      insertPosition: null
                    };
                  await backendAdapter.createNode(updatePathCreateInput);
                  this.persistedNodeIds.add(nodeId); // Track as persisted

                  // CRITICAL: Fetch the created node to get its version from backend
                  // This prevents version conflicts on subsequent updates
                  const createdNode = await backendAdapter.getNode(nodeId);
                  if (createdNode) {
                    const localNode = this.nodes.get(nodeId);
                    if (localNode) {
                      localNode.version = createdNode.version;
                      this.nodesSet(nodeId, localNode); // Update local node with backend version
                    }
                  }
                }

                // Mark update as persisted
                this.markUpdatePersisted(nodeId, update);
                onPersistSuccess?.();
              } catch (dbError) {
                const error = dbError instanceof Error ? dbError : new Error(String(dbError));

                // Check if this is a VERSION_CONFLICT error (daemon OCC)
                const occError = isVersionConflict(dbError) ? dbError : null;

                // Suppress expected errors in in-memory test mode
                if (shouldLogDatabaseErrors()) {
                  log.error(`Database write failed for node ${nodeId}:`, {
                    error,
                    fullError: dbError
                  });
                }

                // Always track errors in test environment for verification
                this.trackErrorIfTesting(error);

                // Rollback the optimistic update
                this.rollbackUpdate(nodeId, update);

                // If this is an OCC error, hydrate from authoritative current_node and notify
                if (occError) {
                  log.warn(
                    `OCC conflict for node ${nodeId}: ` +
                      `expected v${occError.conflictData.expected}, got v${occError.conflictData.actual}`
                  );

                  // Capture BEFORE clearing: was a genuinely different write
                  // (e.g. a second edit that arrived while this one was
                  // executing) queued behind this one at the moment its OCC
                  // conflict was detected? `clearQueued()` below unconditionally
                  // cancels and removes it — necessary so it doesn't retry with
                  // the now-stale version this failing write captured — but
                  // that cancellation would otherwise erase the only evidence
                  // that write ever existed by the time the fallback resync
                  // below needs to know about it (see resyncNodeFromServer's
                  // `directCallHadQueuedWrite` param doc).
                  const hadQueuedWrite = PersistenceCoordinator.getInstance().isQueued(nodeId);
                  // Clear queued operations to prevent stale-version retries
                  PersistenceCoordinator.getInstance().clearQueued(nodeId);

                  // Normalize before hydrating: this payload crosses the same
                  // sync boundary as a `database`-sourced broadcast, so it gets
                  // the same typed-field promotion. Without it a type-specific
                  // node (ai-chat, task) would land in the store with its
                  // fields still buried under `properties[<type>]` — e.g. an
                  // ai-chat node with no top-level `status`/`messages`, which
                  // strands the viewer's typing indicator after a conflict.
                  const currentNode = occError.conflictData.current_node
                    ? normalizeNodeData(occError.conflictData.current_node)
                    : null;
                  // Route the hydration through the same staleness policy a
                  // daemon broadcast gets. This path writes via `nodesSet`
                  // rather than `setNode`, so without this check the two
                  // writers into this store apply different policies and can
                  // disagree about which snapshot wins — the conflict payload
                  // could install a snapshot an already-applied broadcast had
                  // superseded. `current_node` is normally the newest state
                  // (the daemon fetches it at conflict time), so this skips
                  // only in the genuine out-of-order case.
                  const hydrationIsStale =
                    currentNode !== null &&
                    shouldSkipStaleAiChatUpdate(currentNode, this.nodes.get(nodeId), {
                      type: 'database',
                      reason: 'occ-resync'
                    });
                  if (hydrationIsStale) {
                    log.debug(
                      `OCC hydration for ${nodeId} is older than local state — ` +
                        `keeping local and resyncing`
                    );
                  }

                  if (currentNode && !hydrationIsStale) {
                    // This writes the conflict payload straight into the
                    // store, same as a `database`-sourced broadcast, so it
                    // must respect the same skip-while-editing guard
                    // `setNode()`/`resyncNodeFromServer()` enforce
                    // (`decideRemoteUpdate`). Without this, an OCC conflict
                    // on a node the user is actively editing — where the
                    // daemon's response happens to embed `current_node` —
                    // would silently clobber the optimistic, actively-edited
                    // content, exactly the class of bug #2066 closed for the
                    // fallback (`resyncNodeFromServer`) path.
                    //
                    // `hasPending` here is `hadQueuedWrite` (captured above,
                    // BEFORE `clearQueued()` ran) rather than a live
                    // `PersistenceCoordinator.hasPending()` read, for the same
                    // reason `resyncNodeFromServer`'s direct call uses it (see
                    // that method's `directCallHadQueuedWrite` param doc):
                    // this fires from inside the very write's own catch
                    // handler, before its `executingOperations` entry has
                    // cleared, so a live `hasPending()` read here would just
                    // see that same failing write's own not-yet-cleared
                    // bookkeeping and treat it as "an edit is pending" every
                    // time — self-referential and racy, not a signal of a
                    // genuinely different in-flight write.
                    const isFocused = focusManager.isNodeEditing(nodeId);
                    const decision = decideRemoteUpdate(
                      currentNode,
                      this.nodes.get(nodeId),
                      { type: 'database', reason: 'occ-resync' },
                      { isFocused, hasPending: hadQueuedWrite }
                    );

                    if (decision.apply) {
                      // Hydrate directly from the authoritative node returned by daemon
                      this.nodesSet(nodeId, currentNode);
                      this.versions.set(nodeId, currentNode.version ?? 1);
                      this.persistedNodeIds.add(nodeId);
                      this.pendingUpdates.delete(nodeId);
                      this.notifySubscribers(nodeId, currentNode, {
                        type: 'database',
                        reason: 'occ-resync'
                      });
                    } else {
                      // Node is actively being edited — keep the local,
                      // in-progress content and skip the clobber. The
                      // conflict response fetch still proves the node exists
                      // server-side, so mark it persisted the same way the
                      // apply branch would; only the content/version
                      // overwrite is skipped. This call site always raises
                      // its own conflict notification unconditionally below
                      // regardless of branch, so no separate notify here
                      // (unlike `resyncNodeFromServer`'s skip branch, which
                      // has no such external caller for its queued-follow-up
                      // case).
                      this.persistedNodeIds.add(nodeId);
                      log.debug(
                        `OCC direct hydration for ${nodeId} skipped — node is actively being edited (focused=${isFocused})`
                      );
                    }
                  } else {
                    // Fallback: fetch from server if daemon didn't embed current_node
                    this.resyncNodeFromServer(nodeId, false, hadQueuedWrite).catch(
                      (resyncError) => {
                        log.error(
                          `Failed to resync after OCC error for node ${nodeId}:`,
                          resyncError
                        );
                      }
                    );
                  }

                  conflictNotifications.add({
                    nodeId,
                    message: CONFLICT_MESSAGE['version-mismatch'],
                    conflictType: 'version-mismatch'
                  });
                  occConflictAlreadyNotified = true;
                } else if (onPersistError) {
                  // Non-OCC failure (network error, validation error, daemon
                  // offline, etc.): the optimistic write above never landed
                  // server-side. `rollbackUpdate()` only rewinds bookkeeping
                  // (metrics, the version counter, the pending-update list) —
                  // `NodeUpdate` carries no previous-value snapshot, so it
                  // cannot restore the field values `updateNode` already
                  // applied to `this.nodes`.
                  //
                  // An earlier version of this fix called `resyncNodeFromServer`
                  // here unconditionally — refetching the whole node from the
                  // server to correct the divergence, the same authoritative
                  // refetch the OCC fallback below uses. Review surfaced three
                  // real problems specific to using that store-wide mechanism
                  // for an arbitrary non-OCC failure (as opposed to its
                  // original, narrower OCC-conflict use): a failed *create*
                  // (server has nothing to return) leaves a permanent phantom
                  // node with no correction possible; the skip-while-editing
                  // guard it needs (see `resyncNodeFromServer`) has no
                  // meaningful "hasPending" signal available to it (see that
                  // method's own comment) and so can't tell this failing
                  // write's optimistic content apart from a second, genuinely
                  // different, still-in-flight write to the same node — and
                  // can clobber the latter; and a resync skipped because the
                  // node was actively focused has no retry, so a divergence
                  // caught mid-edit can stay uncorrected indefinitely. All
                  // three trace back to the same root cause: a full-node
                  // server round-trip is the wrong grain of correction for
                  // "one specific write, to one specific field, failed" — it
                  // can only either replace everything or nothing, and
                  // "everything" is exactly what creates the races above.
                  //
                  // `onPersistError` instead lets the *caller that made this
                  // specific write* — which already knows exactly which
                  // field(s) it changed and what the prior value was — make a
                  // narrowly-scoped local correction (see kanban-view.svelte's
                  // `moveCard`) with none of that: no server round-trip, no
                  // guard needed, no reliance on a fetch racing an unrelated
                  // write to the same node. Opt-in and additive: a caller that
                  // doesn't pass it gets exactly the pre-existing behavior
                  // (rollbackUpdate's bookkeeping + the write-failure
                  // notification below), same as before this callback existed.
                  onPersistError(error);
                }

                throw error; // Re-throw to mark operation as failed in coordinator
              }
            },
            {
              mode:
                isStructuralChange || isPropertyChange || isNodeTypeChange || isTypeSpecificChange
                  ? 'immediate'
                  : 'debounce',
              dependencies: dependencies.length > 0 ? dependencies : undefined
            }
          );

          // Handle cancellation errors (expected when operations are superseded)
          handle.promise.catch((err) => {
            if (err instanceof OperationCancelledError) {
              // Operation was cancelled by a newer operation - this is expected
              return;
            }
            // An OCC error already raised its own specific version-mismatch
            // notification inside the persistence closure's own catch above
            // (see `occConflictAlreadyNotified`'s declaration for why this is
            // a captured flag rather than re-deriving it from `err` via
            // `isVersionConflict` — the re-thrown `err` has already lost the
            // shape that check needs).
            if (occConflictAlreadyNotified) return;
            // Surface non-OCC write failures visibly so users know their change didn't save
            conflictNotifications.add({
              nodeId,
              message: CONFLICT_MESSAGE['write-failure'],
              conflictType: 'write-failure'
            });
          });
        }
      }
    } catch (error) {
      log.error(`Error updating node ${nodeId}:`, error);
      throw error;
    } finally {
      // Only time work that happened. Recording a skipped call would time a
      // failed `Map` lookup and fold it into the average of real updates.
      if (didUpdate) {
        this.recordMetric(performance.now() - startTime);
      }
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
  setNode(rawNode: Node, source: UpdateSource, skipPersistence = false): void {
    // Normalize typed node shapes (e.g. AiChatNode) whenever data arrives from
    // the backend so the store always holds the typed shape, not raw wire data.
    const node = source.type === 'database' ? normalizeNodeData(rawNode) : rawNode;

    const isNewNode = !this.persistedNodeIds.has(node.id);

    // Track hierarchy changes for logging
    // New nodes trigger hierarchy change, content-only updates do not
    const existingNode = this.nodes.get(node.id);
    const isHierarchyChange = !existingNode;

    // Skip-while-editing guard, policy extracted to `remote-update-policy.ts`
    // (`decideRemoteUpdate`). A daemon-broadcast event (source.type ===
    // 'database') arriving for a node the user is actively editing — or has
    // unsaved local changes for — would otherwise overwrite the optimistic
    // store with the *older* server-confirmed state. The optimistic state is
    // authoritative until persistence settles, so we keep the local content.
    // Every such event is a genuine foreign write: the daemon suppresses this
    // client's own write echoes before they ever reach here (ADR-026's C5 extension),
    // so there is nothing left to classify.
    //
    // CRITICAL: do NOT call `this.nodes.set()` or mutate any property of a
    // node inside the reactive Map. Either triggers Svelte re-renders that
    // remount the textarea (the `{#if isEditing}` block in base-node.svelte)
    // and reset selectionStart.
    //
    // `source.type === 'database'` is the contract for "this update came from
    // the daemon's domain-event broadcast" — see UpdateSource in
    // `$lib/types/update-protocol`. Local user actions use `'viewer'`. The
    // guard relies on no other producer of `'database'` events bypassing the
    // intended skip behavior; the only consumers today are
    // `tauri-sync-listener` and `browser-sync-service`.
    //
    // Fixes typing corruption (chars dropped/replaced
    // under sustained input as the optimistic store is clobbered by the
    // daemon's own confirmation looped back through the WatchNodes stream).
    //
    // Compute the predicates once into locals: (a) `hasPending` does three
    // Map lookups, and (b) the coordinator transitions a node between its
    // pending/executing/queued maps, so reading it twice can return
    // different answers — the log message would otherwise contradict the
    // branch taken.
    const isFocused = focusManager.editingNodeId === node.id;
    const hasPending = PersistenceCoordinator.getInstance().hasPending(node.id);

    if (shouldSkipStaleAiChatUpdate(node, existingNode, source)) {
      log.debug(`setNode: skipping ai-chat database update with fewer messages`, {
        nodeId: node.id
      });
      return;
    }

    const decision = decideRemoteUpdate(node, existingNode, source, { isFocused, hasPending });

    if (!decision.apply) {
      log.debug(
        `setNode: skipping clobber of actively-edited node ${node.id} ` +
          `(focused=${isFocused}, pending=${hasPending})`
      );
      if (decision.notifyConflict) {
        // A foreign write to a node the user is actively editing. We skip
        // the clobber to protect the optimistic text, but that must not be
        // silent — raise a version-mismatch notification (deduped per node)
        // so the conflict is visible.
        const alreadyFlagged = conflictNotifications.notifications.some(
          (n) => n.nodeId === node.id && n.conflictType === 'version-mismatch'
        );
        if (!alreadyFlagged) {
          conflictNotifications.add({
            nodeId: node.id,
            message: CONFLICT_MESSAGE['version-mismatch'],
            conflictType: 'version-mismatch'
          });
        }
      }
      // `persistedNodeIds.add` is safe here precisely because the guard
      // only runs when `existingNode` is truthy — a database event for a
      // node we've already seen implies the node IS persisted server-side.
      // Do not remove the `existingNode` check thinking the add is
      // unconditional bookkeeping; it is not.
      this.persistedNodeIds.add(node.id);
      // Do NOT touch this.nodes or notify subscribers — there is no
      // observable change to the local view, and any reactive write here
      // remounts the focused textarea.
      return;
    }

    this.nodesSet(node.id, node);
    this.versions.set(node.id, this.getNextVersion(node.id));
    this.notifySubscribers(node.id, node, source);

    if (isHierarchyChange) {
      log.debug(`Hierarchy change for node: ${node.id}`);
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
    // Phase 1: Eliminate ephemeral nodes during editing
    // - Only skip persistence when explicitly requested via skipPersistence flag
    // - This flag is ONLY true for initial viewer placeholder (when no children exist)
    // - All other blank nodes (created via Enter key, etc.) persist immediately
    const persistBehavior = this.determinePersistenceBehavior(source, options);
    if (persistBehavior.shouldPersist) {
      const shouldPersist = source.type !== 'viewer' || isNewNode;

      if (shouldPersist) {
        // No placeholder checks - all real nodes should be persisted

        // Delegate to PersistenceCoordinator
        // CRITICAL FIX: Track InsertPosition.After sibling as dependency to prevent race conditions
        // When creating a node with After(siblingId), the referenced sibling MUST exist in DB first
        // Otherwise backend fails with "Node 'xyz' does not exist"
        const dependencies: Array<string | (() => Promise<void>)> = [];

        // If this node inserts After a sibling, wait for that sibling to be persisted first
        const insertPos = (node as Node & { insertPosition?: InsertPosition | null })
          .insertPosition;
        const afterSiblingId = insertPos?.type === 'after' ? insertPos.siblingId : undefined;
        if (afterSiblingId && !this.persistedNodeIds.has(afterSiblingId)) {
          dependencies.push(afterSiblingId);
        }

        // Always persist the full node including content
        // Real nodes (even with only syntax like "## ") must include content field for backend validation
        // The old code stripped content for "placeholder" header nodes, but now all user-created nodes
        // should persist with their full content, even if it's just syntax

        // Capture handle to catch cancellation errors
        // CRITICAL: Only capture the node ID, not the node object itself.
        // The operation closure must read CURRENT state from this.nodes at execution time,
        // not the stale state captured when persist() was called.
        // This prevents race conditions where rapid typing causes earlier states to overwrite later ones.
        const nodeId = node.id;
        const handle = PersistenceCoordinator.getInstance().persist(
          nodeId,
          async () => {
            try {
              // CRITICAL: Read current node state at execution time, not capture time
              // This ensures we persist the latest content, not stale content from when persist() was called
              let currentNode = this.nodes.get(nodeId);
              if (!currentNode) {
                log.warn(`Node ${nodeId} no longer exists in store, skipping persistence`);
                return;
              }

              // Check if node has been persisted - use in-memory tracking to avoid database query
              const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);
              if (isPersistedToDatabase) {
                // CRITICAL: Wait for any pending move operation to complete before UPDATE.
                // Move operations (indent/outdent) increment the version in the backend.
                // If we UPDATE before the move completes, we'll have a version mismatch.
                const pendingMove = getPendingMoveOperation(nodeId);
                if (pendingMove) {
                  log.debug(
                    `[UPDATE] Waiting for pending move operation on ${nodeId.substring(0, 8)}`
                  );
                  await pendingMove;
                  // Re-read current node to get updated version after move
                  const refreshedNode = this.nodes.get(nodeId);
                  if (refreshedNode) {
                    currentNode = refreshedNode;
                  }
                }

                try {
                  // Get current version for optimistic concurrency control.
                  const currentVersion = this.computeOccVersionForUpdate(nodeId);
                  const updatedFromBackend = await backendAdapter.updateNode(
                    nodeId,
                    currentVersion,
                    currentNode
                  );
                  // Sync the backend-assigned version AND typed fields into the
                  // local node. `node_to_typed_value` (the backend's single
                  // flattening authority) promotes type-specific fields — ai-chat's
                  // `provider`/`model`, task's `status`/`priority`, etc. — from the
                  // namespaced storage shape to genuinely top-level fields on this
                  // response. The optimistic write above sent the UN-flattened
                  // `{ properties: {...} }` shape client-side (matching storage, not
                  // the wire contract), so the local node's top-level typed fields —
                  // read directly by viewers like AiChatNodeViewer's
                  // `node?.provider` — never got corrected to match. Previously only
                  // `.version` was synced here, so e.g. an ai-chat model selection
                  // persisted correctly server-side but the local node never
                  // observed `provider`/`model` becoming defined, leaving the UI
                  // stuck on "Choose a model to get started" even after the write
                  // succeeded.
                  //
                  // Spread the response over the local node so every
                  // type-specific top-level field is corrected, but scope the
                  // patch: apply a field from the response only if the local
                  // node's current value for it still matches `currentNode` —
                  // the pre-RPC snapshot this write's request was actually
                  // built from. `currentNode` here IS the full payload this
                  // write sent, so "still matches" covers both "this write
                  // changed it" (response reflects this write's own value)
                  // and "nothing changed it" (safe to take the backend's
                  // value, including a newly-promoted typed field where both
                  // sides are `undefined`). If a user (or another in-flight
                  // write, e.g. a second Kanban move on the same node fired
                  // before this RPC resolved) changed a field this write
                  // never touched while this request was in flight, the
                  // field no longer matches `currentNode` and is left alone —
                  // otherwise this response's stale value would clobber the
                  // newer one (mirrors the equivalent, already-fixed clobber
                  // class in `updateNode()`'s success handler). `content` and
                  // `properties` keep their own special-case handling;
                  // `version` is always applied — every response's version is
                  // the latest authoritative one, needed for the next
                  // write's OCC check regardless of which fields it touches.
                  // `properties` is compared shallowly since callers replace
                  // it wholesale rather than patching individual keys.
                  //
                  // Comparing against a SINGLE `currentNode` snapshot (rather
                  // than tracking per-field ownership) is safe only because
                  // `PersistenceCoordinator` (`persist()`, this closure's
                  // caller) executes at most one real RPC per node at a
                  // time — a second `setNode()` write for the same node
                  // while this one is executing collapses into a single
                  // queued slot and only starts once this write's response
                  // has already been applied here. A future change that let
                  // two RPCs for the same node race concurrently would need
                  // this comparison to also account for that, not just
                  // compare against the one snapshot (mirrors the identical
                  // invariant `updateNode()`'s own success handler relies
                  // on).
                  const latestNode = this.nodes.get(nodeId);
                  if (latestNode && updatedFromBackend) {
                    const localContent = latestNode.content;
                    // Only take the backend's properties if it actually
                    // returned some — `undefined` must never clobber a
                    // defined local value.
                    const localHasMovedOn =
                      latestNode.properties !== currentNode.properties &&
                      JSON.stringify(latestNode.properties) !==
                        JSON.stringify(currentNode.properties);
                    const localProperties =
                      localHasMovedOn || updatedFromBackend.properties === undefined
                        ? latestNode.properties
                        : updatedFromBackend.properties;
                    const scopedFields: Record<string, unknown> = {};
                    const localRec = latestNode as unknown as Record<string, unknown>;
                    const currentRec = currentNode as unknown as Record<string, unknown>;
                    for (const [key, value] of Object.entries(updatedFromBackend)) {
                      if (key === 'content' || key === 'properties' || key === 'version') {
                        continue;
                      }
                      if (localRec[key] === currentRec[key]) {
                        scopedFields[key] = value;
                      }
                    }
                    Object.assign(latestNode, scopedFields, {
                      content: localContent,
                      properties: localProperties,
                      version: updatedFromBackend.version
                    });
                    this.nodesSet(nodeId, latestNode);
                  }
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
                    log.warn(
                      `Node ${nodeId} not found in database, creating instead of updating (error: ${errorMessage})`
                    );
                    const fallbackCreateInput: import('$lib/services/backend-adapter').CreateNodeInput =
                      {
                        id: currentNode.id,
                        nodeType: currentNode.nodeType,
                        content: currentNode.content,
                        properties: currentNode.properties,
                        mentions: currentNode.mentions,
                        parentId: this.getParentId(nodeId),
                        insertPosition: null
                      };
                    await backendAdapter.createNode(fallbackCreateInput);
                    this.persistedNodeIds.add(nodeId);
                  } else {
                    throw updateError;
                  }
                }
              } else {
                const nodeWithInsertPos = currentNode as Node & {
                  insertPosition?: InsertPosition | null;
                };
                if (
                  nodeWithInsertPos.insertPosition?.type === 'after' &&
                  nodeWithInsertPos.insertPosition.siblingId
                ) {
                  const siblingId = nodeWithInsertPos.insertPosition.siblingId;
                  const currentParentId = this.getParentId(nodeId);
                  if (this.shouldClearStaleInsertAfter(siblingId, currentParentId)) {
                    log.debug(
                      `[CREATE] Clearing stale insertPosition.after for ${nodeId.substring(0, 8)}: ` +
                        `sibling ${siblingId.substring(0, 8)} reports a ` +
                        `different parent via structureTree (structureTree parent=${currentParentId?.substring(0, 8) ?? 'null'})`
                    );
                    nodeWithInsertPos.insertPosition = { type: 'end' };
                  }
                }

                // Derive parent from structureTree (single source of truth for hierarchy)
                const createInput: import('$lib/services/backend-adapter').CreateNodeInput = {
                  id: currentNode.id,
                  nodeType: currentNode.nodeType,
                  content: currentNode.content,
                  properties: currentNode.properties,
                  mentions: currentNode.mentions,
                  parentId: this.getParentId(nodeId),
                  insertPosition: nodeWithInsertPos.insertPosition ?? null
                };
                await backendAdapter.createNode(createInput);
                this.persistedNodeIds.add(nodeId); // Track as persisted

                // CRITICAL: Fetch the created node to get its version from backend
                // This prevents version conflicts on subsequent updates
                const createdNode = await backendAdapter.getNode(nodeId);
                if (createdNode) {
                  // BUG FIX: Only update the VERSION, not the entire node!
                  // The user may have continued typing while createNode was in flight.
                  // We must preserve their local changes and only take the version from backend.
                  const latestLocalNode = this.nodes.get(nodeId);
                  if (latestLocalNode) {
                    latestLocalNode.version = createdNode.version;
                    this.nodesSet(nodeId, latestLocalNode);
                  }
                }
              }
            } catch (dbError) {
              // Properly stringify Tauri errors which come as plain objects
              const errorMessage =
                dbError instanceof Error
                  ? dbError.message
                  : typeof dbError === 'object' && dbError !== null
                    ? JSON.stringify(dbError)
                    : String(dbError);
              const error = dbError instanceof Error ? dbError : new Error(errorMessage);

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                log.error(`Database write failed for node ${node.id}:`, errorMessage);
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              throw error; // Re-throw to mark operation as failed in coordinator
            }
          },
          {
            // Use debounce mode for new viewer nodes to coalesce rapid updates.
            // This allows indent/outdent to update structureTree BEFORE the CREATE fires,
            // enabling single-transaction create-with-correct-parent instead of CREATE + MOVE.
            // The indentNode function checks isNodePersisted() and handles unpersisted nodes
            // by updating structureTree and re-triggering setNode (cancelling the previous pending CREATE).
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
          // Unlike updateNode()/deleteNode()/updateTaskNode(), this closure's
          // own `catch (dbError)` above has no OCC-specific branch — it never
          // raises a version-mismatch notification of its own for this write
          // to duplicate, so there is nothing here to skip. (An
          // `isVersionConflict(err)` guard mirroring those other call sites
          // used to sit here; removed as dead code — it could never match
          // anyway, since the closure re-throws a generic wrapped `Error`
          // that has already lost the original error's shape, and even if it
          // had matched, there was no earlier notification to avoid
          // duplicating.) Every failure this write can produce, OCC
          // conflicts included, surfaces through the generic notification
          // below — confirmed by test, not just this comment.
          conflictNotifications.add({
            nodeId,
            message: CONFLICT_MESSAGE['write-failure'],
            conflictType: 'write-failure'
          });
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

    // Normalize typed node shapes from the backend before storing
    const normalizedNodes = source.type === 'database' ? nodes.map(normalizeNodeData) : nodes;

    // Add all nodes to the store
    for (const node of normalizedNodes) {
      const existingNode = this.nodes.get(node.id);

      // Same guard as setNode: never overwrite an ai-chat node with a stale
      // snapshot (older version, or same version with fewer messages). These
      // nodes come from a fresh tree load, so they carry current server
      // versions — which is exactly what the guard compares on.
      if (shouldSkipStaleAiChatUpdate(node, existingNode, source)) {
        log.debug(`batchSetNodes: skipping ai-chat stale snapshot`, { nodeId: node.id });
        continue;
      }

      // Same skip-while-editing guard/policy as setNode: a
      // concurrent tree (re)load with a `database` source must NOT clobber a
      // node the user is actively editing — `doLoadChildrenTree` passes a
      // database source, so a reload for a parent whose child is mid-keystroke
      // would overwrite the child's optimistic content.
      const isFocused = focusManager.editingNodeId === node.id;
      const hasPending = PersistenceCoordinator.getInstance().hasPending(node.id);
      const decision = decideRemoteUpdate(node, existingNode, source, { isFocused, hasPending });
      if (!decision.apply) {
        log.debug(
          `batchSetNodes: skipping clobber of actively-edited node ${node.id} ` +
            `(focused=${isFocused}, pending=${hasPending})`
        );
        // Every database-sourced event reaching here is a genuine foreign
        // write (the daemon suppresses this client's own echoes before they
        // arrive — ADR-026's C5 extension): leave the node's version alone so the next
        // RPC uses our local version, conflicts, and surfaces the foreign
        // change (preserves OCC). batchSetNodes does not raise conflict
        // notifications (pre-existing behavior, unchanged here).
        this.persistedNodeIds.add(node.id);
        continue;
      }

      const isHierarchyChange = !existingNode;

      if (isHierarchyChange) {
        hasHierarchyChanges = true;
      }

      this.nodesSet(node.id, node);
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
      log.debug(`Batch hierarchy change: ${nodes.length} nodes added`);
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
   * @param onRefused - Called if the backend refuses the delete via the subtree
   *   access gate, AFTER this store restores its own state. Lets a caller that
   *   also removed the node optimistically from its OWN state (e.g. the reactive
   *   view service's `_rootNodeIds`/`_uiState`) restore that too — the store can't
   *   reach those layers. Not called for success or any other error.
   */
  deleteNode(
    nodeId: string,
    source: UpdateSource,
    skipPersistence = false,
    dependencies: string[] = [],
    onRefused?: () => void
  ): void {
    // Cancel any active batch before deletion
    this.cancelBatch(nodeId);

    const node = this.nodes.get(nodeId);
    if (node) {
      // Capture what the optimistic removal is about to strip, so a backend refusal
      // (subtree-access-denied) can restore the node exactly as it was.
      const removedVersion = this.versions.get(nodeId);
      const wasPersisted = this.persistedNodeIds.has(nodeId);
      // Captured (not cloned) — safe to hand the same Map back on restore.
      // `updateTaskNode()` is the only caller of `bumpTaskFieldSeq()`/
      // `getTaskFieldSeq()`, and it no-ops whenever `this.nodes.get(nodeId)`
      // is missing (see its own `existingNode` guard) — so nothing can touch
      // this node's field-sequence map while it's optimistically deleted and
      // not yet restored.
      const removedTaskFieldSeq = this.taskFieldWriteSeq.get(nodeId);

      this.nodesDelete(nodeId);
      this.versions.delete(nodeId);
      this.pendingUpdates.delete(nodeId);
      this.persistedNodeIds.delete(nodeId); // Remove from tracking set
      this.taskFieldWriteSeq.delete(nodeId);
      this.cancelPendingEviction(nodeId); // Node is gone — nothing left to evict
      this.notifySubscribers(nodeId, node, source);

      log.debug(`Node deleted: ${nodeId}`);

      // Phase 2.4: Persist deletion to database
      const persistBehavior = this.determinePersistenceBehavior(source, { skipPersistence });
      if (persistBehavior.shouldPersist) {
        // Filter dependencies to only include nodes with pending persistence operations
        const pendingDeps = dependencies.filter((depId) =>
          PersistenceCoordinator.getInstance().isPending(depId)
        );

        // Set from inside the closure below when a subtree-access-denied
        // refusal has already raised its own specific notification, so the
        // outer handle.promise.catch() can skip piling a second, generic
        // one on top. Can't just re-check `isSubtreeAccessDenied` on the
        // outer catch's error: the closure re-throws `error` (`dbError
        // instanceof Error ? dbError : new Error(String(dbError))`), which
        // for a plain-object CommandError (the real shape errors cross the
        // Tauri/gRPC boundary in — see `isSubtreeAccessDenied`'s own doc
        // comment) is NOT `instanceof Error`, so it gets wrapped in a fresh
        // generic `Error` that has lost the `.code`/`.conflictData` shape
        // entirely by the time it reaches the outer catch.
        let subtreeAccessDeniedAlreadyNotified = false;

        // Capture handle to catch cancellation errors
        const handle = PersistenceCoordinator.getInstance().persist(
          nodeId,
          async () => {
            try {
              // Get current version for optimistic concurrency control
              // Note: node has already been removed from this.nodes, so we use the captured node variable
              const currentVersion = node.version ?? 1;
              await backendAdapter.deleteNode(nodeId, currentVersion);
            } catch (dbError) {
              const error = dbError instanceof Error ? dbError : new Error(String(dbError));

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                log.error(`Database deletion failed for node ${nodeId}:`, error);
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              // A cascade delete refused by the ADR-041 subtree access gate: the node
              // was already removed optimistically, but nothing was deleted on the
              // backend. Restore exactly what the optimistic removal stripped and
              // surface the refusal to the UI. Non-refusal errors keep today's
              // behavior (the removal stands, error re-thrown to the coordinator).
              if (isSubtreeAccessDenied(dbError)) {
                this.nodesSet(nodeId, node);
                if (removedVersion !== undefined) {
                  this.versions.set(nodeId, removedVersion);
                }
                if (wasPersisted) {
                  this.persistedNodeIds.add(nodeId);
                }
                // Restore the field-sequence map alongside `versions`/
                // `persistedNodeIds` above — otherwise a refused delete would
                // leave a resurrected node's per-field write-sequence
                // counters silently reset to zero, which a future change
                // elsewhere in `updateTaskNode()` could turn into a real
                // same-field clobber (the exact class this file's
                // `bumpTaskFieldSeq()`/`getTaskFieldSeq()` exist to prevent).
                if (removedTaskFieldSeq !== undefined) {
                  this.taskFieldWriteSeq.set(nodeId, removedTaskFieldSeq);
                }
                this.notifySubscribers(nodeId, node, source);

                // Let the caller restore its own optimistic removal (view layer)
                // before we surface the refusal.
                onRefused?.();

                showSubtreeAccessDenied(dbError.conflictData.inaccessibleCount);
                subtreeAccessDeniedAlreadyNotified = true;
              }

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
          // A subtree-access-denied refusal already restored the node and
          // surfaced its own specific notification (showSubtreeAccessDenied)
          // inside the persist closure's own catch above, before re-throwing
          // to mark the coordinator operation failed. Same intent as the
          // isVersionConflict check at the other call sites — don't pile a
          // second, generic notification on top of the one already raised
          // for the exact same event — but implemented via the captured
          // flag rather than re-deriving it from `err` (see
          // `subtreeAccessDeniedAlreadyNotified`'s declaration for why
          // re-checking `isSubtreeAccessDenied(err)` here doesn't work).
          if (subtreeAccessDeniedAlreadyNotified) return;
          // Surface genuine deletion failures visibly, INCLUDING a
          // DependencyFailedError — a dependency this deletion was waiting
          // on (a node that had to finish persisting first, to avoid a
          // FOREIGN KEY violation on the delete) failing or being
          // cancelled. In that case this deletion's own backend RPC never
          // even ran, and nothing else is going to retry it, so leaving
          // this silent (the prior behavior — "Real errors are logged by
          // PersistenceCoordinator" was not actually true; nothing
          // surfaced to the user either way) would mean the node stays
          // gone locally while the deletion never actually reaches the
          // server.
          conflictNotifications.add({
            nodeId,
            message: CONFLICT_MESSAGE['write-failure'],
            conflictType: 'write-failure'
          });
        });
      }
    }
  }

  /**
   * Update a task node with type-safe property updates
   *
   * Routes task-specific field updates (status, priority, dueDate) through
   * the type-safe update path that directly modifies task node properties in the backend.
   *
   * This method provides end-to-end type safety for task updates:
   * - Frontend sends TaskNodeUpdate (not generic NodeUpdate)
   * - Backend updates task node properties directly (not via JSON properties blob)
   * - Returns TaskNode with updated fields and new version
   *
   * @param nodeId - Task node ID to update
   * @param update - TaskNodeUpdate with task-specific fields to update
   * @param source - Source of the update (viewer, database, MCP)
   */
  updateTaskNode(
    nodeId: string,
    update: import('$lib/types').TaskNodeUpdate,
    source: UpdateSource
  ): void {
    const existingNode = this.nodes.get(nodeId);
    if (!existingNode) {
      log.warn(`Cannot update non-existent task node: ${nodeId}`);
      return;
    }

    if (existingNode.nodeType !== 'task') {
      log.warn(
        `updateTaskNode called on non-task node: ${nodeId} (type: ${existingNode.nodeType})`
      );
      return;
    }

    // Apply update optimistically to local state
    // Map TaskNodeUpdate fields to TaskNode properties for local state
    const localChanges: Partial<TaskNode> = {};
    // Per-field write-sequence numbers captured at THIS write's own
    // optimistic-apply moment — see `bumpTaskFieldSeq()`'s doc comment. Used
    // below (inside the persistence closure) to detect whether a second,
    // same-field `updateTaskNode()` write raced in before this write's RPC
    // resolved, so this write's confirmed value for that field can be
    // skipped instead of transiently clobbering the newer one.
    const myFieldSeq: Partial<Record<TaskFieldName, number>> = {};
    if (update.status !== undefined) {
      localChanges.status = update.status;
      myFieldSeq.status = this.bumpTaskFieldSeq(nodeId, 'status');
    }
    if (update.priority !== undefined) {
      // TaskNodeUpdate allows null to clear priority; TaskNode uses undefined
      localChanges.priority = update.priority ?? undefined;
      myFieldSeq.priority = this.bumpTaskFieldSeq(nodeId, 'priority');
    }
    if (update.dueDate !== undefined) {
      localChanges.dueDate = update.dueDate;
      myFieldSeq.dueDate = this.bumpTaskFieldSeq(nodeId, 'dueDate');
    }
    if (update.startedAt !== undefined) {
      localChanges.startedAt = update.startedAt;
      myFieldSeq.startedAt = this.bumpTaskFieldSeq(nodeId, 'startedAt');
    }
    if (update.completedAt !== undefined) {
      localChanges.completedAt = update.completedAt;
      myFieldSeq.completedAt = this.bumpTaskFieldSeq(nodeId, 'completedAt');
    }
    if (update.content !== undefined) {
      localChanges.content = update.content;
      myFieldSeq.content = this.bumpTaskFieldSeq(nodeId, 'content');
    }

    // Update local node optimistically
    // Cast is safe: existingNode.nodeType === 'task' is verified above
    const updatedNode = { ...existingNode, ...localChanges } as unknown as Node;
    this.nodesSet(nodeId, updatedNode);
    this.notifySubscribers(nodeId, updatedNode, source);

    // Set from inside the closure below when an OCC (version-conflict) error
    // has already raised its own specific `version-mismatch` notification, so
    // the outer `handle.promise.catch()` can skip piling a second, generic
    // `write-failure` one on top. Can't just re-check `isVersionConflict` on
    // the outer catch's `err`: the closure re-throws `error` (`dbError
    // instanceof Error ? dbError : new Error(String(dbError))`), which for a
    // plain-object CommandError (the real shape errors cross the Tauri/gRPC
    // boundary in — see `isVersionConflict`'s own doc comment) is NOT
    // `instanceof Error`, so it gets wrapped in a fresh generic `Error` that
    // has lost the `.code`/`.conflictData` shape entirely by the time it
    // reaches the outer catch. Same fix `updateNode()`/`deleteNode()` already
    // apply for their analogous captured flags.
    let occConflictAlreadyNotified = false;

    // Capture handle to catch cancellation errors
    const handle = PersistenceCoordinator.getInstance().persist(
      nodeId,
      async () => {
        try {
          // Read version at EXECUTION time (not call time) to pick up any
          // resync that occurred while this operation was queued
          const currentNode = this.nodes.get(nodeId);
          const currentVersion = currentNode?.version ?? existingNode.version ?? 1;

          const updatedTaskNode = await backendAdapter.updateTaskNode(
            nodeId,
            currentVersion,
            update
          );

          // Update local node with backend version
          const localNode = this.nodes.get(nodeId);
          if (localNode && updatedTaskNode) {
            // `PersistenceCoordinator` serializes real RPCs per node — only
            // one `updateTaskNode()` write executes at a time — so the
            // version this response carries is always the latest
            // authoritative one and is always safe to apply.
            localNode.version = updatedTaskNode.version;

            // Only apply the type-specific (and content) fields THIS
            // write's own request actually asked to change. `localNode` is
            // whatever `this.nodes.get(nodeId)` returns AT THE MOMENT this
            // write's RPC resolves — not necessarily the object this
            // write's own optimistic update installed. If a second
            // `updateTaskNode()` call for the same node was submitted while
            // this one was still executing, it collapses into
            // `PersistenceCoordinator`'s queued slot and applies its OWN
            // optimistic update (a different object, or the same object
            // mutated further) immediately — before this write's RPC even
            // settles. `updatedTaskNode` reflects the server's task row as
            // of THIS write's OWN request, which is frozen for every field
            // this write never changed (their values just reflect
            // whatever the server already had when this write was sent).
            // Unconditionally `Object.assign`-ing all six type-specific
            // fields (the previous behavior) would silently overwrite a
            // concurrently-resolving write's optimistic or already-
            // confirmed value for any field THIS write didn't intend to
            // touch. Building a patch limited to the fields this write's
            // own `update` specified avoids that: a field this write
            // didn't ask to change is left exactly as the store currently
            // has it, however it got there.
            //
            // Field scoping alone doesn't close the SAME-field case: if a
            // second write also changes (say) `status` while this write's
            // RPC is still in flight, this write's own confirmed `status`
            // is stale by the time it lands — it reflects the server's
            // state as of THIS write's request, not the newer optimistic
            // value the second write already applied. Each field's guard
            // below (`this.getTaskFieldSeq(...) === myFieldSeq.field`)
            // detects that: if the field's write-sequence has moved past
            // the number this write captured at its own optimistic-apply
            // time, a newer same-field write raced in, and this write skips
            // applying its now-stale value for that one field rather than
            // clobbering the newer one. The newer write's own RPC (queued
            // behind this one) applies its own confirmed value when it
            // resolves, so the store still converges correctly.
            const confirmedFields: Partial<TaskNode> = {};
            if (
              update.status !== undefined &&
              this.getTaskFieldSeq(nodeId, 'status') === myFieldSeq.status
            ) {
              confirmedFields.status = updatedTaskNode.status;
            }
            if (
              update.priority !== undefined &&
              this.getTaskFieldSeq(nodeId, 'priority') === myFieldSeq.priority
            ) {
              confirmedFields.priority = updatedTaskNode.priority;
            }
            if (
              update.dueDate !== undefined &&
              this.getTaskFieldSeq(nodeId, 'dueDate') === myFieldSeq.dueDate
            ) {
              confirmedFields.dueDate = updatedTaskNode.dueDate;
            }
            if (
              update.startedAt !== undefined &&
              this.getTaskFieldSeq(nodeId, 'startedAt') === myFieldSeq.startedAt
            ) {
              confirmedFields.startedAt = updatedTaskNode.startedAt;
            }
            if (
              update.completedAt !== undefined &&
              this.getTaskFieldSeq(nodeId, 'completedAt') === myFieldSeq.completedAt
            ) {
              confirmedFields.completedAt = updatedTaskNode.completedAt;
            }
            // Use Object.assign to safely update fields that may not exist on Node interface
            Object.assign(localNode, confirmedFields);
            if (
              update.content !== undefined &&
              updatedTaskNode.content !== undefined &&
              this.getTaskFieldSeq(nodeId, 'content') === myFieldSeq.content
            ) {
              localNode.content = updatedTaskNode.content;
            }
            this.nodesSet(nodeId, localNode);
          }
        } catch (dbError) {
          const error = dbError instanceof Error ? dbError : new Error(String(dbError));
          const occError = isVersionConflict(dbError) ? dbError : null;

          // Suppress expected errors in in-memory test mode
          if (shouldLogDatabaseErrors()) {
            log.error(`Task update failed for node ${nodeId}:`, error);
          }

          // Always track errors in test environment for verification
          this.trackErrorIfTesting(error);

          // Rollback bookkeeping only — do NOT force-revert the store's
          // content to `existingNode` (this call's own captured pre-edit
          // snapshot). `updateTaskNode()` has no `NodeUpdate`/`pendingUpdates`
          // tracking the way `updateNode()` does, so there is no
          // `rollbackUpdate()`-equivalent history to unwind here — but
          // unconditionally calling `nodesSet(nodeId, existingNode)` would
          // clobber a second, genuinely queued `updateTaskNode()` write's
          // optimistic value (or its already-confirmed server result) if
          // that write applied its own update to this node while this one
          // was still executing. Notify subscribers with whatever the store
          // CURRENTLY holds instead — mirrors `updateNode()`'s sibling catch
          // handler, which calls `rollbackUpdate()` and never calls
          // `nodesSet()`/touches node content itself.
          const nodeAfterFailure = this.nodes.get(nodeId);
          if (nodeAfterFailure) {
            this.notifySubscribers(nodeId, nodeAfterFailure, source);
          }

          // If this is an OCC error, hydrate from authoritative current_node and notify
          if (occError) {
            log.warn(
              `OCC conflict for task node ${nodeId}: ` +
                `expected v${occError.conflictData.expected}, got v${occError.conflictData.actual}`
            );
            // See the identical capture in updateNode()'s OCC handler above
            // for why this must be read BEFORE clearQueued() below erases it.
            const hadQueuedWrite = PersistenceCoordinator.getInstance().isQueued(nodeId);
            PersistenceCoordinator.getInstance().clearQueued(nodeId);

            // Normalized for the same reason as the generic update path above:
            // the conflict payload is a sync-boundary node and must get the
            // same typed-field promotion a broadcast would.
            const currentNode = occError.conflictData.current_node
              ? normalizeNodeData(occError.conflictData.current_node)
              : null;
            if (currentNode) {
              // This writes the conflict payload straight into the store,
              // same as a `database`-sourced broadcast, so it must respect
              // the same skip-while-editing guard `setNode()`/
              // `resyncNodeFromServer()` enforce (`decideRemoteUpdate`) —
              // the identical clobber class #2071 closed for `updateNode()`'s
              // sibling handler, applied here to the task-property path.
              //
              // `hasPending` is `hadQueuedWrite` (captured above, BEFORE
              // `clearQueued()` ran) rather than a live
              // `PersistenceCoordinator.hasPending()` read, for the same
              // reason `updateNode()`'s direct-hydration branch uses it: this
              // fires from inside the very write's own catch handler, before
              // its `executingOperations` entry has cleared, so a live
              // `hasPending()` read here would just see that same failing
              // write's own not-yet-cleared bookkeeping and treat it as "an
              // edit is pending" every time — self-referential and racy, not
              // a signal of a genuinely different in-flight write.
              const isFocused = focusManager.isNodeEditing(nodeId);
              const decision = decideRemoteUpdate(
                currentNode,
                this.nodes.get(nodeId),
                { type: 'database', reason: 'occ-resync' },
                { isFocused, hasPending: hadQueuedWrite }
              );

              if (decision.apply) {
                this.nodesSet(nodeId, currentNode);
                this.versions.set(nodeId, currentNode.version ?? 1);
                this.persistedNodeIds.add(nodeId);
                this.pendingUpdates.delete(nodeId);
                this.notifySubscribers(nodeId, currentNode, {
                  type: 'database',
                  reason: 'occ-resync'
                });
              } else {
                // Node is actively being edited — keep the local,
                // in-progress content and skip the clobber. The conflict
                // response fetch still proves the node exists server-side,
                // so mark it persisted the same way the apply branch would;
                // only the content/version overwrite is skipped. This call
                // site always raises its own conflict notification
                // unconditionally below regardless of branch, so no
                // separate notify here (unlike `resyncNodeFromServer`'s skip
                // branch, which has no such external caller for its
                // queued-follow-up case).
                this.persistedNodeIds.add(nodeId);
                log.debug(
                  `OCC direct hydration for task node ${nodeId} skipped — node is actively being edited (focused=${isFocused})`
                );
              }
            } else {
              this.resyncNodeFromServer(nodeId, false, hadQueuedWrite).catch((resyncError) => {
                log.error(`Failed to resync after OCC error for task node ${nodeId}:`, resyncError);
              });
            }

            conflictNotifications.add({
              nodeId,
              message: CONFLICT_MESSAGE['version-mismatch'],
              conflictType: 'version-mismatch'
            });
            occConflictAlreadyNotified = true;
          }

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
        // Operation was cancelled by a newer operation - this is expected
        return;
      }
      // An OCC error already raised its own specific version-mismatch
      // notification inside the persistence closure's own catch above (see
      // `occConflictAlreadyNotified`'s declaration for why this is a
      // captured flag rather than re-deriving it from `err` via
      // `isVersionConflict` — the re-thrown `err` has already lost the shape
      // that check needs).
      if (occConflictAlreadyNotified) return;
      // Surface non-OCC write failures visibly so users know their change
      // didn't save — matches updateNode()'s/deleteNode()'s/setNode()'s
      // outer catch, which all have this same fallback. Without it, a task
      // status/priority/due-date edit that fails outside an OCC conflict
      // reverted (or, in a later revision, stayed as the optimistic value) with zero
      // user-visible signal.
      conflictNotifications.add({
        nodeId,
        message: CONFLICT_MESSAGE['write-failure'],
        conflictType: 'write-failure'
      });
    });
  }

  /**
   * The current database generation (see `databaseEpoch`). A fetch-then-write
   * path captures this before awaiting the daemon; if it has advanced by the
   * time the response lands, the active database was switched underneath the
   * read and the result belongs to the previous database — the caller must drop
   * it rather than write it into the now-active store.
   */
  currentEpoch(): number {
    return this.databaseEpoch;
  }

  /**
   * Evict every cached node and its per-node metadata.
   *
   * Used both by tests and by the ADR-053 database hot-swap: switching the
   * active local database must never leave the previous database's nodes
   * visible. Clearing the reactive `nodes` map plus notifying subscribers makes
   * consumers re-derive against the now-empty store and reload from the
   * newly-active database. Component subscriptions themselves are preserved.
   *
   * Hot-swap callers must flush pending saves (`flushAllPendingSaves`) BEFORE
   * switching the routed clients so in-flight writes land in the database they
   * were made against, not the one being switched to.
   *
   * Bumps `databaseEpoch` so any read dispatched against the previous database
   * but still in flight is dropped rather than written into the now-active
   * store — see `currentEpoch()`.
   */
  clearAll(): void {
    this.databaseEpoch++;
    this.nodesClear();
    this.versions.clear();
    this.pendingUpdates.clear();
    this.taskFieldWriteSeq.clear();
    this.persistedNodeIds.clear();
    this.batchedNotifications.clear();
    this.activeBatches.clear();
    this.pendingTreeLoads.clear();
    this.resyncingNodes.clear();
    this.resyncQueued.clear();
    for (const timer of this.evictionTimers.values()) {
      clearTimeout(timer);
    }
    this.evictionTimers.clear();
    this.openDocumentRootIds.clear();
    this.hasOpenDocumentReport = false;
    this.notifyAllSubscribers();
  }

  // ========================================================================
  // New Methods for BaseNodeViewer Migration
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
      // databaseSource is reused for both the parent prefetch and the children below.
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };

      // ADR-053: capture the database generation before any daemon read so a
      // switch mid-flight is detectable below and the results are dropped
      // rather than written into the now-active database's store.
      const epoch = this.databaseEpoch;

      // Ensure the parent node itself is in the store before loading children.
      // This prevents BaseNodeViewer from treating a not-yet-loaded parent as a
      // stale/deleted node and closing the tab prematurely. Backlinks are a
      // separate resource — see mentions-and-references.md — fetched here
      // alongside the parent prefetch and children, all in parallel rather
      // than as sequential round-trips.
      const needsParentFetch = !this.nodes.has(parentId);
      const [parentNode, nodes, mentionedIn] = await Promise.all([
        needsParentFetch ? backendAdapter.getNode(parentId) : Promise.resolve(null),
        backendAdapter.getChildren(parentId),
        backendAdapter.getMentioningContainers(parentId)
      ]);

      // The active database switched while these reads were in flight — the
      // rows belong to the previous database, so apply none of them (writing
      // them, or their structureTree edges, would orphan the previous
      // database's nodes into the now-active store).
      if (this.databaseEpoch !== epoch) return [];

      if (parentNode) {
        this.setNode({ ...parentNode, mentionedIn }, databaseSource);
      } else {
        this.applyMentionedIn(parentId, mentionedIn);
      }

      // Add nodes to store with database source
      // Database source type will automatically mark nodes as persisted (see determinePersistenceBehavior)
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
        log.error(`Failed to load children for parent ${parentId}:`, error);
      }

      throw error;
    }
  }

  /**
   * Merge a freshly-fetched `mentionedIn` list onto an already-cached node and
   * notify subscribers. No-ops if the node isn't cached (nothing to merge onto).
   *
   * Backlinks are fetched independently of the node payload (see
   * `loadChildrenForParent`/`doLoadChildrenTree`/`refreshMentionedIn`), so every
   * caller that fetches them needs this same "merge onto whatever's cached, keep
   * everything else" step rather than overwriting the whole node.
   */
  private applyMentionedIn(nodeId: string, mentionedIn: NodeReference[]): void {
    const existing = this.nodes.get(nodeId);
    if (!existing) return;
    const updated = { ...existing, mentionedIn };
    this.nodesSet(nodeId, updated);
    this.notifySubscribers(nodeId, updated, { type: 'database', reason: 'mention-refresh' });
  }

  /**
   * Refetch backlinks (mentionedIn) for a single node and merge them into the
   * store. Called by the sync listener when a `mentions` relationship is
   * created/deleted so the panel updates without a reload — see
   * `tauri-sync-listener.ts`'s `mentions` branches.
   *
   * No-ops if the node isn't currently cached: nothing is displaying it, so
   * there's nothing to refresh (and no risk of missing the refresh later —
   * the node's next `loadChildrenForParent`/`loadChildrenTree` call fetches
   * backlinks fresh regardless).
   */
  async refreshMentionedIn(nodeId: string): Promise<void> {
    if (!this.nodes.has(nodeId)) return;
    const mentionedIn = await backendAdapter.getMentioningContainers(nodeId);
    this.applyMentionedIn(nodeId, mentionedIn);
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
      // ADR-053: capture the database generation before the daemon read so a
      // switch mid-flight is detectable below.
      const epoch = this.databaseEpoch;

      // Backlinks are a separate resource, fetched in parallel with the tree
      // rather than embedded in the node payload — see mentions-and-references.md.
      const [tree, mentionedIn] = await Promise.all([
        backendAdapter.getChildrenTree(parentId),
        backendAdapter.getMentioningContainers(parentId)
      ]);

      // The active database switched while this read was in flight — the tree
      // belongs to the previous database, so drop it rather than batch it (and
      // its structureTree edges) into the now-active store.
      if (this.databaseEpoch !== epoch) return [];

      if (!tree) {
        // Date nodes are virtual — they are created lazily in the backend when their first
        // child is saved. A brand-new date node that has never been persisted will return an
        // empty tree here. Synthesize a minimal in-memory node so BaseNodeViewer's
        // post-load existence check does not mistake it for a deleted/stale node and close
        // the tab.
        if (isValidDateId(parentId)) {
          const now = new Date().toISOString();
          const virtualDateNode: Node = {
            id: parentId,
            nodeType: 'date',
            content: '',
            version: 0, // 0 = placeholder; real version assigned by backend on first write
            createdAt: now,
            modifiedAt: now,
            properties: {}
          };
          // Use database source so determinePersistenceBehavior marks this as persisted and
          // does not trigger an unwanted write for this virtual placeholder.
          const virtualSource = { type: 'database' as const, reason: 'virtual-date-node' };
          this.setNode(virtualDateNode, virtualSource);
        }
        return [];
      }

      const allNodes: Node[] = [];
      const allRelationships: Array<{ parentId: string; childId: string; order: number }> = [];
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };

      // OPTIMIZATION: Add parent node itself to the store
      // This eliminates the need for a separate getNode() call in base-node-viewer
      // CRITICAL: Only add parent if not already in store to avoid overwriting pending
      // optimistic updates. If the parent was recently modified (e.g., slash command type
      // conversion), batchSetNodes with a database source would overwrite the in-memory
      // version with stale data, causing the subsequent persist to send wrong content.
      // Example: /customer slash command sets content='Untitled', but loadChildrenTree
      // immediately overwrites it with content='/customer' from the database before the
      // 500ms debounce persist fires.
      const { children: _children, ...parentNodeFields } = tree;
      const parentNode: Node = parentNodeFields as Node;
      if (!this.nodes.has(parentNode.id)) {
        allNodes.push({ ...parentNode, mentionedIn });
      } else {
        // Parent object itself is already cached (and kept as-is, per the
        // optimistic-update note above) — but mentionedIn must still refresh
        // here, since this is the only place it's fetched for this node.
        this.applyMentionedIn(parentNode.id, mentionedIn);
      }

      // Helper to recursively process NodeWithChildren and collect nodes + edges
      // OPTIMIZED: Collects all nodes first, then batch adds them
      const processNode = (
        nodeWithChildren: import('$lib/types').NodeWithChildren,
        nodeParentId: string,
        order: number
      ) => {
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
      // Always pass ALL relationships — batchAddRelationships / addChildInternal handles
      // deduplication internally: existing children get their order updated and re-sorted,
      // which corrects any stale optimistic order values (e.g. from empty nodes whose
      // relationship:created event fired before the backend persisted the correct order).
      if (allRelationships.length > 0) {
        structureTree.batchAddRelationships(allRelationships);
      }

      // Run invariant check after hydration completes (skipped in test environment
      // because the structureTree singleton accumulates state across tests and
      // produces false-positive orphan violations).
      if (!isTestEnvironment()) {
        const nodeIdSet = new Set(this.nodes.keys());
        // Allowlist __root__ sentinel plus any date nodes currently in the tree
        // that aren't in this.nodes (e.g. when loading a child of a date node
        // before the date node itself has been added to the store).
        const virtualIds = new Set<string>(['__root__']);
        for (const parentId of structureTree.children.keys()) {
          if (isValidDateId(parentId)) virtualIds.add(parentId);
        }
        structureTree.assertInvariants(nodeIdSet, virtualIds);
      }

      return allNodes;
    } catch (error) {
      // Suppress expected errors in in-memory test mode
      if (shouldLogDatabaseErrors()) {
        log.error(`Failed to load children tree for parent ${parentId}:`, error);
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
   * Check if a node has a persistence operation currently executing
   * @param nodeId - Node ID to check
   * @returns True if an operation is in-flight for this node
   */
  isNodePersistenceExecuting(nodeId: string): boolean {
    return PersistenceCoordinator.getInstance().isExecuting(nodeId);
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
   * NOTE: This only waits for already-executing operations. It does NOT trigger
   * debounced operations that haven't started yet. For that, use flushNodeSaves().
   *
   * @param nodeIds - Array of node IDs to wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async waitForNodeSaves(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    return PersistenceCoordinator.getInstance().waitForPersistence(nodeIds, timeoutMs);
  }

  /**
   * Flush specific pending node saves immediately and wait for completion.
   *
   * Unlike waitForNodeSaves which only waits for in-flight operations,
   * this method also TRIGGERS debounced operations that haven't started yet.
   *
   * Use this when you need to ensure specific nodes are fully persisted
   * before performing dependent operations (e.g., moveNode that references them).
   *
   * @param nodeIds - Array of node IDs to flush and wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async flushNodeSaves(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    return PersistenceCoordinator.getInstance().flushAndWaitForNodes(nodeIds, timeoutMs);
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

  // ========================================================================
  // Phase 3: External Update Handling (MCP-Ready)
  // ========================================================================

  /**
   * Handle updates from external sources (MCP server, database sync, etc.)
   *
   * This method provides the integration point for a future MCP server.
   * It routes external updates through the same conflict detection and
   * synchronization pipeline as local edits.
   *
   * @param source - Source type: 'mcp-server', 'database', or 'external'
   * @param update - The node update to apply
   *
   * @example
   * // Future: When the MCP server is ready
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
      log.warn(`External update for non-existent node: ${update.nodeId} from ${sourceType}`);
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
      skipPersistence: sourceType === 'database'
    });
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

    log.debug(`Update rolled back for node: ${nodeId}`);
  }

  /**
   * Resync a node from the server — used both after an OCC conflict and
   * after a non-OCC write failure (see the write-failure recovery path in
   * `updateNode()`).
   *
   * Implements a "server-wins" conflict resolution strategy:
   * - Fetches the current server state and replaces the local node entirely
   *   (unless the node is actively being edited — see the skip-while-editing
   *   guard below)
   * - User's pending edits are discarded in favor of server state
   * - This ensures the node is no longer stuck after a version conflict or a
   *   failed write
   *
   * Safe to call multiple times for the same node: only one fetch runs at a
   * time, but a call that arrives while one is already in flight is not
   * dropped — it queues exactly one follow-up, run once the in-flight fetch
   * settles, so a second failure's correction is never silently lost.
   *
   * Future enhancement: Implement conflict merge UI
   *
   * @param directCallHadQueuedWrite - DIRECT-call callers only (see the
   *   `hasPending` computation below for why this can't just be read live
   *   inside this method): whether `PersistenceCoordinator.isQueued(nodeId)`
   *   was true at the moment the caller detected its OCC conflict, captured
   *   BEFORE it called `clearQueued(nodeId)` — which unconditionally cancels
   *   and removes any queued write for this node (to stop a stale-version
   *   retry of the FAILING write itself), and so would otherwise erase the
   *   only evidence that a genuinely different second write was ever queued
   *   here. Ignored for the queued-follow-up call (`_isQueuedFollowUp: true`),
   *   which computes a live `hasPending()` read instead.
   */
  async resyncNodeFromServer(
    nodeId: string,
    _isQueuedFollowUp = false,
    directCallHadQueuedWrite = false
  ): Promise<void> {
    // Idempotency guard: prevent concurrent resync operations on same node.
    // A second caller while one is already in flight doesn't get dropped
    // outright, though — it queues exactly one follow-up (single-slot,
    // latest-wins, mirroring PersistenceCoordinator's own queued-write
    // pattern above). Without that follow-up, two failures landing close
    // together for the same node — e.g. two rapid Kanban drags, or a drag
    // plus a property edit, both failing during a short daemon outage —
    // would silently drop the second correction: the in-flight fetch can
    // easily have already been issued before the second failure's optimistic
    // write even landed locally, so it isn't guaranteed to reflect it.
    if (this.resyncingNodes.has(nodeId)) {
      log.debug(`Resync already in progress for node ${nodeId}, queuing a follow-up`);
      this.resyncQueued.add(nodeId);
      return;
    }

    this.resyncingNodes.add(nodeId);

    try {
      // ADR-053: capture the database generation before the daemon read.
      const epoch = this.databaseEpoch;
      // Snapshot by VALUE, not by reference: several other write-success
      // paths in this file (e.g. the `Object.assign(localNode, ...)` +
      // `nodesSet(nodeId, localNode)` pattern used to apply a confirmed
      // backend response) mutate the existing node object in place and then
      // re-set the *same* reference — `this.nodes.get(nodeId)` afterward is
      // `===` its pre-mutation self, so a reference check here would miss
      // exactly the case it exists to catch. Serializing sidesteps that
      // entirely: it only cares whether the data changed, never how.
      const nodeBeforeFetch = this.nodes.get(nodeId);
      const snapshotBeforeFetch = nodeBeforeFetch ? JSON.stringify(nodeBeforeFetch) : undefined;
      const serverNode = await backendAdapter.getNode(nodeId);

      // The active database switched while this resync was in flight — the
      // fetched row belongs to the previous database, so drop it.
      if (this.databaseEpoch !== epoch) return;

      if (serverNode) {
        // A second, unrelated write for this same node (e.g. a Kanban drag
        // to a different column, fired from another pane, anything) landed
        // locally while we were fetching. Our fetch reflects state from
        // *before* that write even happened, so applying it now would
        // silently discard that write's optimistic value — it might go on
        // to persist successfully moments later, which this resync has no
        // way to know. Bail and let that write's own success/failure path
        // (or its own resync, if it also fails) be the one to reconcile.
        const currentSnapshot = JSON.stringify(this.nodes.get(nodeId));
        if (currentSnapshot !== snapshotBeforeFetch) {
          log.debug(
            `resyncNodeFromServer: local node ${nodeId} changed while fetching — skipping to avoid clobbering a newer local write`
          );
          return;
        }
        // This writes the fetched row straight into the store, same as a
        // `database`-sourced broadcast — so it must respect the same
        // skip-while-editing guard `setNode()` enforces (`decideRemoteUpdate`).
        // Without this check, a resync racing an in-progress edit (e.g. a
        // debounced content persist that failed for an unrelated reason while
        // the user kept typing) would silently overwrite the optimistic,
        // actively-edited content with this now-stale server snapshot —
        // exactly the clobber `setNode()` was built to prevent, just reached
        // through a different write path.
        // `hasPending` is NOT a live `PersistenceCoordinator.hasPending(nodeId)`
        // read when this is the DIRECT call from inside a failing write's own
        // catch handler (`_isQueuedFollowUp` false) — confirmed empirically
        // (see the regression tests below): at this point in the DIRECT call,
        // `executingOperations` still has an entry for this node (the failing
        // write's own `runOperation` hasn't reached its `finally` yet), so a
        // live `hasPending()` read here is true almost every time regardless
        // of whether anything else is genuinely queued — self-referential and
        // racy, and it would defeat this recovery path for its single most
        // common case (an isolated failure with nothing else in flight).
        // `pendingOperations` is equally self-referential here for the same
        // reason (the failing write's own placeholder is still registered).
        //
        // `queuedOperations`, in contrast, is never populated by a write's own
        // bookkeeping — only by a genuinely different write collapsed behind
        // it while it was executing — which is exactly the signal this needs.
        // But by the time this method's decision point runs, the caller has
        // already called `clearQueued(nodeId)` (to stop a stale-version retry
        // of the FAILING write itself), which erases that evidence. So the
        // caller must capture `PersistenceCoordinator.isQueued(nodeId)` BEFORE
        // calling `clearQueued()` and pass it in as `directCallHadQueuedWrite`
        // — see that param's doc above.
        //
        // For the QUEUED follow-up, none of this applies — it fires from
        // resyncNodeFromServer's own `finally`, strictly after the direct
        // call's fetch (and so also after the triggering write's
        // `executingOperations` entry) has settled, so a live `hasPending()`
        // reading here reflects a genuinely different, still-in-flight write,
        // not this method's own residue — worth protecting for real.
        const isFocused = focusManager.isNodeEditing(nodeId);
        const hasPending = _isQueuedFollowUp
          ? PersistenceCoordinator.getInstance().hasPending(nodeId)
          : directCallHadQueuedWrite;
        const decision = decideRemoteUpdate(
          serverNode,
          nodeBeforeFetch,
          { type: 'database', reason: 'occ-resync' },
          { isFocused, hasPending }
        );
        if (!decision.apply) {
          // Still mark as persisted, same as `setNode()`'s equivalent skip
          // branch and for the same reason: a *successful* fetch of
          // `serverNode` is itself proof the node exists server-side,
          // independent of whether we go on to apply its content. Skipping
          // this would leave `persistedNodeIds` out of sync for a node
          // whose local bookkeeping had drifted (e.g. after a page reload
          // or database reset) — the exact case that bookkeeping exists to
          // self-correct — with nothing else positioned to fix it.
          this.persistedNodeIds.add(nodeId);
          log.debug(
            `resyncNodeFromServer: skipping clobber of actively-edited node ${nodeId} (focused=${isFocused}, pending=${hasPending})`
          );
          // Mirrors `setNode()`'s identical-shaped guard: a skip caused by a
          // genuinely newer incoming version is a real foreign-write signal
          // and must not be silent. Deduped per node so a caller that
          // already raised its own conflict notification for this same
          // event (every current OCC call site does, unconditionally, right
          // after invoking this method) doesn't produce a second toast — but
          // the queued follow-up, which has no such external caller, still
          // gets one.
          if (decision.notifyConflict) {
            const alreadyFlagged = conflictNotifications.notifications.some(
              (n) => n.nodeId === nodeId && n.conflictType === 'version-mismatch'
            );
            if (!alreadyFlagged) {
              conflictNotifications.add({
                nodeId,
                message: CONFLICT_MESSAGE['version-mismatch'],
                conflictType: 'version-mismatch'
              });
            }
          }
          return;
        }

        // Replace in-memory node with server state
        this.nodesSet(nodeId, serverNode);

        // Sync version to match server
        this.versions.set(nodeId, serverNode.version ?? 1);

        // Mark as persisted since we just fetched from server
        this.persistedNodeIds.add(nodeId);

        // Clear any pending updates for this node
        this.pendingUpdates.delete(nodeId);

        // Notify subscribers with server state
        this.notifySubscribers(nodeId, serverNode, {
          type: 'database',
          reason: 'occ-resync'
        });

        log.warn(
          `Node ${nodeId} resynced from server after OCC error ` +
            `(server version: ${serverNode.version ?? 1})`
        );
      } else {
        log.error(`Failed to resync node ${nodeId}: Node not found on server`);
      }
    } catch (error) {
      log.error(`Failed to resync node ${nodeId} from server:`, error);
      throw error;
    } finally {
      // Always clean up tracking set, even on error
      this.resyncingNodes.delete(nodeId);
      // A second caller arrived while this fetch was in flight and queued a
      // follow-up (see the idempotency guard above) — run one more resync so
      // its correction isn't dropped. Fire-and-forget: this method's own
      // caller is only waiting on THIS resync, not a chain of them.
      if (this.resyncQueued.delete(nodeId)) {
        void this.resyncNodeFromServer(nodeId, true).catch((followUpError) => {
          log.error(`Follow-up resync failed for node ${nodeId}:`, followUpError);
        });
      }
    }
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
          log.error(`Subscription callback error:`, error);
        }
      }
    }

    // Notify wildcard subscribers
    for (const sub of this.wildcardSubscriptions) {
      try {
        sub.callback(node, source);
        sub.callCount++;
      } catch (error) {
        log.error(`Wildcard subscription callback error:`, error);
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
            log.error(`Subscription callback error:`, error);
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
      rollbackCount: 0
    };
  }

  /**
   * Record operation timing
   */
  private recordMetric(duration: number): void {
    // Call this ONLY for a call that incremented `updateCount`, and exactly
    // once per increment. The incremental mean below is valid only when the
    // count already includes the sample being folded in; called without that,
    // the sample displaces the mean instead of extending it (and divides by
    // zero on the very first such call).
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
      log.warn(' Auto-committing batch after inactivity timeout', {
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
      log.warn(' Attempted to add to non-existent batch', {
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
      this.nodesSet(nodeId, updatedNode);

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
        log.warn(' Batch commit aborted - node not found', {
          nodeId,
          batchId: batch.batchId
        });
        return;
      }

      // Nothing to persist if batch has no changes
      if (Object.keys(batch.changes).length === 0) {
        return;
      }

      // Always persist batched changes - even blank/syntax-only nodes
      // Real nodes (created by user actions) should always be persisted
      // The viewer-local placeholder never enters batch system
      this.persistBatchedChanges(nodeId, batch.changes, finalNode);
    } catch (error) {
      log.error(' Batch commit error', {
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
    log.debug(`Committing all batches: ${nodeIds.length} active`);
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
    const handle = PersistenceCoordinator.getInstance().persist(
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
            // CRITICAL: Wait for any pending move operation to complete before UPDATE.
            // Move operations (indent/outdent) increment the version in the backend.
            // If we UPDATE before the move completes, we'll have a version mismatch.
            let currentNode = this.nodes.get(nodeId);
            const pendingMove = getPendingMoveOperation(nodeId);
            if (pendingMove) {
              log.debug(
                `[BATCH UPDATE] Waiting for pending move operation on ${nodeId.substring(0, 8)}`
              );
              await pendingMove;
              // Re-read current node to get updated version after move
              const refreshedNode = this.nodes.get(nodeId);
              if (refreshedNode) {
                currentNode = refreshedNode;
              }
            }

            // Get current version for optimistic concurrency control
            const currentVersion = currentNode?.version ?? finalNode.version ?? 1;

            // CRITICAL: Capture updated node to get new version from backend
            // This prevents version conflicts on subsequent updates
            const updatedNodeFromBackend = await backendAdapter.updateNode(
              nodeId,
              currentVersion,
              changes
            );

            // Update local node with backend version
            const localNode = this.nodes.get(nodeId);
            if (localNode && updatedNodeFromBackend) {
              localNode.version = updatedNodeFromBackend.version;
              this.nodesSet(nodeId, localNode);
            }
          } else {
            // Try CREATE, but handle race condition where old path persisted first
            try {
              const batchCreateInput: import('$lib/services/backend-adapter').CreateNodeInput = {
                id: finalNode.id,
                nodeType: finalNode.nodeType,
                content: finalNode.content,
                properties: finalNode.properties,
                mentions: finalNode.mentions,
                parentId: this.getParentId(nodeId),
                insertPosition: null
              };
              await backendAdapter.createNode(batchCreateInput);
              this.persistedNodeIds.add(nodeId);

              // CRITICAL: Fetch the created node to get its version from backend
              // This prevents version conflicts on subsequent updates
              const createdNode = await backendAdapter.getNode(nodeId);
              if (createdNode) {
                // BUG FIX: Only update the VERSION, not the entire node!
                // The user may have continued typing while createNode was in flight.
                // We must preserve their local changes and only take the version from backend.
                const latestLocalNode = this.nodes.get(nodeId);
                if (latestLocalNode) {
                  latestLocalNode.version = createdNode.version;
                  this.nodesSet(nodeId, latestLocalNode);
                }
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
                // First check for pending move operations
                let raceCurrentNode = this.nodes.get(nodeId);
                const raceMove = getPendingMoveOperation(nodeId);
                if (raceMove) {
                  log.debug(
                    `[BATCH RACE] Waiting for pending move operation on ${nodeId.substring(0, 8)}`
                  );
                  await raceMove;
                  const refreshed = this.nodes.get(nodeId);
                  if (refreshed) {
                    raceCurrentNode = refreshed;
                  }
                }
                const currentVersion = raceCurrentNode?.version ?? finalNode.version ?? 1;
                const updatedNodeFromBackend = await backendAdapter.updateNode(
                  nodeId,
                  currentVersion,
                  changes
                );
                this.persistedNodeIds.add(nodeId);

                // Update local node with backend version
                const localNode = this.nodes.get(nodeId);
                if (localNode && updatedNodeFromBackend) {
                  localNode.version = updatedNodeFromBackend.version;
                  this.nodesSet(nodeId, localNode);
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
            log.error(`Batch persistence failed for node ${nodeId}:`, error);
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

    // Handle cancellation errors (expected when operations are superseded) —
    // matches the other persist() call sites. Without this, a superseded
    // batch write's rejection (e.g. via cancelPending() when a re-batch
    // supersedes an in-flight batch operation) becomes an unhandled promise
    // rejection instead of being tolerated like elsewhere in this file.
    //
    // Unlike updateNode()/updateTaskNodeStatus(), the operation closure above
    // has no OCC-specific handling (no rollback, no resync, no notification)
    // — it only logs and re-throws. So a VERSION_CONFLICT here must NOT be
    // silently swallowed the way it is at those other call sites (where it's
    // already been handled internally): every non-cancellation failure,
    // including OCC, falls through to the write-failure notification below.
    handle.promise.catch((err) => {
      if (err instanceof OperationCancelledError) {
        // Operation was cancelled by a newer operation - this is expected
        return;
      }
      // Surface write failures visibly so users know their change didn't save
      conflictNotifications.add({
        nodeId,
        message: CONFLICT_MESSAGE['write-failure'],
        conflictType: 'write-failure'
      });
    });
  }

  // ========================================================================
  // Snapshot/Restore for Optimistic Rollback
  // ========================================================================

  /**
   * Take a snapshot of all nodes for optimistic rollback
   *
   * Creates a deep copy of the current node state that can be restored
   * if a backend operation fails.
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
   *
   * @param snapshotMap - Previously captured snapshot to restore
   */
  restore(snapshotMap: Map<string, Node>): void {
    // Clear current nodes and restore from snapshot. Routed through
    // nodesClear()/nodesSet() (not a raw this.nodes.clear()/.set()) so
    // nodeGeneration stays in sync with what's actually in `nodes` — a
    // restored node is stamped fresh-as-of-now, same as any other write.
    this.nodesClear();
    for (const [nodeId, node] of snapshotMap) {
      this.nodesSet(nodeId, node);
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
    this.nodesClear();
    this.persistedNodeIds.clear();
    this.subscriptions.clear();
    this.wildcardSubscriptions.clear();
    this.pendingUpdates.clear();
    this.versions.clear();
    this.reconnectGeneration = 0;
    this.testErrors = [];

    // Cancel all active batches
    for (const [nodeId] of this.activeBatches) {
      this.cancelBatch(nodeId);
    }
    this.activeBatches.clear();

    for (const timer of this.evictionTimers.values()) {
      clearTimeout(timer);
    }
    this.evictionTimers.clear();
    this.openDocumentRootIds.clear();
    this.hasOpenDocumentReport = false;
    this.evictionInactivityMs = 30_000;

    this.metrics = {
      updateCount: 0,
      avgUpdateTime: 0,
      maxUpdateTime: 0,
      subscriptionCount: 0,
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

// A daemon reconnect (crash/restart, or a wedged-channel recovery — see
// daemon-status.ts) means the WatchNodes bridge opened a fresh stream with no
// catch-up replay: any node updates that happened during the outage never
// reached the store. Mark the whole cache possibly-stale so the next time
// each node is hydrated (mount, or navigating back to it), it is re-confirmed
// against the backend instead of trusting a cache entry that predates the
// gap. Mirrors the same reload-on-reconnect idiom `schemasStore` and
// `collectionsData` already use, but lazily per-node instead of eagerly
// reloading everything.
onDaemonReconnect(() => sharedNodeStore.markPossiblyStaleAfterReconnect());

/**
 * Default export
 */
export default SharedNodeStore;
