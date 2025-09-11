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

// Cache to maintain node object identity and prevent unnecessary re-renders
const nodeDepthCache = new Map<string, Node & { hierarchyDepth: number }>();

// Force cache invalidation for complex operations that affect multiple nodes
 
let _cacheVersion = 0; // Incremented by invalidateNodeCache to force cache clearing

// Function to invalidate cache for complex operations (indent/outdent/hierarchy changes)
export function invalidateNodeCache() {
  _cacheVersion++; // Force cache recreation by changing version
  nodeDepthCache.clear();
}

// Derived store for visible nodes with hierarchy traversal
export const visibleNodes = derived([nodes, rootNodeIds], ([$nodes, $rootNodeIds]) => {
  // Recursive helper function to get visible nodes with depth
  const getVisibleNodesRecursive = (nodeIds: string[], depth: number = 0): (Node & { hierarchyDepth: number })[] => {
    const result: (Node & { hierarchyDepth: number })[] = [];
    for (const nodeId of nodeIds) {
      const node = $nodes.get(nodeId);
      if (node) {
        // Check if we have a cached version with the same depth and cache version
        const cacheKey = `${nodeId}-${depth}-${_cacheVersion}`;
        let cachedNode = nodeDepthCache.get(cacheKey);
        
        // Create new cached node only if needed (new node, content, expanded state, or children changed)
        const childrenChanged = !cachedNode || 
          cachedNode.children.length !== node.children.length ||
          !cachedNode.children.every((id, index) => id === node.children[index]);
          
        if (!cachedNode || 
            cachedNode.content !== node.content || 
            cachedNode.expanded !== node.expanded ||
            childrenChanged) {
          cachedNode = { ...node, hierarchyDepth: depth };
          nodeDepthCache.set(cacheKey, cachedNode);
          
          // Clean up old cache entries for this node at different depths
          for (const [key] of nodeDepthCache) {
            if (key.startsWith(`${nodeId}-`) && key !== cacheKey) {
              nodeDepthCache.delete(key);
            }
          }
        }
        
        result.push(cachedNode);

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
