/**
 * Test Node Helper Utilities
 *
 * Shared helper functions for integration tests that work with HttpAdapter.
 */

import type { Node } from '$lib/types';
import type { HttpAdapter } from '$lib/services/backend-adapter';

/**
 * Creates a node via HttpAdapter and fetches it back to ensure it exists.
 *
 * @param adapter - The HttpAdapter instance to use for creating the node
 * @param nodeData - The node data (without createdAt/modifiedAt timestamps)
 * @returns The created Node object with all fields populated
 * @throws Error if node creation fails or node cannot be retrieved
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
  await adapter.createNode(nodeData);
  const node = await adapter.getNode(nodeData.id);
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
