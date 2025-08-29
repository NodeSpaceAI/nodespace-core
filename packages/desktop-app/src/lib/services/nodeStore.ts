/**
 * Node State Store
 *
 * Centralized reactive state management for nodes using Svelte's writable stores.
 * This provides proper reactivity outside of .svelte components.
 */

import { writable, derived } from 'svelte/store';
import type { Node } from './nodeManager';

// Core reactive state stores
export const nodes = writable<Map<string, Node>>(new Map());
export const rootNodeIds = writable<string[]>([]);

// Derived store for visible nodes with hierarchy traversal
export const visibleNodes = derived([nodes, rootNodeIds], ([$nodes, $rootNodeIds]) => {
  // Recursive helper function to get visible nodes with depth
  const getVisibleNodesRecursive = (nodeIds: string[], depth: number = 0): Node[] => {
    const result: Node[] = [];
    for (const nodeId of nodeIds) {
      const node = $nodes.get(nodeId);
      if (node) {
        // Add hierarchy depth to node for CSS indentation
        const nodeWithDepth = { ...node, hierarchyDepth: depth };
        result.push(nodeWithDepth);

        // Include children if node is expanded
        if (node.expanded && node.children.length > 0) {
          result.push(...getVisibleNodesRecursive(node.children, depth + 1));
        }
      }
    }
    return result;
  };

  return getVisibleNodesRecursive($rootNodeIds);
});
