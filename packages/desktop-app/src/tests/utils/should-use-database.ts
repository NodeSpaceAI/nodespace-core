/**
 * Test Database Mode Utilities
 *
 * Provides utilities for switching between in-memory testing (default) and full
 * database integration testing via the TEST_USE_DATABASE environment variable.
 *
 * **Quick Start:**
 * - `bun run test` - Fast in-memory mode (default)
 * - `bun run test:db` - Full database integration mode
 *
 * **For comprehensive documentation**, see:
 * - [Testing Guide](../../../docs/architecture/development/testing-guide.md)
 * - Usage patterns, migration guide, and design rationale
 *
 * @see docs/architecture/development/testing-guide.md
 */

import { createTestDatabase, initializeTestDatabase, cleanupTestDatabase } from './test-database';

// Re-export utility functions from production code
export { shouldLogDatabaseErrors, isTestEnvironment } from '$lib/utils/test-environment';

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
