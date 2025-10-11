/**
 * Test Database Utilities for HTTP Dev Server Testing
 *
 * Provides utilities for creating isolated test databases, ensuring
 * proper cleanup, and initializing databases for tests.
 *
 * # Usage
 *
 * ```typescript
 * import { createTestDatabase, cleanupTestDatabase, initializeTestDatabase } from './test-database';
 *
 * describe('My Test Suite', () => {
 *   let dbPath: string;
 *
 *   beforeEach(async () => {
 *     dbPath = createTestDatabase('my-test');
 *     await initializeTestDatabase(dbPath);
 *   });
 *
 *   afterEach(async () => {
 *     await cleanupTestDatabase(dbPath);
 *   });
 *
 *   it('should work with real database', async () => {
 *     // Test with real backend access
 *   });
 * });
 * ```
 */

import { HttpAdapter } from '$lib/services/backend-adapter';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { tmpdir } from 'node:os';

/**
 * Create a unique test database path
 *
 * @param testName - Name of the test (used for unique file naming)
 * @returns Path to the test database file
 *
 * @example
 * const dbPath = createTestDatabase('node-crud-tests');
 * // Returns: /tmp/nodespace-test-node-crud-tests-1234567890.db
 */
export function createTestDatabase(testName: string): string {
  const timestamp = Date.now();
  const sanitizedName = testName.replace(/[^a-z0-9-]/gi, '-').toLowerCase();
  const filename = `nodespace-test-${sanitizedName}-${timestamp}.db`;
  return path.join(tmpdir(), filename);
}

/**
 * Clean up a test database by deleting the file
 *
 * @param dbPath - Path to the database file to delete
 *
 * @example
 * await cleanupTestDatabase('/tmp/nodespace-test-my-test-1234.db');
 */
export async function cleanupTestDatabase(dbPath: string): Promise<void> {
  try {
    if (fs.existsSync(dbPath)) {
      fs.unlinkSync(dbPath);
      console.log(`[Test] Cleaned up test database: ${dbPath}`);
    }

    // Also clean up any -shm and -wal files (SQLite temporary files)
    const shmPath = `${dbPath}-shm`;
    const walPath = `${dbPath}-wal`;

    if (fs.existsSync(shmPath)) {
      fs.unlinkSync(shmPath);
    }

    if (fs.existsSync(walPath)) {
      fs.unlinkSync(walPath);
    }
  } catch (error) {
    console.warn(`[Test] Warning: Failed to clean up test database: ${error}`);
    // Don't throw - cleanup failures shouldn't fail tests
  }
}

/**
 * Initialize a test database via HTTP dev server
 *
 * This calls the `/api/database/init?db_path=` endpoint to initialize
 * the database at the specified path.
 *
 * @param dbPath - Path to the database file
 * @param serverUrl - HTTP dev server URL (default: http://localhost:3001)
 * @returns Path to the initialized database (should match input)
 *
 * @example
 * const dbPath = createTestDatabase('my-test');
 * await initializeTestDatabase(dbPath);
 */
export async function initializeTestDatabase(
  dbPath: string,
  serverUrl: string = 'http://localhost:3001'
): Promise<string> {
  const adapter = new HttpAdapter(serverUrl);
  const initializedPath = await adapter.initializeDatabase(dbPath);
  console.log(`[Test] Initialized test database: ${initializedPath}`);
  return initializedPath;
}

/**
 * Clean up all test databases in the temp directory
 *
 * Useful for cleaning up leftover databases from failed tests.
 * Can be run manually or as part of a cleanup script.
 *
 * @example
 * // In a global test teardown
 * await cleanupAllTestDatabases();
 */
export async function cleanupAllTestDatabases(): Promise<void> {
  try {
    const tempDir = tmpdir();
    const files = fs.readdirSync(tempDir);

    const testDbFiles = files.filter(
      (file) =>
        file.startsWith('nodespace-test-') &&
        (file.endsWith('.db') || file.endsWith('.db-shm') || file.endsWith('.db-wal'))
    );

    for (const file of testDbFiles) {
      const filePath = path.join(tempDir, file);
      try {
        fs.unlinkSync(filePath);
        console.log(`[Test] Cleaned up orphaned test database: ${file}`);
      } catch (error) {
        console.warn(`[Test] Warning: Failed to clean up ${file}: ${error}`);
      }
    }

    if (testDbFiles.length > 0) {
      console.log(`[Test] Cleaned up ${testDbFiles.length} test database files`);
    }
  } catch (error) {
    console.warn(`[Test] Warning: Failed to clean up test databases: ${error}`);
  }
}
