/**
 * Node Search Service
 *
 * Provides filtering and search functionality for nodes, specifically
 * for @mention autocomplete and node discovery features.
 *
 * Extracted from base-node.svelte to provide:
 * - Testable, reusable search logic
 * - Centralized filter rules
 * - Easy extension for new features (e.g., date shortcuts)
 */

import type { Node } from '$lib/types/node';

/**
 * Options for filtering mentionable nodes
 */
export interface FilterMentionableNodesOptions {
  /**
   * Node types to exclude from results
   * @default ['date'] - Date nodes are excluded by default (accessible via date shortcuts)
   */
  excludeNodeTypes?: string[];
}

/**
 * NodeSearchService
 *
 * Static service class providing node filtering and search functionality.
 */
export class NodeSearchService {
  /**
   * Filter nodes for @mention autocomplete
   *
   * Filtering rules:
   * 1. Exclude date nodes (accessible via date shortcuts - Issue #201)
   * 2. Match query in title/content (case-insensitive)
   * 3. Include ALL non-text nodes regardless of hierarchy
   * 4. Include ONLY container text nodes (top-level)
   *
   * @param nodes - Array of nodes to filter
   * @param query - Search query string
   * @param options - Filtering options
   * @returns Filtered array of mentionable nodes
   *
   * @example
   * ```typescript
   * const nodes = Array.from(nodeManager.nodes.values());
   * const results = NodeSearchService.filterMentionableNodes(nodes, 'my search');
   * ```
   */
  static filterMentionableNodes(
    nodes: Node[],
    query: string,
    options: FilterMentionableNodesOptions = {}
  ): Node[] {
    const excludeTypes = options.excludeNodeTypes || ['date'];

    return nodes
      .filter((node) => !excludeTypes.includes(node.nodeType))
      .filter((node) => this.matchesQuery(node, query))
      .filter((node) => this.isMentionable(node));
  }

  /**
   * Check if a node matches the search query
   *
   * Searches both the first line (title) and full content of the node.
   * Case-insensitive matching.
   *
   * @param node - Node to check
   * @param query - Search query string
   * @returns True if node matches query
   *
   * @private
   */
  private static matchesQuery(node: Node, query: string): boolean {
    // Extract first line as title
    const title = node.content.split('\n')[0] || '';

    // Case-insensitive search in title and content
    const lowerQuery = query.toLowerCase();
    return (
      title.toLowerCase().includes(lowerQuery) || node.content.toLowerCase().includes(lowerQuery)
    );
  }

  /**
   * Check if a node is mentionable in autocomplete
   *
   * Rules:
   * - Non-text nodes (tasks, persons, etc.): Always mentionable
   * - Text nodes: Only containers (top-level, not nested content)
   *
   * Rationale: Text node children are part of their parent's content,
   * so we only show the container nodes to avoid duplication.
   *
   * @param node - Node to check
   * @returns True if node can be mentioned
   *
   * @private
   */
  private static isMentionable(node: Node): boolean {
    // Non-text nodes: always mentionable
    if (node.nodeType !== 'text') {
      return true;
    }

    // Text nodes: only containers (containerNodeId === null)
    return node.containerNodeId === null;
  }
}
