import { isA } from '$lib/types/core-node-types';

/**
 * Node types that are created via pattern conversion (e.g., "> text" → quote-block)
 * These types require atomic batching to prevent race conditions between content and nodeType updates
 */
const PATTERN_CONVERTED_NODE_TYPES = ['quote-block', 'code-block', 'ordered-list'] as const;

/**
 * Check if a node type requires atomic batching for pattern conversions
 * @param nodeType - Node type to check
 * @returns true if the node type requires batching (was created via pattern conversion)
 */
export function requiresAtomicBatching(nodeType: string): boolean {
  return PATTERN_CONVERTED_NODE_TYPES.some((type) => isA(nodeType, type));
}
