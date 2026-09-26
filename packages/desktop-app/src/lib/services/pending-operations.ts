/**
 * Pending Operations Tracker
 *
 * Tracks pending move operations to prevent race conditions between
 * indent/outdent and content updates. This is in a separate module
 * to avoid circular dependencies between SharedNodeStore and ReactiveNodeService.
 */

import { createLogger } from '$lib/utils/logger';

const log = createLogger('PendingOps');

/**
 * Handed to a move's body by `trackMoveOperation()`. A move flushes the moved
 * node's pending writes before sending, so it calls `coverWritesThrough()`
 * with the persistence sequence (`SharedNodeStore.persistenceSequence()`)
 * immediately before that flush: every write registered up to then is one the
 * move waits on, and must therefore not wait on the move.
 */
export interface MoveTicket {
  coverWritesThrough(sequence: number): void;
}

interface TrackedMove {
  promise: Promise<void>;
  /** Last persistence sequence this move's flush waits on; null until it flushes. */
  coversWritesThrough: number | null;
}

// Track pending move operations to prevent race conditions
// When indent/outdent fires a moveNodeCommand, subsequent operations must wait for it
const pendingMoves = new Map<string, Set<TrackedMove>>();

/**
 * Wait for all pending move operations to complete before starting a new hierarchy change.
 * This prevents "Sibling not found" errors during rapid Enter+Tab+Shift+Tab sequences.
 *
 * Race condition scenario:
 * 1. User indents B under A (fires moveNodeCommand async)
 * 2. User immediately outdents C (which inserts After B via InsertPosition)
 * 3. If step 1's moveNodeCommand hasn't completed, edge A→B doesn't exist yet
 * 4. Backend fails with "Sibling not found: B" because B isn't in A's has_child edges
 */
export async function waitForPendingMoveOperations(): Promise<void> {
  if (pendingMoves.size === 0) return;
  const promises = [...pendingMoves.values()].flatMap((moves) =>
    [...moves].map((move) => move.promise)
  );
  await Promise.all(promises);
}

/**
 * Run a move operation for `nodeId` and track it until it settles.
 *
 * `run` starts synchronously, before the move is registered, so a
 * `waitForPendingMoveOperations()` at its start waits only on earlier moves.
 */
export function trackMoveOperation(
  nodeId: string,
  run: (ticket: MoveTicket) => Promise<void>
): Promise<void> {
  log.debug(`Tracking move for ${nodeId.substring(0, 8)}`);
  const move: TrackedMove = { promise: Promise.resolve(), coversWritesThrough: null };
  const operation = run({
    coverWritesThrough: (sequence) => {
      move.coversWritesThrough = sequence;
    }
  });
  move.promise = operation.finally(() => {
    log.debug(`Move completed for ${nodeId.substring(0, 8)}`);
    const moves = pendingMoves.get(nodeId);
    moves?.delete(move);
    if (moves?.size === 0) pendingMoves.delete(nodeId);
  });
  const moves = pendingMoves.get(nodeId) ?? new Set<TrackedMove>();
  moves.add(move);
  pendingMoves.set(nodeId, moves);
  return move.promise;
}

/**
 * The pending moves of `nodeId` that the write with persistence sequence
 * `writeSequence` must wait for, or undefined when there are none.
 *
 * A move bumps the node's version server-side, so a write sent before it lands
 * would conflict on a stale version. But a move also waits for the node's
 * pending writes before it sends, and a write that waited on such a move would
 * never finish. So a write waits only for moves whose flush does not cover it:
 * moves that have flushed, and flushed before the write was registered. A move
 * that has not flushed yet will wait for the write instead.
 */
export function movesAheadOfWrite(
  nodeId: string,
  writeSequence: number
): Promise<void> | undefined {
  const moves = pendingMoves.get(nodeId);
  if (!moves) return undefined;
  const ahead = [...moves].filter(
    (move) => move.coversWritesThrough !== null && move.coversWritesThrough < writeSequence
  );
  log.debug(
    `movesAheadOfWrite(${nodeId.substring(0, 8)}, #${writeSequence}): ${ahead.length} of ${moves.size}`
  );
  if (ahead.length === 0) return undefined;
  return Promise.all(ahead.map((move) => move.promise.catch(() => {}))).then(() => {});
}
