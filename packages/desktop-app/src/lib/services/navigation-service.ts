/**
 * NavigationService - Handles nodespace:// link navigation and tab creation
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
import {
  addTab,
  tabState,
  updateTabContent,
  createPane,
  setActivePane
} from '$lib/stores/navigation';
import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { get } from 'svelte/store';
import type { Node } from '$lib/types';
import { formatDateTitle } from '$lib/utils/date-formatting';
import { formatTabTitle } from '$lib/utils/text-formatting';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('NavigationService');

/** Core built-in node types — everything else is a custom schema type */
const CORE_NODE_TYPES = new Set([
  'text', 'task', 'date', 'header', 'code-block', 'quote-block',
  'ordered-list', 'horizontal-line', 'table', 'checkbox', 'collection',
  'query', 'schema'
]);

function isCustomSchemaNodeType(nodeType: string): boolean {
  return !CORE_NODE_TYPES.has(nodeType);
}

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
   * All nodes (including virtual date nodes) are handled uniformly by the backend.
   * The backend returns virtual date nodes automatically for YYYY-MM-DD format IDs.
   *
   * Nodes are fetched from store (sync) or backend (async) if not in store.
   */
  async resolveNodeTarget(nodeId: string): Promise<NavigationTarget | null> {
    // Check store first (synchronous)
    let node = sharedNodeStore.getNode(nodeId);

    if (!node) {
      // Not in store, fetch from backend (handles virtual dates automatically)
      log.debug(`Node ${nodeId} not in store, fetching from backend...`);
      const { getNode } = await import('./tauri-commands');

      try {
        const fetchedNode = await getNode(nodeId);
        if (!fetchedNode) {
          log.error(`Node ${nodeId} not found in backend`);
          return null;
        }
        node = fetchedNode;

        // Add to store for future use
        // Use type 'database' and skipPersistence since already in backend (or virtual)
        // Date nodes are handled specially by ensureAncestorChainPersisted (skips them)
        sharedNodeStore.setNode(
          node,
          { type: 'database', reason: 'fetched-for-link-click' },
          true // skipPersistence - already in backend or virtual
        );
      } catch (error) {
        log.error(`Failed to fetch node ${nodeId}:`, error);
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
   * Generate tab title for a node
   *
   * Uses specialized formatting for date nodes, and shared formatTabTitle
   * utility for all other node types to ensure consistency.
   *
   * @param node - The node to generate a title for
   * @returns Human-readable tab title
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

    // For other nodes, prefer computed title (from title_template) over content
    if (node.title && typeof node.title === 'string' && node.title.trim()) {
      return formatTabTitle(node.title, `${node.nodeType} Node`);
    }

    if (node.content && typeof node.content === 'string' && node.content.trim()) {
      return formatTabTitle(node.content, `${node.nodeType} Node`);
    }

    // For custom schema types, use the plugin display name (e.g. "Customer")
    if (isCustomSchemaNodeType(node.nodeType)) {
      const plugin = pluginRegistry.getPlugin(node.nodeType);
      if (plugin?.name) return plugin.name;
    }

    // Fallback to node type
    return `${node.nodeType} Node`;
  }

  /**
   * Walk up the structureTree to find the navigation ancestor of a node.
   *
   * Entity nodes that have their own dedicated viewer (task, date, query, etc.)
   * are returned as-is — they should open in their own viewer, not be resolved
   * to a parent's viewer.
   *
   * Primitive child nodes (text, header, code-block) without dedicated viewers
   * walk up to the nearest viewer-owning ancestor or the tree root.
   *
   * Returns the original nodeId if it's a root node or has its own viewer.
   */
  private findNavigationAncestor(nodeId: string): string {
    if (!structureTree) return nodeId;

    // If the target node itself has a dedicated viewer, it should open in that viewer
    const targetNode = sharedNodeStore.getNode(nodeId);
    if (targetNode && pluginRegistry.hasViewer(targetNode.nodeType)) {
      return nodeId;
    }

    // Custom schema entity nodes are root-level entities —
    // always open them directly, not their parent (e.g. date node)
    if (targetNode && isCustomSchemaNodeType(targetNode.nodeType)) {
      return nodeId;
    }

    let currentId = nodeId;
    const visited = new Set<string>();

    while (true) {
      if (visited.has(currentId)) break; // cycle protection
      visited.add(currentId);

      const parentId = structureTree.getParent(currentId);
      if (!parentId || parentId === '__root__') break;
      currentId = parentId;

      // Stop at the first ancestor that has its own viewer
      const parentNode = sharedNodeStore.getNode(currentId);
      if (parentNode && pluginRegistry.hasViewer(parentNode.nodeType)) {
        break;
      }
    }

    return currentId;
  }

  /**
   * Scroll to a node element in the DOM after it renders.
   * Uses requestAnimationFrame + polling retries for async-loaded child nodes.
   */
  private scrollToNode(nodeId: string): void {
    const escapedId = CSS.escape(nodeId);
    const attemptScroll = () => {
      const el = document.querySelector(`[data-node-id="${escapedId}"]`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        return true;
      }
      return false;
    };

    // First attempt after current frame renders
    requestAnimationFrame(() => {
      if (attemptScroll()) return;

      // Retry with increasing delays for lazy-loaded content (100ms, 250ms, 500ms)
      const delays = [100, 250, 500];
      let attempt = 0;
      const retry = () => {
        if (attemptScroll() || attempt >= delays.length) return;
        setTimeout(retry, delays[attempt++]);
      };
      retry();
    });
  }

  /**
   * Resolve a navigation target, and if it's a non-root node, also resolve
   * the navigation ancestor's info for tab navigation. Returns both the original
   * target and the navigation-level ancestor info.
   */
  private async resolveWithNavigationAncestor(nodeId: string): Promise<{
    target: NavigationTarget;
    navNodeId: string;
    navNodeType: string;
    navTitle: string;
    isNonRoot: boolean;
  } | null> {
    const target = await this.resolveNodeTarget(nodeId);
    if (!target) return null;

    const ancestorId = this.findNavigationAncestor(target.nodeId);
    const isNonRoot = ancestorId !== target.nodeId;

    let navNodeId = target.nodeId;
    let navNodeType = target.nodeType;
    let navTitle = target.title;

    if (isNonRoot) {
      const ancestorTarget = await this.resolveNodeTarget(ancestorId);
      if (ancestorTarget) {
        navNodeId = ancestorTarget.nodeId;
        navNodeType = ancestorTarget.nodeType;
        navTitle = ancestorTarget.title;
      }
    }

    return { target, navNodeId, navNodeType, navTitle, isNonRoot };
  }

  /**
   * Navigate to a node by UUID
   *
   * For non-root nodes: resolves to the navigation ancestor, navigates to it,
   * then scrolls to the target child node's position in the viewer.
   *
   * @param nodeId - The UUID of the node to navigate to
   * @param openInNewTab - If true, always create a new tab. If false, switch to existing tab if present.
   * @param sourcePaneId - The pane ID where the click originated (optional, defaults to active pane)
   */
  async navigateToNode(
    nodeId: string,
    openInNewTab: boolean = false,
    sourcePaneId?: string,
    makeTabActive: boolean = true
  ): Promise<void> {
    const resolved = await this.resolveWithNavigationAncestor(nodeId);
    if (!resolved) return;

    const { target, navNodeId, navNodeType, navTitle, isNonRoot } = resolved;

    const currentState = get(tabState);

    if (openInNewTab) {
      // Cmd+Click: Always create new tab in the source pane (or active pane if no source provided)
      const targetPaneId = sourcePaneId ?? currentState.activePaneId;
      const newTab = {
        id: uuidv4(),
        title: navTitle,
        type: 'node' as const,
        content: {
          nodeId: navNodeId,
          nodeType: navNodeType
        },
        closeable: true,
        paneId: targetPaneId
      };

      addTab(newTab, makeTabActive);

      // Scroll to child after new tab renders
      if (isNonRoot) {
        this.scrollToNode(target.nodeId);
      }
      return;
    }

    // Regular click: Navigate within current tab
    const activeTabId = currentState.activeTabIds[currentState.activePaneId];
    const activeTab = currentState.tabs.find((t) => t.id === activeTabId);
    const currentViewNodeId = activeTab?.content?.nodeId;

    // Check if we're already viewing the ancestor node
    if (isNonRoot && currentViewNodeId === navNodeId) {
      // Already viewing the ancestor — just scroll to the target child (no reload)
      this.scrollToNode(target.nodeId);
      return;
    }

    // Navigate to the ancestor node (or direct node if already root)
    updateTabContent(activeTabId, {
      nodeId: navNodeId,
      nodeType: navNodeType
    });

    // Scroll to child after navigation renders
    if (isNonRoot) {
      this.scrollToNode(target.nodeId);
    }
  }

  /**
   * Navigate to a node in the other pane (Cmd+Shift+Click behavior)
   *
   * If only one pane exists:
   * - Creates a second pane (50/50 split)
   * - Opens the node in the new pane
   *
   * If two panes exist:
   * - Opens the node in the pane that is NOT the source pane
   * - Switches focus to that pane
   *
   * @param nodeId - The UUID of the node to navigate to
   * @param sourcePaneId - The pane ID where the click originated (optional, defaults to active pane)
   */
  async navigateToNodeInOtherPane(nodeId: string, sourcePaneId?: string): Promise<void> {
    const resolved = await this.resolveWithNavigationAncestor(nodeId);
    if (!resolved) return;

    const { target, navNodeId, navNodeType, navTitle, isNonRoot } = resolved;

    const currentState = get(tabState);
    // Use provided source pane, or fall back to active pane
    const currentPaneId = sourcePaneId ?? currentState.activePaneId;

    if (currentState.panes.length === 1) {
      // Create second pane (automatically sets 50/50 split)
      const newPane = createPane();

      if (!newPane) {
        log.error('Failed to create second pane (max panes reached)');
        return;
      }

      log.debug(`Created second pane: ${newPane.id}`);

      // Create tab in the new pane
      const newTab = {
        id: uuidv4(),
        title: navTitle,
        type: 'node' as const,
        content: {
          nodeId: navNodeId,
          nodeType: navNodeType
        },
        closeable: true,
        paneId: newPane.id
      };

      addTab(newTab);
      setActivePane(newPane.id);
    } else {
      // Two panes exist - open in the OTHER pane (not the active one)
      const otherPane = currentState.panes.find((p) => p.id !== currentPaneId);

      if (!otherPane) {
        log.error('Could not find other pane');
        return;
      }

      log.debug(`Opening in other pane: ${otherPane.id}`);

      // Create tab in the other pane
      const newTab = {
        id: uuidv4(),
        title: navTitle,
        type: 'node' as const,
        content: {
          nodeId: navNodeId,
          nodeType: navNodeType
        },
        closeable: true,
        paneId: otherPane.id
      };

      addTab(newTab);
      setActivePane(otherPane.id);
    }

    // Scroll to child after the other pane renders
    if (isNonRoot) {
      this.scrollToNode(target.nodeId);
    }
  }
}

/**
 * Lazy initialization getter function (NOT module-level singleton export)
 * This avoids triggering dependency chains during module import
 */
export function getNavigationService(): NavigationService {
  return NavigationService.getInstance();
}
