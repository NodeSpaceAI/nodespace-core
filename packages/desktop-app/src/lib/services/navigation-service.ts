/**
 * NavigationService - Handles nodespace:// link navigation and tab creation
 *
 * CRITICAL: Uses lazy initialization pattern (getter function) to avoid
 * module-level singleton exports that cause app freeze during initialization.
 *
 * Architecture Decision:
 * - ❌ BAD: export const navigationService = NavigationService.getInstance()
 * - ✅ GOOD: export function getNavigationService(): NavigationService
 *
 * This service:
 * - Resolves node UUIDs to node types using SharedNodeStore (synchronous)
 * - Creates or switches to tabs dynamically
 * - Generates human-readable tab titles from node content
 *
 * Three navigation entry points, differing in what they do to existing tabs:
 * `navigateToNode` re-points the current tab, `navigateToNodeInOtherPane`
 * always opens a new one beside it, and `focusOrOpenNode` reuses a tab already
 * showing the node. See each method for which to reach for.
 */

import { v4 as uuidv4 } from 'uuid';
import {
  addTab,
  navigationStore,
  updateTabContent,
  createPane,
  setActivePane,
  setActiveTab,
  DEFAULT_PANE_ID,
  LOADING_TAB_TITLE
} from '$lib/stores/navigation.svelte';
import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import type { Node } from '$lib/types';
import { formatDateTitle } from '$lib/utils/date-formatting';
import { formatTabTitle } from '$lib/utils/text-formatting';
import { createLogger } from '$lib/utils/logger';
import { rendersAsEntityRow, hasEntityNounName } from '$lib/design/components/node-type-predicates';
import { resolveTitleOrContent } from '$lib/utils/node-display-title';

const log = createLogger('NavigationService');

export interface NavigationTarget {
  nodeId: string;
  nodeType: string;
  title: string;
}

/**
 * Options for {@link NavigationService.focusOrOpenNode}.
 *
 * The three fields exist because the call sites genuinely differ, not to be
 * general: a schema opens under `nodeType: 'query'` so the tab routes to
 * QueryNodeViewer, the Daily Journal must not focus a non-date tab that happens
 * to share today's id, and most callers leave the title at its default while
 * the collection viewer already knows it.
 */
export interface FocusOrOpenOptions {
  /**
   * The `nodeType` written into the tab's content, which selects the viewer.
   * Often the node's own type, but deliberately an override for tabs that route
   * elsewhere (a schema id opened as `'query'`, a chat as `'ai-chat'`).
   */
  nodeType: string;
  /**
   * Title for a newly created tab. Defaults to `LOADING_TAB_TITLE`
   * (`navigation.svelte.ts`), a placeholder `computeTabTitle` (tab-title.ts)
   * replaces with a generic fallback once the node hydrates with no title of
   * its own. Pass a value only when the caller already knows the real title,
   * or has something better than the generic fallback to show meanwhile
   * (e.g. a related node's id) — computeTabTitle preserves any title other
   * than the exact default, even past hydration.
   */
  title?: string;
  /**
   * When true, an existing tab must also match `nodeType` to be focused.
   *
   * Off by default: a node id identifies one node, so any tab showing it is the
   * tab the user means. Daily Journal needs it because a date id is not a real
   * node id — nothing stops another tab from carrying the same string.
   *
   * This is a symptom rather than a feature: it exists because date ids are
   * bare `YYYY-MM-DD` strings sharing a namespace with nothing. If date-node
   * identity is ever fixed, this option loses its only caller and should go
   * with it. New callers should not need it.
   */
  matchNodeType?: boolean;
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
      const { backendAdapter } = await import('./backend-adapter');

      try {
        // ADR-053: capture the database generation before the daemon read so a
        // switch mid-fetch drops the write below.
        const epoch = sharedNodeStore.currentEpoch();
        const fetchedNode = await backendAdapter.getNode(nodeId);
        if (!fetchedNode) {
          log.error(`Node ${nodeId} not found in backend`);
          return null;
        }
        node = fetchedNode;

        // The active database switched while this read was in flight — the
        // fetched row belongs to the previous database. Drop it and abort the
        // navigation rather than opening a tab for a node absent from the
        // now-active store.
        if (sharedNodeStore.currentEpoch() !== epoch) {
          return null;
        }

        // Add to store for future use.
        // Use type 'database' and skipPersistence since already in backend (or virtual)
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

    // For other nodes, prefer the computed title (from a title_template) over content — the
    // same rule every other title-resolving surface uses (resolveTitleOrContent), so a
    // title-template-driven type with a stale `content` (e.g. converted from another type)
    // doesn't leak that stale value here instead of falling through to the fallback below.
    const hasTitleTemplate = pluginRegistry.hasTitleTemplate(node.nodeType);
    const resolved = resolveTitleOrContent(node, hasTitleTemplate);
    if (resolved.trim()) {
      return formatTabTitle(resolved, `${node.nodeType} Node`);
    }

    // Untitled node: fall back to the plugin's display name (e.g. "Untitled Customer"),
    // rather than the raw type id ("customer Node"). Gated to types whose plugin `name` is a
    // genuine entity noun — entity rows by construction, plus any inline-editable type that is
    // nonetheless an entity noun (person — see hasEntityNounName's doc comment). Inline-
    // editable content primitives name their plugins for the registry, not the user ("Text
    // Node", "Header Node"), so using those here would render "Text Node" for an untitled text
    // node instead of the `<type> Node` fallback below.
    if (rendersAsEntityRow(node.nodeType) || hasEntityNounName(node.nodeType)) {
      const plugin = pluginRegistry.getPlugin(node.nodeType);
      if (plugin?.name) return `Untitled ${plugin.name}`;
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

    // Entity rows (no inline node component — they render read-only via the BaseNode
    // fallback) are standalone entities: open them directly, not their parent (e.g. the
    // date node they happen to sit under). Inline primitives (text, header, code-block)
    // fall through and walk up to the nearest viewer-owning ancestor.
    if (targetNode && rendersAsEntityRow(targetNode.nodeType)) {
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

    const currentState = navigationStore.state;

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
   * The pane a new tab should open into: the active one, or the first pane if
   * the active id is stale.
   *
   * Public because a caller that opens a non-node tab still needs it —
   * `openSearchTab` creates a `type: 'search'` tab with no `content`, so
   * {@link focusOrOpenNode}'s node lookup does not apply to it, but the pane
   * choice is the same question. Callers opening node tabs should not need
   * this; `focusOrOpenNode` resolves the pane itself.
   */
  targetPaneId(): string {
    const state = navigationStore.state;
    const paneExists = state.panes.some((pane) => pane.id === state.activePaneId);
    if (paneExists) return state.activePaneId;
    return state.panes[0]?.id ?? DEFAULT_PANE_ID;
  }

  /**
   * Focus the tab already showing a node; report whether one was found.
   *
   * Also the focus half of {@link focusOrOpenNode}, so the two cannot drift on
   * what counts as "already open". Called directly by the caller whose
   * "not open yet" branch is not a plain new tab — QueryNodeViewer opens a row
   * in the *other* pane, so the list stays visible beside it, and wants this
   * reuse check with its own opening behaviour.
   *
   * @param nodeId - Node whose tab to focus
   * @param nodeType - When given, the tab must also match this type. Omit for
   *   the usual case: a node id identifies one node, so any tab showing it is
   *   the tab the user means.
   * @returns true if a matching tab was found and focused
   */
  focusNodeTab(nodeId: string, nodeType?: string): boolean {
    const existingTab = navigationStore.state.tabs.find(
      (tab) =>
        tab.content?.nodeId === nodeId &&
        (nodeType === undefined || tab.content?.nodeType === nodeType)
    );
    if (!existingTab) return false;

    setActiveTab(existingTab.id, existingTab.paneId);
    return true;
  }

  /**
   * Focus the tab already showing a node, or open one if none exists.
   *
   * This is the "open it, but don't open it twice" behaviour behind clicking a
   * node in the sidebar, a search result, or a collection member. It differs
   * from the other two navigation methods in what it does to existing tabs:
   *
   * - {@link navigateToNode} re-points the *current* tab, replacing what the
   *   user was looking at.
   * - {@link navigateToNodeInOtherPane} always calls `addTab`, so clicking the
   *   same node twice stacks duplicates.
   * - This method reuses a matching tab wherever it lives, switching panes if
   *   the match is in the other one.
   *
   * Unlike the other two it does **not** resolve to a navigation ancestor: it
   * opens the node it was given. Callers pass ids they mean to open as tabs —
   * a schema, a chat, a collection member — not child nodes to scroll to.
   *
   * @param nodeId - Node to focus or open
   * @param options - Tab `nodeType`, optional `title`, optional `matchNodeType`
   */
  focusOrOpenNode(nodeId: string, options: FocusOrOpenOptions): void {
    const { nodeType, title = LOADING_TAB_TITLE, matchNodeType = false } = options;

    if (this.focusNodeTab(nodeId, matchNodeType ? nodeType : undefined)) return;

    addTab(
      {
        id: uuidv4(),
        title,
        type: 'node',
        content: { nodeId, nodeType },
        closeable: true,
        paneId: this.targetPaneId()
      },
      true
    );
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

    const currentState = navigationStore.state;
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
