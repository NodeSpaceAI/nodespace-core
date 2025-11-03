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
 * MCP Version Conflict Error Data
 * Matches the error data structure from Rust MCP layer (packages/core/src/mcp/types.rs)
 */
export interface VersionConflictData {
  /** Error type identifier */
  type: 'VersionConflict';

  /** Node ID that had the conflict */
  node_id: string;

  /** Version the client expected */
  expected_version: number;

  /** Actual current version in database */
  actual_version: number;

  /** Full current node state from database for client-side merge */
  current_node: import('./node').Node;
}

/**
 * MCP Error Response Format
 * Matches JSON-RPC 2.0 error format with custom version conflict code
 */
export interface MCPError {
  /** JSON-RPC error code (-32005 for version conflicts) */
  code: number;

  /** Human-readable error message */
  message: string;

  /** Optional structured error data (contains VersionConflictData for conflicts) */
  data?: VersionConflictData | unknown;
}

/**
 * Version conflict error code constant
 */
export const VERSION_CONFLICT_CODE = -32005;

/**
 * Type guard to check if an MCP error is a version conflict
 */
export function isVersionConflict(
  error: unknown
): error is MCPError & { data: VersionConflictData } {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  return (
    err.code === VERSION_CONFLICT_CODE &&
    typeof err.data === 'object' &&
    err.data !== null &&
    (err.data as Record<string, unknown>).type === 'VersionConflict'
  );
}
