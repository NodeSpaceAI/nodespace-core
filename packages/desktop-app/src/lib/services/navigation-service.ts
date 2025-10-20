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
import { addTab, setActiveTab, tabState } from '$lib/stores/navigation';
import { sharedNodeStore } from './shared-node-store';
import { get } from 'svelte/store';
import type { Node } from '$lib/types';

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
   * Fetches from backend if not in store
   */
  async resolveNodeTarget(nodeId: string): Promise<NavigationTarget | null> {
    // First check store (synchronous)
    let node = sharedNodeStore.getNode(nodeId);

    // If not in store, check if it's a date node (format: YYYY-MM-DD)
    const dateRegex = /^\d{4}-\d{2}-\d{2}$/;
    const isDateNode = dateRegex.test(nodeId);

    if (!node && isDateNode) {
      // Date nodes are virtual - they always exist even without database entry
      // Create a synthetic date node
      const date = new Date(nodeId);
      const now = new Date().toISOString();
      node = {
        id: nodeId,
        nodeType: 'date',
        content: '',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        properties: { date: date.toISOString() },
        embeddingVector: null,
        createdAt: now,
        modifiedAt: now
      } as Node;
      console.log(`[NavigationService] Created synthetic date node for ${nodeId}`);
    } else if (!node) {
      // For non-date nodes, fetch from backend
      console.log(`[NavigationService] Node ${nodeId} not in store, fetching from backend...`);
      const { backendAdapter } = await import('./backend-adapter');

      try {
        const fetchedNode = await backendAdapter.getNode(nodeId);
        if (!fetchedNode) {
          console.error(`[NavigationService] Node ${nodeId} not found in backend`);
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
        console.error(`[NavigationService] Failed to fetch node ${nodeId}:`, error);
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
      return this.formatDateTitle(date);
    }

    // For other nodes, use first line of content (max 40 chars)
    if (node.content && typeof node.content === 'string') {
      const firstLine = node.content.split('\n')[0].trim();
      return firstLine.length > 40 ? firstLine.substring(0, 37) + '...' : firstLine;
    }

    // Fallback to node type
    return `${node.nodeType} Node`;
  }

  private formatDateTitle(date: Date): string {
    const today = new Date();
    const tomorrow = new Date(today);
    tomorrow.setDate(tomorrow.getDate() + 1);
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);

    const normalizeDate = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
    const targetDate = normalizeDate(date);
    const todayNormalized = normalizeDate(today);
    const tomorrowNormalized = normalizeDate(tomorrow);
    const yesterdayNormalized = normalizeDate(yesterday);

    if (targetDate.getTime() === todayNormalized.getTime()) return 'Today';
    if (targetDate.getTime() === tomorrowNormalized.getTime()) return 'Tomorrow';
    if (targetDate.getTime() === yesterdayNormalized.getTime()) return 'Yesterday';

    const year = targetDate.getFullYear();
    const month = String(targetDate.getMonth() + 1).padStart(2, '0');
    const day = String(targetDate.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
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

    // Check if tab already exists for this node
    const currentState = get(tabState);

    // Special case: Check if clicking today's date and "Today" tab exists
    const todayDateId = new Date().toISOString().split('T')[0]; // YYYY-MM-DD
    const isTodayDate = nodeId === todayDateId;
    const todayTab = isTodayDate ? currentState.tabs.find((tab) => tab.id === 'today') : null;

    if (todayTab && !openInNewTab) {
      // Switch to existing "Today" tab
      setActiveTab(todayTab.id);
      console.log(`[NavigationService] Switched to existing "Today" tab`);
      return;
    }

    // Check for existing node-based tabs
    const existingTab = currentState.tabs.find(
      (tab) => tab.content && (tab.content as { nodeId?: string }).nodeId === nodeId
    );

    if (existingTab && !openInNewTab) {
      // Switch to existing tab
      setActiveTab(existingTab.id);
      console.log(`[NavigationService] Switched to existing tab: ${existingTab.title}`);
      return;
    }

    // Create new tab
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
    console.log(`[NavigationService] Created new tab: ${newTab.title}`);
  }
}

/**
 * Lazy initialization getter function (NOT module-level singleton export)
 * This avoids triggering dependency chains during module import
 */
export function getNavigationService(): NavigationService {
  return NavigationService.getInstance();
}
