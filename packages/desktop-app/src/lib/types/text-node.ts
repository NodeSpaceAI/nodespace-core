/**
 * Type-Safe TextNode Wrapper
 *
 * Provides type narrowing for text nodes while maintaining
 * the universal Node storage model.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { TextNode, isTextNode } from '$lib/types/text-node';
 *
 * // Type guard
 * if (isTextNode(node)) {
 *   console.log('This is a text node');
 * }
 * ```
 */

import type { Node } from './node';

/**
 * TextNode interface extending base Node
 *
 * Represents a simple text node - the most common node type.
 */
export interface TextNode extends Node {
  nodeType: 'text';
}

/**
 * Type guard to check if a node is a text node
 *
 * @param node - Node to check
 * @returns True if node is a text node
 *
 * @example
 * ```typescript
 * if (isTextNode(node)) {
 *   // TypeScript knows node is TextNode here
 *   console.log('Text content:', node.content);
 * }
 * ```
 */
export function isTextNode(node: Node): node is TextNode {
  return node.nodeType === 'text';
}

/**
 * Helper namespace for text node operations
 */
export const TextNodeHelpers = {
  /**
   * Check if node is a text node
   */
  isTextNode,

  /**
   * Create a new text node with specified content
   *
   * @param content - The text content
   * @returns New text node
   */
  createTextNode(content: string): TextNode {
    return {
      id: `text-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'text',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Check if text is empty or whitespace only
   *
   * @param node - Text node to check
   * @returns True if content is empty or whitespace
   */
  isEmpty(node: TextNode): boolean {
    return node.content.trim().length === 0;
  },

  /**
   * Get word count for a text node
   *
   * @param node - Text node
   * @returns Number of words in the text
   */
  getWordCount(node: TextNode): number {
    if (node.content.trim().length === 0) return 0;
    return node.content.trim().split(/\s+/).length;
  }
};
