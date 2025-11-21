/**
 * Type-Safe OrderedList Node Wrapper
 *
 * Provides ergonomic, type-safe access to ordered list node properties
 * while maintaining the universal Node storage model.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { OrderedListNode, isOrderedListNode } from '$lib/types/ordered-list-node';
 *
 * // Type guard
 * if (isOrderedListNode(node)) {
 *   console.log('This is an ordered list item');
 * }
 * ```
 */

import type { Node } from './node';

/**
 * OrderedList node interface extending base Node
 *
 * Represents an item in an ordered (numbered) list.
 */
export interface OrderedListNode extends Node {
  nodeType: 'ordered-list';
  properties: {
    [key: string]: unknown;
  };
}

/**
 * Type guard to check if a node is an ordered list node
 *
 * @param node - Node to check
 * @returns True if node is an ordered list node
 *
 * @example
 * ```typescript
 * if (isOrderedListNode(node)) {
 *   // TypeScript knows node is OrderedListNode here
 *   console.log('List item:', node.content);
 * }
 * ```
 */
export function isOrderedListNode(node: Node): node is OrderedListNode {
  return node.nodeType === 'ordered-list';
}

/**
 * Helper namespace for ordered list node operations
 *
 * Provides utility functions for working with ordered list nodes.
 */
export const OrderedListNodeHelpers = {
  /**
   * Check if node is an ordered list
   */
  isOrderedListNode,

  /**
   * Create a new ordered list node with specified content
   *
   * @param content - The list item content
   * @param parentId - Optional parent node ID
   * @returns New ordered list node
   *
   * @example
   * ```typescript
   * const item = OrderedListNodeHelpers.createOrderedListItem(
   *   'First step in the process',
   *   'parent-123'
   * );
   * ```
   */
  createOrderedListItem(content: string): OrderedListNode {
    return {
      id: `ordered-list-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'ordered-list',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Check if list item content is multiline
   *
   * @param node - Ordered list node to check
   * @returns True if content contains multiple lines
   *
   * @example
   * ```typescript
   * OrderedListNodeHelpers.isMultiline(listNode); // true if has newlines
   * ```
   */
  isMultiline(node: OrderedListNode): boolean {
    return node.content.includes('\n');
  },

  /**
   * Get the first line of a list item (useful for previews)
   *
   * @param node - Ordered list node
   * @returns First line of the list item content
   *
   * @example
   * ```typescript
   * const firstLine = OrderedListNodeHelpers.getFirstLine(listNode);
   * console.log('Preview:', firstLine);
   * ```
   */
  getFirstLine(node: OrderedListNode): string {
    const lines = node.content.split('\n');
    return lines[0] || '';
  },

  /**
   * Get line count for an ordered list item
   *
   * @param node - Ordered list node
   * @returns Number of lines in the list item
   *
   * @example
   * ```typescript
   * const lines = OrderedListNodeHelpers.getLineCount(listNode);
   * console.log(`List item has ${lines} lines`);
   * ```
   */
  getLineCount(node: OrderedListNode): number {
    if (node.content.length === 0) return 0;
    return node.content.split('\n').length;
  },

  /**
   * Extract leading number from content if present
   *
   * @param content - List item content
   * @returns Extracted number or null if not found
   *
   * @example
   * ```typescript
   * OrderedListNodeHelpers.extractNumber('1. First item'); // 1
   * OrderedListNodeHelpers.extractNumber('42. Answer'); // 42
   * OrderedListNodeHelpers.extractNumber('No number here'); // null
   * ```
   */
  extractNumber(content: string): number | null {
    const match = content.match(/^(\d+)\.\s/);
    return match ? parseInt(match[1], 10) : null;
  },

  /**
   * Check if content starts with a number prefix
   *
   * @param node - Ordered list node
   * @returns True if content starts with "N. "
   *
   * @example
   * ```typescript
   * OrderedListNodeHelpers.hasNumberPrefix(listNode); // true if starts with "1. "
   * ```
   */
  hasNumberPrefix(node: OrderedListNode): boolean {
    return /^\d+\.\s/.test(node.content);
  },

  /**
   * Remove number prefix from content if present
   *
   * @param content - List item content
   * @returns Content without number prefix
   *
   * @example
   * ```typescript
   * OrderedListNodeHelpers.removeNumberPrefix('1. First item'); // "First item"
   * OrderedListNodeHelpers.removeNumberPrefix('No prefix'); // "No prefix"
   * ```
   */
  removeNumberPrefix(content: string): string {
    return content.replace(/^\d+\.\s/, '');
  }
};
