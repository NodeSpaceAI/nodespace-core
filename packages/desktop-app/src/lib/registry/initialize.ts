/**
 * Registry Initialization
 *
 * Register built-in node types with the experimental registry.
 * This doesn't affect any existing functionality.
 */

import { basicNodeTypeRegistry } from './basicNodeTypeRegistry.js';
import { textNodeType, taskNodeType, aiChatNodeType } from './nodeTypes.js';

let initialized = false;

/**
 * Initialize the experimental node type registry
 * Safe to call multiple times
 */
export function initializeBasicRegistry(): void {
  if (initialized) {
    return;
  }

  // Register built-in node types
  basicNodeTypeRegistry.register(textNodeType);
  basicNodeTypeRegistry.register(taskNodeType);
  basicNodeTypeRegistry.register(aiChatNodeType);

  initialized = true;

  // Log for debugging (can be removed later)
  console.log('[NodeTypeRegistry] Initialized with:', basicNodeTypeRegistry.getStats());
}

/**
 * Get registry statistics for debugging
 */
export function getRegistryStats() {
  return basicNodeTypeRegistry.getStats();
}
