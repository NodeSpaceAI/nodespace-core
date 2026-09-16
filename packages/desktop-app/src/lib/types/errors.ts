/**
 * Error Types for NodeSpace
 *
 * Structured error handling for Tauri commands and service operations.
 */

/**
 * Structured error from Tauri commands
 *
 * Provides better observability and debugging by including error codes
 * and optional details alongside user-facing messages.
 */
export interface CommandError {
  /** User-facing error message */
  message: string;

  /** Machine-readable error code (e.g., "NODE_SERVICE_ERROR", "INVALID_NODE_TYPE") */
  code?: string;

  /** Optional detailed error information for debugging */
  details?: string;
}

/**
 * Type guard to check if an error is a CommandError
 */
export function isCommandError(error: unknown): error is CommandError {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  return typeof err.message === 'string';
}

/**
 * Convert unknown error to user-friendly Error instance
 */
export function toError(error: unknown): Error {
  if (error instanceof Error) {
    return error;
  }

  if (isCommandError(error)) {
    return new Error(error.message);
  }

  if (typeof error === 'string') {
    return new Error(error);
  }

  return new Error('Unknown error occurred');
}

/**
 * Database initialization errors
 */
export class DatabaseInitializationError extends Error {
  constructor(
    message: string,
    public readonly details?: string
  ) {
    super(message);
    this.name = 'DatabaseInitializationError';
  }
}

/**
 * Node operation errors
 */
export class NodeOperationError extends Error {
  constructor(
    message: string,
    public readonly nodeId?: string,
    public readonly operation?: string
  ) {
    super(message);
    this.name = 'NodeOperationError';
  }
}

/**
 * Structured conflict payload carried by VERSION_CONFLICT CommandErrors.
 * Mirrors the JSON emitted by the daemon's x-version-conflict metadata header.
 */
export interface VersionConflictData {
  /** Node ID that had the conflict */
  node_id: string;

  /** Version the client expected */
  expected: number;

  /** Actual current version in database */
  actual: number;

  /** Full current node state from database for client-side hydration */
  current_node: import('./node').Node | null;
}

/**
 * A CommandError that carries a structured version-conflict payload.
 * Produced by the daemon → Tauri command → frontend pipeline when two
 * writers race on the same node version.
 */
export interface VersionConflictCommandError extends CommandError {
  code: 'VERSION_CONFLICT';
  conflictData: VersionConflictData;
}

/**
 * Type guard: returns true when the thrown value is a VERSION_CONFLICT
 * CommandError carrying the daemon's structured conflict payload.
 *
 * Matches the gRPC/Tauri shape: { code: "VERSION_CONFLICT", conflictData: {...} }
 */
export function isVersionConflict(
  error: unknown
): error is VersionConflictCommandError {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  if (err.code !== 'VERSION_CONFLICT') return false;
  if (typeof err.conflictData !== 'object' || err.conflictData === null) return false;

  const cd = err.conflictData as Record<string, unknown>;
  return typeof cd.node_id === 'string';
}

/**
 * Structured payload carried by SUBTREE_ACCESS_DENIED CommandErrors.
 * Mirrors the JSON derived from the daemon's x-subtree-inaccessible-count metadata
 * header when a cascade delete is refused by the ADR-041 subtree access gate.
 */
export interface SubtreeAccessDeniedData {
  /** Minimum number of nodes in the delete the actor cannot read. */
  inaccessibleCount: number;
}

/**
 * A CommandError that carries a structured subtree-access-denied payload.
 * Produced by the daemon → Tauri command → frontend pipeline when a cascade
 * delete is refused because the subtree contains nodes the actor cannot read.
 */
export interface SubtreeAccessDeniedCommandError extends CommandError {
  code: 'SUBTREE_ACCESS_DENIED';
  conflictData: SubtreeAccessDeniedData;
}

/**
 * Type guard: returns true when the thrown value is a SUBTREE_ACCESS_DENIED
 * CommandError carrying the daemon's structured refusal payload.
 *
 * Matches the gRPC/Tauri shape: { code: "SUBTREE_ACCESS_DENIED", conflictData: { inaccessibleCount } }
 */
export function isSubtreeAccessDenied(
  error: unknown
): error is SubtreeAccessDeniedCommandError {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  if (err.code !== 'SUBTREE_ACCESS_DENIED') return false;
  if (typeof err.conflictData !== 'object' || err.conflictData === null) return false;

  const cd = err.conflictData as Record<string, unknown>;
  return typeof cd.inaccessibleCount === 'number';
}

/**
 * Structured payload carried by PLAY_RULE_REJECTED CommandErrors.
 * Mirrors the JSON emitted by the daemon's x-play-rule-rejected metadata
 * header when a `RuleClass::Invariant` rule's `reject` action (ADR-060 §2)
 * vetoes a write.
 */
export interface PlayRuleRejectedData {
  /** Node ID the write targeted */
  node_id: string;

  /** ID of the play whose rule rejected the write */
  play_id: string;

  /** Name of the specific rule that rejected the write */
  rule_id: string;

  /** The rejecting rule's own author-supplied violation text */
  message: string;
}

/**
 * A CommandError that carries a structured play-rule-rejection payload.
 * Produced by the daemon → Tauri command → frontend pipeline when a
 * synchronous invariant rule's `reject` action vetoes a `create_node` or
 * `update_node` call — structurally the same category of outcome as a
 * VERSION_CONFLICT (the write did not take effect), just discovered inside
 * invariant-rule evaluation rather than the OCC check.
 */
export interface PlayRuleRejectedCommandError extends CommandError {
  code: 'PLAY_RULE_REJECTED';
  conflictData: PlayRuleRejectedData;
}

/**
 * Type guard: returns true when the thrown value is a PLAY_RULE_REJECTED
 * CommandError carrying the daemon's structured rejection payload.
 *
 * Matches the gRPC/Tauri shape: { code: "PLAY_RULE_REJECTED", conflictData: { node_id, play_id, rule_id, message } }
 */
export function isPlayRuleRejected(error: unknown): error is PlayRuleRejectedCommandError {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  if (err.code !== 'PLAY_RULE_REJECTED') return false;
  if (typeof err.conflictData !== 'object' || err.conflictData === null) return false;

  const cd = err.conflictData as Record<string, unknown>;
  return typeof cd.message === 'string';
}
