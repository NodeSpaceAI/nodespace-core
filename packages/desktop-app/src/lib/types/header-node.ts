/**
 * Type-Safe HeaderNode Wrapper
 *
 * Provides type narrowing for header nodes while maintaining
 * the universal Node storage model.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { HeaderNode, isHeaderNode, getHeaderLevel } from '$lib/types/header-node';
 *
 * // Type guard
 * if (isHeaderNode(node)) {
 *   const level = getHeaderLevel(node);
 *   console.log(`H${level}: ${node.content}`);
 * }
 * ```
 */

import type { Node } from './node';

/**
 * HeaderNode interface extending base Node
 *
 * Represents a header/heading node with level derived from content.
 * Level is determined by counting # characters at the start of content.
 */
export interface HeaderNode extends Node {
  nodeType: 'header';
}

/**
 * Type guard to check if a node is a header node
 *
 * @param node - Node to check
 * @returns True if node is a header node
 *
 * @example
 * ```typescript
 * if (isHeaderNode(node)) {
 *   // TypeScript knows node is HeaderNode here
 *   const level = getHeaderLevel(node);
 * }
 * ```
 */
export function isHeaderNode(node: Node): node is HeaderNode {
  return node.nodeType === 'header';
}

/**
 * Get the header level from content
 *
 * Parses the number of # characters at the start of content.
 * Returns 1-6 (standard HTML heading levels), defaulting to 1.
 *
 * @param node - Header node
 * @returns Header level (1-6)
 *
 * @example
 * ```typescript
 * // For node with content "## My Heading"
 * getHeaderLevel(headerNode); // 2
 * ```
 */
export function getHeaderLevel(node: HeaderNode): number {
  const match = node.content.match(/^(#{1,6})\s/);
  if (match) {
    return Math.min(match[1].length, 6);
  }
  return 1;
}

/**
 * Get the header text without the # prefix
 *
 * @param node - Header node
 * @returns Header text without markdown syntax
 *
 * @example
 * ```typescript
 * // For node with content "## My Heading"
 * getHeaderText(headerNode); // "My Heading"
 * ```
 */
export function getHeaderText(node: HeaderNode): string {
  return node.content.replace(/^#{1,6}\s*/, '');
}

/**
 * Set the header level (immutable)
 *
 * Returns a new node with the updated header level in content.
 * Original node is not modified.
 *
 * @param node - Header node
 * @param level - New header level (1-6)
 * @returns New node with updated level in content
 */
export function setHeaderLevel(node: HeaderNode, level: number): HeaderNode {
  const clampedLevel = Math.max(1, Math.min(6, level));
  const text = getHeaderText(node);
  const newContent = '#'.repeat(clampedLevel) + ' ' + text;
  return {
    ...node,
    content: newContent
  };
}

/**
 * Helper namespace for header node operations
 */
export const HeaderNodeHelpers = {
  isHeaderNode,
  getHeaderLevel,
  getHeaderText,
  setHeaderLevel,

  /**
   * Create a new header node with specified content and level
   *
   * @param text - The header text
   * @param level - Header level (1-6), defaults to 1
   * @returns New header node
   */
  createHeaderNode(text: string, level: number = 1): HeaderNode {
    const clampedLevel = Math.max(1, Math.min(6, level));
    return {
      id: `header-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'header',
      content: '#'.repeat(clampedLevel) + ' ' + text,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Check if header is a top-level heading (H1)
   */
  isTopLevel(node: HeaderNode): boolean {
    return getHeaderLevel(node) === 1;
  },

  /**
   * Check if header is a sub-heading (H2-H6)
   */
  isSubHeading(node: HeaderNode): boolean {
    return getHeaderLevel(node) > 1;
  }
};
