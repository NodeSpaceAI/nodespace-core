/**
 * Test Database Mode Utilities
 *
 * Provides utilities for determining whether tests should use real database
 * via HTTP adapter or in-memory mode for faster execution.
 *
 * # Database Modes
 *
 * **In-Memory Mode (default)**
 * - Fast execution (~2-3 seconds for full suite)
 * - No database initialization
 * - PersistenceCoordinator silently catches DatabaseInitializationError
 * - Suitable for business logic and UI tests
 * - Run with: `bun run test`
 *
 * **Database Mode (opt-in)**
 * - Full integration testing with real SQLite database
 * - Uses HTTP adapter to communicate with dev-server
 * - Slower execution (~10-15 seconds for full suite)
 * - Verifies actual database persistence
 * - Run with: `TEST_USE_DATABASE=true bun run test` or `bun run test:db`
 *
 * # Usage
 *
 * ```typescript
 * import { shouldUseDatabase, initializeDatabaseIfNeeded } from '../utils/should-use-database';
 *
 * describe('My Test Suite', () => {
 *   let dbPath: string | null;
 *
 *   beforeEach(async () => {
 *     // Automatically initializes database only if TEST_USE_DATABASE=true
 *     dbPath = await initializeDatabaseIfNeeded('my-test');
 *   });
 *
 *   afterEach(async () => {
 *     if (dbPath) {
 *       await cleanupTestDatabase(dbPath);
 *     }
 *   });
 * });
 * ```
 *
 * # Design Rationale
 *
 * This pattern separates test concerns:
 * - Most tests focus on business logic (in-memory is sufficient)
 * - Integration tests explicitly opt-in to database mode
 * - Clear separation of fast vs. comprehensive test runs
 * - No noisy console errors in default mode
 */

import { createTestDatabase, initializeTestDatabase, cleanupTestDatabase } from './test-database';

/**
 * Determines if tests should use real database via HTTP adapter
 *
 * Default: false (in-memory mode for speed)
 * Override: TEST_USE_DATABASE=true (full database integration)
 *
 * @returns true if tests should initialize and use real database
 *
 * @example
 * if (shouldUseDatabase()) {
 *   console.log('Running with real database');
 * } else {
 *   console.log('Running in-memory only');
 * }
 */
export function shouldUseDatabase(): boolean {
  return process.env.TEST_USE_DATABASE === 'true';
}

/**
 * Initialize database for test if in database mode
 *
 * This is idempotent and safe to call even if not needed.
 * Returns null in in-memory mode, or the database path in database mode.
 *
 * @param testName - Name of the test (used for unique file naming)
 * @returns Database path if in database mode, null if in-memory mode
 *
 * @example
 * beforeEach(async () => {
 *   dbPath = await initializeDatabaseIfNeeded('sibling-chain-integrity');
 *   // dbPath will be null in in-memory mode, or a path in database mode
 * });
 */
export async function initializeDatabaseIfNeeded(testName: string): Promise<string | null> {
  if (!shouldUseDatabase()) {
    return null; // In-memory mode, no database
  }

  const dbPath = createTestDatabase(testName);
  await initializeTestDatabase(dbPath);
  return dbPath;
}

/**
 * Cleanup database if it was initialized
 *
 * Safe to call with null (no-op in in-memory mode)
 *
 * @param dbPath - Database path to cleanup, or null
 *
 * @example
 * afterEach(async () => {
 *   await cleanupDatabaseIfNeeded(dbPath);
 * });
 */
export async function cleanupDatabaseIfNeeded(dbPath: string | null): Promise<void> {
  if (dbPath) {
    await cleanupTestDatabase(dbPath);
  }
}

/**
 * Get current test mode name for logging
 *
 * @returns "database" or "in-memory"
 */
export function getTestModeName(): 'database' | 'in-memory' {
  return shouldUseDatabase() ? 'database' : 'in-memory';
}
