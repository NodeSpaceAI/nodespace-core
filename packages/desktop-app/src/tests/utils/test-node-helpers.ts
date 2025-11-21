/**
 * Test Node Helper Utilities - DEPRECATED
 *
 * NOTE: Issue #558 deleted the backend-adapter service that this module depended on.
 * HTTP dev server testing is no longer supported. Tests should use in-memory mode
 * or be rewritten to use Tauri commands directly (when running in Tauri context).
 */

import type { Node } from '$lib/types';
import { TestNodeBuilder } from './test-node-builder';
import { shouldUseDatabase } from './should-use-database';

/**
 * HTTP Adapter interface - DEPRECATED stub for type compatibility
 * @deprecated Use tauri-commands instead
 */
export interface HttpAdapter {
  initializeDatabase(): Promise<void>;
  createNode(data: unknown): Promise<void>;
  getNode(id: string): Promise<Node | null>;
  deleteNode(id: string, version: number): Promise<void>;
}

/**
 * Retries an async operation with exponential backoff.
 * @deprecated HTTP dev server testing no longer supported
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
      if (attempt === maxRetries) {
        throw lastError;
      }
      const delayMs = baseDelayMs * Math.pow(2, attempt - 1);
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }

  throw lastError!;
}

/**
 * Creates a node via HttpAdapter and fetches it back - NO LONGER FUNCTIONAL
 * @deprecated HTTP dev server and backend-adapter were deleted in Issue #558
 */
export async function createAndFetchNode(
  _adapter: HttpAdapter,
  nodeData: Omit<Node, 'createdAt' | 'modifiedAt'>
): Promise<Node> {
  // In-memory mode: use TestNodeBuilder
  return new TestNodeBuilder()
    .withId(nodeData.id)
    .withType(nodeData.nodeType as 'text' | 'task' | 'date')
    .withContent(nodeData.content)
    .withVersion(nodeData.version ?? 1)
    .withProperties(nodeData.properties ?? {})
    .withEmbedding(nodeData.embeddingVector ?? null)
    .withMentions(nodeData.mentions ?? [])
    .buildWithTimestamps();
}

/**
 * Checks if the HTTP dev server is running - NO LONGER FUNCTIONAL
 * @deprecated HTTP dev server testing was removed in Issue #558
 */
export async function checkServerHealth(_adapter: HttpAdapter): Promise<void> {
  if (shouldUseDatabase()) {
    throw new Error(
      '[DEPRECATED] HTTP dev server testing was removed in Issue #558.\n' +
      'Tests should use in-memory mode (default) or be rewritten for Tauri integration.'
    );
  }
  // In-memory mode: no-op
}

/**
 * Checks if an error indicates an unavailable HTTP endpoint
 */
export function skipIfEndpointUnavailable(error: unknown, endpointName: string): boolean {
  const errorMessage = error instanceof Error ? error.message : String(error);
  if (errorMessage.includes('405') || errorMessage.includes('500') || errorMessage.includes('DEPRECATED')) {
    console.log(`[Test] ${endpointName} not available - test skipped`);
    return true;
  }
  return false;
}

/**
 * Creates a node using the appropriate method based on current test mode.
 * Now always uses in-memory mode since HTTP dev server was removed.
 */
export async function createNodeForCurrentMode(
  _adapter: HttpAdapter,
  nodeData: {
    id: string;
    nodeType:
      | 'text'
      | 'task'
      | 'date'
      | 'header'
      | 'code-block'
      | 'quote-block'
      | 'ordered-list'
      | 'ai-chat';
    content: string;
    version?: number;
    properties: Record<string, unknown>;
    embeddingVector: number[] | null;
    mentions: string[];
  }
): Promise<Node> {
  // Always use in-memory mode since HTTP dev server was removed
  return new TestNodeBuilder()
    .withId(nodeData.id)
    .withType(nodeData.nodeType)
    .withContent(nodeData.content)
    .withVersion(nodeData.version ?? 1)
    .withProperties(nodeData.properties)
    .withEmbedding(nodeData.embeddingVector)
    .withMentions(nodeData.mentions)
    .buildWithTimestamps();
}
