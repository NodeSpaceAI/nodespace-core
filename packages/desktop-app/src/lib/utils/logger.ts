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

// Define log levels as const array for type safety and single source of truth
const LOG_LEVELS = ['debug', 'info', 'warn', 'error'] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

interface LoggerConfig {
  enabled: boolean;
  level: LogLevel;
  prefix?: string;
}

// Environment detection (cached at module load)
const isTest =
  (typeof import.meta !== 'undefined' && import.meta.env?.VITEST === 'true') ||
  (typeof process !== 'undefined' && process.env?.VITEST === 'true');

const isProd =
  (typeof import.meta !== 'undefined' && import.meta.env?.PROD === true) ||
  (typeof process !== 'undefined' && process.env?.NODE_ENV === 'production');

// Default log level based on environment
// Production: only show warnings and errors (clean console)
// Development: show all logs including debug
const DEFAULT_LEVEL: LogLevel = isProd ? 'warn' : 'debug';

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
