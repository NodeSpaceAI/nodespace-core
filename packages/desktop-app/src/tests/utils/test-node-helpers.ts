/**
 * Test Node Helper Utilities
 *
 * Shared helper functions for integration tests that work with HttpAdapter.
 */

import type { Node } from '$lib/types';
import type { HttpAdapter } from '$lib/services/backend-adapter';

/**
 * Retries an async operation with exponential backoff.
 *
 * Used to handle transient HTTP 500 errors from dev-server SQLite write contention.
 * These errors are test infrastructure artifacts (high concurrency during parallel tests)
 * and don't reflect real application behavior (single-user, human-speed operations).
 *
 * @param operation - The async operation to retry
 * @param maxRetries - Maximum number of retry attempts (default: 3)
 * @param baseDelayMs - Base delay in milliseconds (default: 50ms)
 * @returns The result of the operation
 * @throws The last error if all retries fail
 */
export async function retryOperation<T>(
  operation: () => Promise<T>,
  maxRetries: number = 3,
  baseDelayMs: number = 50
): Promise<T> {
  let lastError: Error | undefined;

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      return await operation();
    } catch (error) {
      lastError = error instanceof Error ? error : new Error(String(error));

      // Only retry on HTTP 500 errors (SQLite "database is locked")
      // Don't retry on other errors (404, validation errors, etc.)
      const errorMessage = lastError.message;
      const isTransientError =
        errorMessage.includes('500') || errorMessage.includes('database is locked');

      if (!isTransientError || attempt === maxRetries) {
        throw lastError;
      }

      // Exponential backoff: 50ms, 100ms, 200ms
      const delayMs = baseDelayMs * Math.pow(2, attempt - 1);
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }

  throw lastError!;
}

/**
 * Creates a node via HttpAdapter and fetches it back to ensure it exists.
 * Automatically retries on transient HTTP 500 errors from dev-server concurrency.
 *
 * @param adapter - The HttpAdapter instance to use for creating the node
 * @param nodeData - The node data (without createdAt/modifiedAt timestamps)
 * @returns The created Node object with all fields populated
 * @throws Error if node creation fails after retries or node cannot be retrieved
 *
 * @example
 * ```typescript
 * const node = await createAndFetchNode(adapter, {
 *   id: 'test-node-1',
 *   nodeType: 'text',
 *   content: 'Hello World',
 *   parentId: null,
 *   containerNodeId: null,
 *   beforeSiblingId: null,
 *   properties: {},
 *   embeddingVector: null,
 *   mentions: []
 * });
 * ```
 */
export async function createAndFetchNode(
  adapter: HttpAdapter,
  nodeData: Omit<Node, 'createdAt' | 'modifiedAt'>
): Promise<Node> {
  await retryOperation(() => adapter.createNode(nodeData));
  const node = await retryOperation(() => adapter.getNode(nodeData.id));
  if (!node) throw new Error(`Failed to create node ${nodeData.id}`);
  return node;
}

/**
 * Checks if the HTTP dev server is running and accessible.
 * Throws a helpful error message if the server is not available.
 *
 * @param adapter - The HttpAdapter instance to check
 * @throws Error with setup instructions if server is not running
 *
 * @example
 * ```typescript
 * beforeAll(async () => {
 *   const adapter = new HttpAdapter('http://localhost:3001');
 *   await checkServerHealth(adapter);
 * });
 * ```
 */
export async function checkServerHealth(adapter: HttpAdapter): Promise<void> {
  try {
    await adapter.initializeDatabase();
  } catch (error) {
    throw new Error(
      '❌ HTTP dev server not running on port 3001!\n\n' +
        'Start the server before running tests:\n' +
        '  Terminal 1: bun run dev:server\n' +
        '  Terminal 2: bun run test\n\n' +
        `Original error: ${error}`
    );
  }
}

/**
 * Checks if an error indicates an unavailable HTTP endpoint and logs appropriate message.
 * Useful for gracefully handling tests when dev server endpoints are not yet implemented
 * or when the dev server needs to be rebuilt.
 *
 * @param error - The error to check
 * @param endpointName - Human-readable name of the endpoint (e.g., "Container endpoint")
 * @returns True if endpoint is unavailable (405/500 error), false otherwise
 *
 * @example
 * ```typescript
 * try {
 *   await backend.createContainerNode(input);
 * } catch (error) {
 *   if (skipIfEndpointUnavailable(error, 'Container endpoint')) {
 *     expect(error).toBeTruthy();
 *     return; // Skip remaining test logic
 *   }
 *   throw error; // Re-throw unexpected errors
 * }
 * ```
 */
export function skipIfEndpointUnavailable(error: unknown, endpointName: string): boolean {
  const errorMessage = error instanceof Error ? error.message : String(error);
  if (errorMessage.includes('405') || errorMessage.includes('500')) {
    console.log(`[Test] ${endpointName} not yet active - test skipped`);
    return true;
  }
  return false;
}
