/**
 * Type-Safe QuoteBlock Node Wrapper
 *
 * Provides ergonomic, type-safe access to quote block node properties
 * while maintaining the universal Node storage model.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { QuoteBlockNode, isQuoteBlockNode } from '$lib/types/quote-block-node';
 *
 * // Type guard
 * if (isQuoteBlockNode(node)) {
 *   console.log('This is a quote block');
 * }
 * ```
 */

import type { Node } from './node';

/**
 * QuoteBlock node interface extending base Node
 *
 * Represents a quote block for displaying quoted text or citations.
 */
export interface QuoteBlockNode extends Node {
  nodeType: 'quote-block';
  properties: {
    [key: string]: unknown;
  };
}

/**
 * Type guard to check if a node is a quote block node
 *
 * @param node - Node to check
 * @returns True if node is a quote block node
 *
 * @example
 * ```typescript
 * if (isQuoteBlockNode(node)) {
 *   // TypeScript knows node is QuoteBlockNode here
 *   console.log('Quote content:', node.content);
 * }
 * ```
 */
export function isQuoteBlockNode(node: Node): node is QuoteBlockNode {
  return node.nodeType === 'quote-block';
}

/**
 * Helper namespace for quote block node operations
 *
 * Provides utility functions for working with quote block nodes.
 */
export const QuoteBlockNodeHelpers = {
  /**
   * Check if node is a quote block
   */
  isQuoteBlockNode,

  /**
   * Create a new quote block node with specified content
   *
   * @param content - The quoted text content
   * @param parentId - Optional parent node ID
   * @returns New quote block node
   *
   * @example
   * ```typescript
   * const quote = QuoteBlockNodeHelpers.createQuoteBlock(
   *   'To be or not to be, that is the question.',
   *   'parent-123'
   * );
   * ```
   */
  createQuoteBlock(content: string, parentId: string | null = null): QuoteBlockNode {
    return {
      id: `quote-block-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'quote-block',
      content,
      parentId,
      containerNodeId: null,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Check if quote content is multiline
   *
   * @param node - Quote block node to check
   * @returns True if content contains multiple lines
   *
   * @example
   * ```typescript
   * QuoteBlockNodeHelpers.isMultiline(quoteNode); // true if has newlines
   * ```
   */
  isMultiline(node: QuoteBlockNode): boolean {
    return node.content.includes('\n');
  },

  /**
   * Get the first line of a quote (useful for previews)
   *
   * @param node - Quote block node
   * @returns First line of the quote content
   *
   * @example
   * ```typescript
   * const firstLine = QuoteBlockNodeHelpers.getFirstLine(quoteNode);
   * console.log('Preview:', firstLine);
   * ```
   */
  getFirstLine(node: QuoteBlockNode): string {
    const lines = node.content.split('\n');
    return lines[0] || '';
  },

  /**
   * Get line count for a quote block
   *
   * @param node - Quote block node
   * @returns Number of lines in the quote
   *
   * @example
   * ```typescript
   * const lines = QuoteBlockNodeHelpers.getLineCount(quoteNode);
   * console.log(`Quote has ${lines} lines`);
   * ```
   */
  getLineCount(node: QuoteBlockNode): number {
    if (node.content.length === 0) return 0;
    return node.content.split('\n').length;
  }
};
