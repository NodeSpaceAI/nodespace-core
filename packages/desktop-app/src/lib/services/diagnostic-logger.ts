/**
 * Diagnostic Logger for Backend Calls
 *
 * This module provides comprehensive logging of all backend calls to help debug
 * persistence issues on machines where nodes don't persist.
 *
 * Features:
 * - Logs all backend method calls with full arguments
 * - Logs responses (success) and errors (failure)
 * - Maintains an in-memory log buffer for the diagnostic panel
 * - Works in both Tauri desktop mode and browser dev mode
 */

import { createLogger } from '$lib/utils/logger';
import { debugChannelWrite } from '$lib/services/debug-channel';
import { toError } from '$lib/types/errors';

const log = createLogger('DiagnosticLogger');

// ============================================================================
// Types
// ============================================================================

export interface DiagnosticLogEntry {
  id: string;
  timestamp: string;
  method: string;
  args: unknown[];
  result?: unknown;
  error?: string;
  durationMs: number;
  status: 'pending' | 'success' | 'error';
}

export interface DiagnosticStats {
  totalCalls: number;
  successCalls: number;
  errorCalls: number;
  avgDurationMs: number;
  methodCounts: Record<string, number>;
}

// ============================================================================
// Diagnostic Log Storage
// ============================================================================

const MAX_LOG_ENTRIES = 500;
let logEntries: DiagnosticLogEntry[] = [];
let isEnabled = true;

/**
 * Generate a unique ID for log entries
 */
function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

/**
 * Add a new log entry
 */
function addLogEntry(entry: DiagnosticLogEntry): void {
  logEntries.push(entry);

  // Trim old entries if we exceed the max
  if (logEntries.length > MAX_LOG_ENTRIES) {
    logEntries = logEntries.slice(-MAX_LOG_ENTRIES);
  }
}

/**
 * Update an existing log entry (for when call completes)
 */
function updateLogEntry(id: string, updates: Partial<DiagnosticLogEntry>): void {
  const entry = logEntries.find(e => e.id === id);
  if (entry) {
    Object.assign(entry, updates);
  }
}

// ============================================================================
// Public API
// ============================================================================

/**
 * Enable or disable diagnostic logging
 */
export function setDiagnosticLoggingEnabled(enabled: boolean): void {
  isEnabled = enabled;
  log.info(`Diagnostic logging ${enabled ? 'enabled' : 'disabled'}`);
}

/**
 * Check if diagnostic logging is enabled
 */
export function isDiagnosticLoggingEnabled(): boolean {
  return isEnabled;
}

/**
 * Get all log entries
 */
export function getLogEntries(): DiagnosticLogEntry[] {
  return [...logEntries];
}

/**
 * Get recent log entries (last N)
 */
export function getRecentLogEntries(count: number = 50): DiagnosticLogEntry[] {
  return logEntries.slice(-count);
}

/**
 * Clear all log entries
 */
export function clearLogEntries(): void {
  logEntries = [];
  log.info('Diagnostic log entries cleared');
}

/**
 * Get diagnostic statistics
 */
export function getDiagnosticStats(): DiagnosticStats {
  const methodCounts: Record<string, number> = {};
  let totalDuration = 0;
  let successCount = 0;
  let errorCount = 0;

  for (const entry of logEntries) {
    methodCounts[entry.method] = (methodCounts[entry.method] || 0) + 1;
    totalDuration += entry.durationMs;
    if (entry.status === 'success') successCount++;
    if (entry.status === 'error') errorCount++;
  }

  return {
    totalCalls: logEntries.length,
    successCalls: successCount,
    errorCalls: errorCount,
    avgDurationMs: logEntries.length > 0 ? totalDuration / logEntries.length : 0,
    methodCounts
  };
}

/**
 * Export logs as JSON for sharing/debugging
 */
export function exportLogsAsJson(): string {
  return JSON.stringify({
    exportedAt: new Date().toISOString(),
    stats: getDiagnosticStats(),
    entries: logEntries
  }, null, 2);
}

// ============================================================================
// Logging Wrapper
// ============================================================================

/**
 * Wrap an async function with diagnostic logging
 *
 * @param methodName - Name of the method being called
 * @param fn - The async function to wrap
 * @param args - Arguments passed to the function
 * @returns The result of the function
 */
export async function withDiagnosticLogging<T>(
  methodName: string,
  fn: () => Promise<T>,
  args: unknown[]
): Promise<T> {
  if (!isEnabled) {
    return fn();
  }

  const entryId = generateId();
  const startTime = performance.now();
  const timestamp = new Date().toISOString();

  // Log start
  log.debug(`[CALL] ${methodName}`, { args: truncateArgs(args) });

  // Add pending entry
  addLogEntry({
    id: entryId,
    timestamp,
    method: methodName,
    args: truncateArgs(args),
    durationMs: 0,
    status: 'pending'
  });

  try {
    const result = await fn();
    const durationMs = performance.now() - startTime;

    // Update entry with success
    updateLogEntry(entryId, {
      result: truncateResult(result),
      durationMs,
      status: 'success'
    });

    log.debug(`[SUCCESS] ${methodName} (${durationMs.toFixed(2)}ms)`, {
      result: truncateResult(result)
    });

    debugChannelWrite({
      kind: 'invoke',
      timestamp: new Date().toISOString(),
      method: methodName,
      args: truncateArgs(args),
      durationMs,
      status: 'success',
      result: truncateResult(result)
    });

    return result;
  } catch (error) {
    const durationMs = performance.now() - startTime;
    const errorMessage = toError(error).message;

    // Update entry with error
    updateLogEntry(entryId, {
      error: errorMessage,
      durationMs,
      status: 'error'
    });

    log.error(`[ERROR] ${methodName} (${durationMs.toFixed(2)}ms)`, {
      error: errorMessage,
      args: truncateArgs(args)
    });

    debugChannelWrite({
      kind: 'invoke',
      timestamp: new Date().toISOString(),
      method: methodName,
      args: truncateArgs(args),
      durationMs,
      status: 'error',
      error: errorMessage
    });

    throw error;
  }
}

/**
 * Truncate arguments for logging (avoid huge objects)
 */
function truncateArgs(args: unknown[]): unknown[] {
  return args.map(arg => {
    if (arg === null || arg === undefined) return arg;
    if (typeof arg === 'string') return arg.length > 200 ? arg.substring(0, 200) + '...' : arg;
    if (typeof arg === 'object') {
      const str = JSON.stringify(arg);
      if (str.length > 500) {
        return `[Object truncated: ${str.substring(0, 200)}...]`;
      }
      return arg;
    }
    return arg;
  });
}

/**
 * Truncate result for logging
 */
function truncateResult(result: unknown): unknown {
  if (result === null || result === undefined) return result;
  if (typeof result === 'string') return result.length > 200 ? result.substring(0, 200) + '...' : result;
  if (Array.isArray(result)) {
    if (result.length > 10) {
      return `[Array with ${result.length} items]`;
    }
    return result;
  }
  if (typeof result === 'object') {
    const str = JSON.stringify(result);
    if (str.length > 500) {
      return `[Object: ${str.substring(0, 100)}...]`;
    }
    return result;
  }
  return result;
}

