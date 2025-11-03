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
 * Initialize a test database via HTTP dev server with verification
 *
 * This function:
 * 1. Calls `/api/database/init?db_path=` to switch the dev server database
 * 2. Verifies the switch completed by creating and reading a sentinel node
 * 3. Retries up to 3 times if verification fails (race condition)
 * 4. Updates singleton tauriNodeService to use this database
 *
 * This fixes Issue #266 (database initialization race condition) by ensuring
 * the database switch completes before subsequent operations.
 *
 * NOTE: This verification ensures database switch timing is correct, but doesn't
 * fully solve backend SQLite write contention. The backend dev-server now implements
 * write serialization (mutex-based queue) to prevent concurrent write conflicts.
 * See: AppState.write_lock in mod.rs and node_endpoints.rs (Issue #266).
 *
 * @param dbPath - Path to the database file
 * @param serverUrl - HTTP dev server URL (default: http://localhost:3001)
 * @param maxRetries - Maximum retry attempts for verification (default: 3)
 * @returns Path to the initialized database (should match input)
 * @throws Error if database switch fails or cannot be verified after retries
 *
 * @example
 * const dbPath = createTestDatabase('my-test');
 * await initializeTestDatabase(dbPath);
 */
export async function initializeTestDatabase(
  dbPath: string,
  serverUrl: string = 'http://localhost:3001',
  maxRetries: number = 3
): Promise<string> {
  const adapter = new HttpAdapter(serverUrl);

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      // STEP 1: Call database init endpoint to switch database
      const initializedPath = await adapter.initializeDatabase(dbPath);

      // STEP 2: Verify database switch by creating, reading, and deleting a sentinel node
      // This ensures subsequent operations hit the correct database
      // Use multiple retries with backoff to handle transient backend readiness issues
      let verificationSuccess = false;
      for (let verifyAttempt = 1; verifyAttempt <= 3; verifyAttempt++) {
        let sentinelId: string | null = null;
        try {
          sentinelId = `__test_sentinel_${Date.now()}_${Math.random().toString(36).substring(7)}`;

          // Create sentinel
          await adapter.createNode({
            id: sentinelId,
            nodeType: 'text',
            content: '__SENTINEL__',
            parentId: null,
            containerNodeId: null,
            beforeSiblingId: null,
            properties: {},
            embeddingVector: null,
            mentions: []
          });

          // Verify we can read it back
          const sentinelNode = await adapter.getNode(sentinelId);
          if (!sentinelNode || sentinelNode.content !== '__SENTINEL__') {
            throw new Error('Sentinel node not found or corrupted');
          }

          // Clean up sentinel
          await adapter.deleteNode(sentinelId, 1);

          // Verify deletion succeeded
          const deletedCheck = await adapter.getNode(sentinelId);
          if (deletedCheck !== null) {
            throw new Error('Sentinel node still exists after deletion');
          }

          sentinelId = null; // Mark as successfully cleaned up
          verificationSuccess = true;
          break; // Success!
        } catch (error) {
          if (verifyAttempt === 3) {
            throw new Error(
              `Database verification failed after 3 attempts (attempt ${attempt}/${maxRetries}): ${error instanceof Error ? error.message : String(error)}`
            );
          }
          // Retry with exponential backoff
          // Base delay: 50ms (sufficient for local HTTP roundtrip + SQLite operation)
          // Multiplier: linear (50ms, 100ms, 150ms) balances retry speed vs. backend load
          // Total max wait: 300ms across 3 attempts keeps tests fast while handling transient issues
          await new Promise((resolve) => setTimeout(resolve, 50 * verifyAttempt));
        } finally {
          // CLEANUP: Ensure sentinel is deleted even if verification fails
          // This prevents database pollution from failed verification attempts
          if (sentinelId) {
            try {
              await adapter.deleteNode(sentinelId, 1);
            } catch {
              // Ignore cleanup failures - node may not exist or database may be inaccessible
              // This is a best-effort cleanup
            }
          }
        }
      }

      if (!verificationSuccess) {
        throw new Error('Database verification failed unexpectedly');
      }

      // STEP 3: Add stabilization delay to ensure backend is fully ready
      // Delay: 100ms (empirically determined - see Issue #266 testing)
      // Why 100ms: Rust dev-server needs time to complete database connection swap
      //            and service reinitialization. Values <100ms caused intermittent failures.
      //            Values >100ms provided no additional benefit (diminishing returns).
      // This prevents race conditions where immediate operations hit stale database state.
      await new Promise((resolve) => setTimeout(resolve, 100));

      // STEP 4: Update the singleton tauriNodeService to use this test database
      // SharedNodeStore uses tauriNodeService.createNode() to persist nodes, so we need
      // to tell the singleton to use this test's specific database.
      //
      // NOTE: We do NOT call tauriNodeService.initializeDatabase(dbPath) because that would
      // make a second HTTP request to the same endpoint, causing the database to be swapped
      // twice. Instead, we use the test-only API to update the singleton's internal state
      // since we've already initialized the dev server above.
      tauriNodeService.__testOnly_setInitializedPath(dbPath);

      console.log(
        `[Test] Database initialized and verified: ${initializedPath} (attempt ${attempt})`
      );
      return initializedPath;
    } catch (error) {
      if (attempt === maxRetries) {
        // Final attempt failed - throw error
        const err = error instanceof Error ? error : new Error(String(error));
        throw new Error(
          `Database initialization failed after ${maxRetries} attempts: ${err.message}\n` +
            `This indicates a persistent race condition or backend issue.\n` +
            `Database path: ${dbPath}`
        );
      }

      // Retry after exponential backoff
      // Base: 100ms (minimum time for backend to recover from transient error)
      // Growth: 2x exponential (100ms → 200ms → 400ms)
      // Cap: 1000ms (prevents excessive test delays while handling severe backend issues)
      // Why exponential: Gives backend progressively more time to recover from resource contention
      const backoffMs = Math.min(100 * Math.pow(2, attempt - 1), 1000);
      console.warn(
        `[Test] Database initialization attempt ${attempt} failed, retrying in ${backoffMs}ms...`,
        error instanceof Error ? error.message : String(error)
      );
      await new Promise((resolve) => setTimeout(resolve, backoffMs));
    }
  }

  // This should never be reached due to throw in final attempt, but TypeScript needs it
  throw new Error('Database initialization failed unexpectedly');
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
        await backend.deleteNode(node.id, node.version);
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
  // In in-memory mode (TEST_USE_DATABASE !== 'true'), skip waiting for database writes
  // since persistence is intentionally disabled and will never complete
  if (process.env.TEST_USE_DATABASE !== 'true') {
    // Still wait a brief moment for any synchronous operations to complete
    await new Promise((resolve) => setTimeout(resolve, 10));
    return;
  }

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
