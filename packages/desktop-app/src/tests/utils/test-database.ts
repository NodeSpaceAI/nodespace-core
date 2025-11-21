/**
 * Test Database Utilities - DEPRECATED
 *
 * NOTE: Issue #558 deleted the backend-adapter service that this module depended on.
 * HTTP dev server testing is no longer supported. Tests should use in-memory mode
 * or be rewritten to use Tauri commands directly (when running in Tauri context).
 *
 * All functions in this module now throw or return stub values.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { tmpdir } from 'node:os';

/**
 * Create a unique test database path
 * @deprecated HTTP dev server testing no longer supported
 */
export function createTestDatabase(testName: string): string {
  const timestamp = Date.now();
  const random = Math.random().toString(36).substring(2, 8);
  const sanitizedName = testName.replace(/[^a-z0-9-]/gi, '-').toLowerCase();
  const filename = `nodespace-test-${sanitizedName}-${timestamp}-${random}.db`;
  return path.join(tmpdir(), filename);
}

/**
 * Clean up a test database by deleting the file
 * @deprecated HTTP dev server testing no longer supported
 */
export async function cleanupTestDatabase(dbPath: string): Promise<void> {
  try {
    if (fs.existsSync(dbPath)) {
      fs.unlinkSync(dbPath);
    }
    const shmPath = `${dbPath}-shm`;
    const walPath = `${dbPath}-wal`;
    if (fs.existsSync(shmPath)) fs.unlinkSync(shmPath);
    if (fs.existsSync(walPath)) fs.unlinkSync(walPath);
  } catch {
    // Ignore cleanup failures
  }
}

/**
 * Initialize a test database - NO LONGER FUNCTIONAL
 * @deprecated HTTP dev server and backend-adapter were deleted in Issue #558
 * @throws Error indicating the function is deprecated
 */
export async function initializeTestDatabase(
  _dbPath: string,
  _serverUrl: string = 'http://localhost:3001',
  _maxRetries: number = 3
): Promise<string> {
  throw new Error(
    '[DEPRECATED] initializeTestDatabase is no longer functional. ' +
    'HTTP dev server testing was removed in Issue #558. ' +
    'Use in-memory tests or Tauri-based integration tests instead.'
  );
}

/**
 * Result of database cleanup operation
 */
export interface CleanDatabaseResult {
  success: boolean;
  deletedCount: number;
  totalCount: number;
}

/**
 * Clean database - NO LONGER FUNCTIONAL
 * @deprecated HTTP dev server and backend-adapter were deleted in Issue #558
 */
export async function cleanDatabase(_backend: unknown): Promise<CleanDatabaseResult> {
  console.warn('[DEPRECATED] cleanDatabase is no longer functional.');
  return { success: false, deletedCount: 0, totalCount: 0 };
}

/**
 * Clean up all test databases in the temp directory
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
      } catch {
        // Ignore
      }
    }
  } catch {
    // Ignore
  }
}

/**
 * Wait for all pending database writes to complete
 */
export async function waitForDatabaseWrites(
  maxWaitMs: number = 5000,
  pollInterval: number = 50
): Promise<void> {
  // In in-memory mode, skip waiting
  if (process.env.TEST_USE_DATABASE !== 'true') {
    await new Promise((resolve) => setTimeout(resolve, 10));
    return;
  }

  const startTime = Date.now();

  while (sharedNodeStore.hasPendingWrites()) {
    const elapsed = Date.now() - startTime;
    if (elapsed >= maxWaitMs) {
      throw new Error(`[Test] Timeout waiting for database writes (${maxWaitMs}ms).`);
    }
    await new Promise((resolve) => setTimeout(resolve, pollInterval));
  }
}
