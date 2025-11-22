/**
 * Test Environment Utilities
 *
 * Provides utilities for detecting test environment and determining
 * whether database errors should be logged to console.
 *
 * These utilities support the TEST_USE_DATABASE flag pattern which allows tests
 * to run in two modes:
 * - In-memory mode (default): Fast, no database, errors suppressed
 * - Database mode (TEST_USE_DATABASE=true): Full integration, errors logged
 *
 * Note: Browser mode detection is handled by the BackendAdapter pattern in
 * backend-adapter.ts which automatically selects the right transport:
 * - Tauri IPC (desktop app)
 * - HTTP fetch (browser dev mode via dev-proxy)
 * - Mocks (test environment)
 *
 * @see src/tests/utils/should-use-database.ts for full test utilities
 * @see src/lib/services/backend-adapter.ts for environment-aware backend communication
 */

/**
 * Determines if database errors should be logged to console
 *
 * Logs errors in production OR when explicitly testing database integration.
 * Suppresses errors in in-memory test mode where they're expected and noisy.
 *
 * @returns true if database errors should be logged to console
 *
 * @example
 * catch (dbError) {
 *   const error = dbError instanceof Error ? dbError : new Error(String(dbError));
 *
 *   if (shouldLogDatabaseErrors()) {
 *     console.error('[Service] Database operation failed:', error);
 *   }
 *
 *   // Always track in test environment for verification
 *   if (isTestEnvironment()) {
 *     this.testErrors.push(error);
 *   }
 * }
 */
export function shouldLogDatabaseErrors(): boolean {
  const inTestMode = typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
  const inDatabaseMode = typeof process !== 'undefined' && process.env.TEST_USE_DATABASE === 'true';

  // Log in production OR when explicitly testing database integration
  return !inTestMode || inDatabaseMode;
}

/**
 * Determines if code is running in test environment
 *
 * Used for test-specific behavior like error tracking in testErrors array.
 *
 * @returns true if running in test environment (NODE_ENV=test)
 *
 * @example
 * if (isTestEnvironment()) {
 *   this.testErrors.push(error);
 * }
 */
export function isTestEnvironment(): boolean {
  return typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
}
