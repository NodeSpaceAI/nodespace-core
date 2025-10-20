/**
 * NavigationService - Handles node:// link navigation and tab creation
 *
 * CRITICAL: Uses lazy initialization pattern (getter function) to avoid
 * module-level singleton exports that cause app freeze during initialization.
 *
 * Architecture Decision (from PR #306 learnings):
 * - ❌ BAD: export const navigationService = NavigationService.getInstance()
 * - ✅ GOOD: export function getNavigationService(): NavigationService
 *
 * This service:
 * - Resolves node UUIDs to node types using SharedNodeStore (synchronous)
 * - Creates or switches to tabs dynamically
 * - Generates human-readable tab titles from node content
 */

import { v4 as uuidv4 } from 'uuid';
import { addTab, tabState, updateTabContent } from '$lib/stores/navigation';
import { sharedNodeStore } from './shared-node-store';
import { get } from 'svelte/store';
import type { Node } from '$lib/types';
import { formatDateTitle, isValidDateString } from '$lib/utils/date-formatting';

// Constants
const MAX_TAB_TITLE_LENGTH = 40;
const LOG_PREFIX = '[NavigationService]';

export interface NavigationTarget {
  nodeId: string;
  nodeType: string;
  title: string;
}

export class NavigationService {
  private static instance: NavigationService | null = null;

  private constructor() {}

  static getInstance(): NavigationService {
    if (!NavigationService.instance) {
      NavigationService.instance = new NavigationService();
    }
    return NavigationService.instance;
  }

  /**
   * Resolve a node UUID to navigation target information
   *
   * Date nodes (YYYY-MM-DD format) are virtual and don't need to exist in the store.
   * They are created on-demand when children are added.
   *
   * Regular nodes are fetched from store (sync) or backend (async) if not in store.
   */
  async resolveNodeTarget(nodeId: string): Promise<NavigationTarget | null> {
    // Check if it's a date node (format: YYYY-MM-DD with semantic validation)
    if (isValidDateString(nodeId)) {
      // Date nodes are virtual - they don't need to exist in database
      // DateNodeViewer will use getNodesForParent(nodeId) to fetch children
      const date = new Date(nodeId);
      console.log(`${LOG_PREFIX} Virtual date node: ${nodeId}`);

      return {
        nodeId: nodeId,
        nodeType: 'date',
        title: formatDateTitle(date)
      };
    }

    // For regular nodes, check store first (synchronous)
    let node = sharedNodeStore.getNode(nodeId);

    if (!node) {
      // Not in store, fetch from backend
      console.log(`${LOG_PREFIX} Node ${nodeId} not in store, fetching from backend...`);
      const { backendAdapter } = await import('./backend-adapter');

      try {
        const fetchedNode = await backendAdapter.getNode(nodeId);
        if (!fetchedNode) {
          console.error(`${LOG_PREFIX} Node ${nodeId} not found in backend`);
          return null;
        }
        node = fetchedNode;

        // Add to store for future use
        sharedNodeStore.setNode(
          node,
          { type: 'external', source: 'navigation-service', description: 'fetched-for-link-click' },
          true // skipPersistence - already in backend
        );
      } catch (error) {
        console.error(`${LOG_PREFIX} Failed to fetch node ${nodeId}:`, error);
        return null;
      }
    }

    return {
      nodeId: node.id,
      nodeType: node.nodeType,
      title: this.generateTabTitle(node)
    };
  }

  /**
   * Generate a human-readable tab title from node content
   */
  private generateTabTitle(node: Node): string {
    // For date nodes, use formatted date
    if (node.nodeType === 'date') {
      const dateValue =
        node.properties && typeof node.properties === 'object' && 'date' in node.properties
          ? node.properties.date
          : Date.now();
      const date = new Date(dateValue as string | number);
      return formatDateTitle(date);
    }

    // For other nodes, use first line of content (max MAX_TAB_TITLE_LENGTH chars)
    if (node.content && typeof node.content === 'string') {
      const firstLine = node.content.split('\n')[0].trim();
      if (firstLine.length > MAX_TAB_TITLE_LENGTH) {
        return firstLine.substring(0, MAX_TAB_TITLE_LENGTH - 3) + '...';
      }
      return firstLine;
    }

    // Fallback to node type
    return `${node.nodeType} Node`;
  }

  /**
   * Navigate to a node by UUID
   * @param nodeId - The UUID of the node to navigate to
   * @param openInNewTab - If true, always create a new tab. If false, switch to existing tab if present.
   */
  async navigateToNode(nodeId: string, openInNewTab: boolean = false): Promise<void> {
    const target = await this.resolveNodeTarget(nodeId);

    if (!target) {
      // Error already logged in resolveNodeTarget
      return;
    }

    const currentState = get(tabState);

    if (openInNewTab) {
      // Cmd+Click: Always create new tab
      const newTab = {
        id: uuidv4(),
        title: target.title,
        type: 'node' as const,
        content: {
          nodeId: target.nodeId,
          nodeType: target.nodeType
        },
        closeable: true
      };

      addTab(newTab);
      return;
    }

    // Regular click: Navigate within current tab
    // Update the active tab's content to show the clicked node
    const activeTabId = currentState.activeTabId;

    updateTabContent(activeTabId, {
      nodeId: target.nodeId,
      nodeType: target.nodeType
    });
  }
}

/**
 * Lazy initialization getter function (NOT module-level singleton export)
 * This avoids triggering dependency chains during module import
 */
export function getNavigationService(): NavigationService {
  return NavigationService.getInstance();
}
