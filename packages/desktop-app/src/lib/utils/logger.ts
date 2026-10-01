/**
 * Test-aware logger utility
 * Automatically silences logs during test execution to improve performance
 *
 * Usage:
 *   import { createLogger, logger } from '$lib/utils/logger';
 *
 *   // Use default logger
 *   logger.debug('Debug message', { data });
 *   logger.info('Info message');
 *   logger.warn('Warning message');
 *   logger.error('Error message', error);
 *
 *   // Create service-specific logger with prefix
 *   const log = createLogger('MyService');
 *   log.debug('Starting operation'); // [MyService] Starting operation
 *
 * Log Levels (in order of severity):
 *   - debug: Detailed debugging info (hidden in production by default)
 *   - info: Operational information (hidden in production by default)
 *   - warn: Warnings that don't block operation
 *   - error: Actual errors
 *
 * Environment Behavior:
 *   - Production: Only warn and error logs shown
 *   - Development: All logs shown (debug level)
 *   - Test: All logs disabled (unless explicitly enabled)
 */

import { debugChannelWrite, isChannelEnabledSync } from '$lib/services/debug-channel';
import { isVitest as isTest } from '$lib/utils/is-vitest';

// Define log levels as const array for type safety and single source of truth
const LOG_LEVELS = ['debug', 'info', 'warn', 'error'] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

interface LoggerConfig {
  enabled: boolean;
  level: LogLevel;
  prefix?: string;
}

const isProd =
  (typeof import.meta !== 'undefined' && import.meta.env?.PROD === true) ||
  (typeof process !== 'undefined' && process.env?.NODE_ENV === 'production');

// Read a localStorage key defensively. Some runtimes (e.g. Node 25's built-in
// localStorage) expose a partial Storage object where `getItem` is not a function,
// so a bare `typeof localStorage !== 'undefined'` guard is insufficient.
function readStorage(key: string): string | null {
  if (typeof localStorage === 'undefined' || typeof localStorage.getItem !== 'function') {
    return null;
  }
  return localStorage.getItem(key);
}

// Default log level based on environment
// Production: only warn/error
// Development: warn/error by default; set localStorage.debug='*' to enable debug
const _storedLevel = readStorage('nodespace:logLevel');
const DEFAULT_LEVEL: LogLevel = isProd
  ? 'warn'
  : (_storedLevel as LogLevel | null) ?? (readStorage('nodespace:debug') ? 'debug' : 'warn');

/**
 * Logger class with environment-aware configuration.
 *
 * For typical usage, prefer the `createLogger()` factory function which
 * provides sensible defaults with an optional service name prefix.
 *
 * Direct instantiation is available for advanced configuration or testing
 * scenarios where you need explicit control over enabled state, log level,
 * or other settings.
 *
 * @example
 * // Recommended: Use factory function
 * const log = createLogger('MyService');
 * log.info('Hello world');
 *
 * @example
 * // Advanced: Direct instantiation for testing
 * const log = new Logger({ enabled: true, level: 'debug', prefix: 'Test' });
 */
// ── Frontend-console capture (diagnostic) ──────────────────────────────────────
// When `NS_FRONTEND_LOG` is set (a file path), every console-level log is also
// emitted as a `console` DebugEvent on the structured NDJSON debug channel (see
// debug-channel.ts). A long-term aid for diagnosing cross-window / event-stream
// behaviour in the real GUI (where the Svelte console isn't otherwise
// capturable headlessly), and for AI agents inspecting the packaged WKWebView.
function forwardToFile(level: LogLevel, message: string, data?: unknown): void {
  debugChannelWrite({ kind: 'console', timestamp: new Date().toISOString(), level, message, data });
}

// Probe capture-enabled at module load so debug logging activates promptly when
// NS_FRONTEND_LOG is set (before the first cross-window sync events arrive).
forwardToFile('info', '[logger] frontend-console capture initialised');

// Global error capture. Thrown exceptions — a Svelte runtime error (e.g. a keyed
// `{#each}` duplicate-key crash), a synchronous throw, or an unhandled promise
// rejection — never pass through the logger methods, so without this they land
// only in the webview devtools console and are invisible to file-based
// diagnosis. Forward them to the same NDJSON channel. Gated on the channel being
// enabled (NS_FRONTEND_LOG) inside `forwardToFile`, so normal builds pay nothing.
if (typeof window !== 'undefined' && !isTest) {
  window.addEventListener('error', (e) => {
    const err = e.error as Error | undefined;
    forwardToFile('error', `[uncaught] ${e.message}`, {
      filename: e.filename,
      lineno: e.lineno,
      colno: e.colno,
      stack: err?.stack
    });
  });
  window.addEventListener('unhandledrejection', (e) => {
    const reason = e.reason as { message?: string; stack?: string } | undefined;
    forwardToFile('error', `[unhandledrejection] ${reason?.message ?? String(e.reason)}`, {
      stack: reason?.stack
    });
  });
}

export class Logger {
  private config: LoggerConfig;

  constructor(config?: Partial<LoggerConfig>) {
    this.config = {
      enabled: !isTest, // Disable in tests by default
      level: DEFAULT_LEVEL,
      prefix: '',
      ...config
    };
  }

  private shouldLog(level: LogLevel): boolean {
    if (!this.config.enabled) return false;

    // When the debug channel is on (NS_FRONTEND_LOG set), log every level so
    // the captured file is complete even though release builds default to warn.
    if (isChannelEnabledSync()) return true;

    const currentLevelIndex = LOG_LEVELS.indexOf(this.config.level);
    const requestedLevelIndex = LOG_LEVELS.indexOf(level);

    return requestedLevelIndex >= currentLevelIndex;
  }

  private formatMessage(message: string): string {
    const prefix = this.config.prefix ? `[${this.config.prefix}] ` : '';
    return `${prefix}${message}`;
  }

  /**
   * Debug level - detailed debugging information
   * Hidden in production by default, use browser devtools to filter
   */
  debug(message: string, data?: unknown): void {
    if (this.shouldLog('debug')) {
      if (data !== undefined) {
        console.debug(this.formatMessage(message), data);
      } else {
        console.debug(this.formatMessage(message));
      }
      forwardToFile('debug', this.formatMessage(message), data);
    }
  }

  /**
   * Info level - operational information
   * Hidden in production by default
   */
  info(message: string, data?: unknown): void {
    if (this.shouldLog('info')) {
      if (data !== undefined) {
        console.log(this.formatMessage(message), data);
      } else {
        console.log(this.formatMessage(message));
      }
      forwardToFile('info', this.formatMessage(message), data);
    }
  }

  /**
   * Warn level - warnings that don't block operation
   * Always visible in production
   */
  warn(message: string, data?: unknown): void {
    if (this.shouldLog('warn')) {
      if (data !== undefined) {
        console.warn(this.formatMessage(message), data);
      } else {
        console.warn(this.formatMessage(message));
      }
      forwardToFile('warn', this.formatMessage(message), data);
    }
  }

  /**
   * Error level - actual errors
   * Always visible in production
   */
  error(message: string, data?: unknown): void {
    if (this.shouldLog('error')) {
      if (data !== undefined) {
        console.error(this.formatMessage(message), data);
      } else {
        console.error(this.formatMessage(message));
      }
      forwardToFile('error', this.formatMessage(message), data);
    }
  }

  /**
   * Group related log messages together
   * Useful for debugging complex operations
   */
  group(label: string): void {
    if (this.shouldLog('debug')) {
      console.group(this.formatMessage(label));
    }
  }

  /**
   * End a log group
   */
  groupEnd(): void {
    if (this.shouldLog('debug')) {
      console.groupEnd();
    }
  }

  /**
   * Time an operation
   * Returns a function to call when the operation is complete
   *
   * Usage:
   *   const done = log.time('Loading data');
   *   await loadData();
   *   done(); // Logs: [MyService] Loading data: 123ms
   */
  time(label: string): () => void {
    if (!this.shouldLog('debug')) {
      return () => {}; // No-op if debug logging disabled
    }

    const start = performance.now();
    const formattedLabel = this.formatMessage(label);

    return () => {
      const duration = performance.now() - start;
      console.debug(`${formattedLabel}: ${duration.toFixed(2)}ms`);
    };
  }
}

/**
 * Create a logger instance with optional prefix
 * Logs are automatically disabled during tests for better performance
 *
 * @param prefix - Service/component name to prefix all log messages
 * @returns Logger instance with the specified prefix
 *
 * @example
 * const log = createLogger('NavigationService');
 * log.debug('Resolving node', { nodeId });
 * // Output: [NavigationService] Resolving node { nodeId: '...' }
 */
export function createLogger(prefix?: string): Logger {
  return new Logger({ prefix });
}

/**
 * Default logger instance (no prefix)
 * Use createLogger() for service-specific loggers with prefixes
 */
export const logger = new Logger();
