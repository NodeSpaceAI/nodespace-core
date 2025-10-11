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

import { HttpAdapter, type BackendAdapter } from '$lib/services/backend-adapter';
import { tauriNodeService } from '$lib/services/tauri-node-service';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { tmpdir } from 'node:os';

/**
 * Create a unique test database path
 *
 * Uses timestamp and random suffix for uniqueness, preventing collisions
 * in parallel test execution.
 *
 * @param testName - Name of the test (used for unique file naming)
 * @returns Path to the test database file
 *
 * @example
 * const dbPath = createTestDatabase('node-crud-tests');
 * // Returns: /tmp/nodespace-test-node-crud-tests-1234567890-a1b2c3.db
 */
export function createTestDatabase(testName: string): string {
  const timestamp = Date.now();
  const random = Math.random().toString(36).substring(2, 8); // 6 character random suffix
  const sanitizedName = testName.replace(/[^a-z0-9-]/gi, '-').toLowerCase();
  const filename = `nodespace-test-${sanitizedName}-${timestamp}-${random}.db`;
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
 * the database at the specified path, and also initializes the singleton
 * tauriNodeService to use this database for the test.
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

  // CRITICAL: Also initialize the singleton tauriNodeService with this test database
  // SharedNodeStore uses tauriNodeService.createNode() to persist nodes, so we need
  // to tell the singleton to use this test's specific database
  await tauriNodeService.initializeDatabase(dbPath);

  console.log(`[Test] Initialized test database: ${initializedPath}`);
  return initializedPath;
}

/**
 * Result of database cleanup operation
 */
export interface CleanDatabaseResult {
  /** Whether all nodes were successfully deleted */
  success: boolean;
  /** Number of nodes successfully deleted */
  deletedCount: number;
  /** Total number of nodes found for deletion */
  totalCount: number;
}

/**
 * Clean database by deleting all nodes
 *
 * This function queries all root nodes and deletes them recursively,
 * effectively clearing the database for the next test.
 *
 * @param backend - Backend adapter to use for operations
 * @returns Cleanup result with success status and counts
 *
 * @example
 * beforeEach(async () => {
 *   const result = await cleanDatabase(backend);
 *   // Optional: assert on cleanup success if needed
 *   expect(result.success).toBe(true);
 * });
 */
export async function cleanDatabase(backend: BackendAdapter): Promise<CleanDatabaseResult> {
  try {
    // Query all root nodes (parentId = null)
    const rootNodes = await backend.queryNodes({ parentId: null });

    // Delete each root node (cascade delete will handle children)
    // Handle errors individually so one failure doesn't stop cleanup
    let successCount = 0;

    for (const node of rootNodes) {
      try {
        await backend.deleteNode(node.id);
        successCount++;
      } catch {
        // Ignore deletion errors - node might already be deleted by cascade
      }
    }

    if (successCount > 0) {
      console.log(
        `[Test] Cleaned database: deleted ${successCount}/${rootNodes.length} root nodes`
      );
    }

    return {
      success: successCount === rootNodes.length,
      deletedCount: successCount,
      totalCount: rootNodes.length
    };
  } catch (error) {
    console.warn(`[Test] Warning: Failed to clean database: ${error}`);
    // Don't throw - cleanup failures shouldn't fail tests
    return {
      success: false,
      deletedCount: 0,
      totalCount: 0
    };
  }
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

/**
 * Wait for all pending database writes to complete
 *
 * This function polls the SharedNodeStore to check if there are pending
 * database writes, and waits until all writes complete or timeout is reached.
 *
 * CRITICAL: Use this after any node creation/update in tests to ensure
 * database persistence completes before checking results.
 *
 * @param maxWaitMs - Maximum time to wait in milliseconds (default: 5000)
 * @param pollInterval - How often to check for completion in milliseconds (default: 50)
 * @returns Promise that resolves when all writes complete or timeout
 * @throws Error if timeout is reached with pending writes
 *
 * @example
 * const nodeId = service.createNode('node-1', '', 'text');
 * await waitForDatabaseWrites();
 * expect(sharedNodeStore.getTestErrors()).toHaveLength(0);
 * const dbNode = await adapter.getNode(nodeId);
 * expect(dbNode).toBeDefined();
 */
export async function waitForDatabaseWrites(
  maxWaitMs: number = 5000,
  pollInterval: number = 50
): Promise<void> {
  const startTime = Date.now();

  while (sharedNodeStore.hasPendingWrites()) {
    const elapsed = Date.now() - startTime;

    if (elapsed >= maxWaitMs) {
      throw new Error(
        `[Test] Timeout waiting for database writes to complete (${maxWaitMs}ms). ` +
          `This indicates a database persistence issue.`
      );
    }

    // Wait before next check
    await new Promise((resolve) => setTimeout(resolve, pollInterval));
  }

  // Defensive check: Verify loop exited correctly
  if (sharedNodeStore.hasPendingWrites()) {
    throw new Error(
      '[Test] Internal error: waitForDatabaseWrites loop exited but writes still pending'
    );
  }

  // All writes completed successfully
  console.log(`[Test] All database writes completed in ${Date.now() - startTime}ms`);
}
