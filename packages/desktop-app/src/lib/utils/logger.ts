/**
 * Test-aware logger utility
 * Automatically silences logs during test execution to improve performance
 */

// Define log levels as const array for type safety and single source of truth
const LOG_LEVELS = ['debug', 'info', 'warn', 'error'] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

interface LoggerConfig {
  enabled: boolean;
  level: LogLevel;
  prefix?: string;
}

class Logger {
  private config: LoggerConfig;

  constructor(config?: Partial<LoggerConfig>) {
    // Disable logging in test environment by default
    // Support multiple runtime environments (Vite, Node, Bun, etc.)
    const isTest =
      (typeof import.meta !== 'undefined' && import.meta.env?.VITEST === 'true') ||
      (typeof process !== 'undefined' && process.env.VITEST === 'true');
    const isDebug =
      (typeof import.meta !== 'undefined' && import.meta.env?.DEV === true) ||
      (typeof process !== 'undefined' && process.env.NODE_ENV === 'development');

    this.config = {
      enabled: !isTest || isDebug,
      level: 'info',
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

  debug(message: string, data?: unknown): void {
    if (this.shouldLog('debug')) {
      if (data !== undefined) {
        console.debug(this.formatMessage(message), data);
      } else {
        console.debug(this.formatMessage(message));
      }
    }
  }

  info(message: string, data?: unknown): void {
    if (this.shouldLog('info')) {
      if (data !== undefined) {
        console.log(this.formatMessage(message), data);
      } else {
        console.log(this.formatMessage(message));
      }
    }
  }

  warn(message: string, data?: unknown): void {
    if (this.shouldLog('warn')) {
      if (data !== undefined) {
        console.warn(this.formatMessage(message), data);
      } else {
        console.warn(this.formatMessage(message));
      }
    }
  }

  error(message: string, data?: unknown): void {
    if (this.shouldLog('error')) {
      if (data !== undefined) {
        console.error(this.formatMessage(message), data);
      } else {
        console.error(this.formatMessage(message));
      }
    }
  }
}

/**
 * Create a logger instance with optional prefix
 * Logs are automatically disabled during tests for better performance
 */
export function createLogger(prefix?: string): Logger {
  return new Logger({ prefix });
}

/**
 * Default logger instance
 */
export const logger = new Logger();
