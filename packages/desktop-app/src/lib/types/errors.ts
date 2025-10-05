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
