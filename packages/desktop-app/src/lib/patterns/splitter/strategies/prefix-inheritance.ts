/**
 * Prefix Inheritance Splitting Strategy
 *
 * Used for node types that have a prefix that should be inherited on new lines:
 * - Headers: "# ", "## ", "### ", etc.
 * - Ordered lists: "1. "
 * - Quote blocks: "> "
 *
 * Behavior:
 * - Cursor at start or within prefix: Create empty node with prefix, preserve original
 * - Cursor after prefix: Split and add prefix to new node
 */

import type { PatternTemplate, SplitResult, SplittingStrategyImpl } from '../../types';

export class PrefixInheritanceStrategy implements SplittingStrategyImpl {
  split(content: string, position: number, pattern: PatternTemplate): SplitResult {
    // Extract the prefix to inherit
    const prefix = this.extractPrefix(content, pattern);
    const prefixLength = prefix.length;

    // Case 1: Cursor at or before start of content
    if (position <= 0) {
      return {
        beforeContent: prefix,
        afterContent: content,
        newNodeCursorPosition: prefixLength
      };
    }

    // Case 2: Cursor within the prefix area
    if (position <= prefixLength) {
      return {
        beforeContent: prefix,
        afterContent: content,
        newNodeCursorPosition: prefixLength
      };
    }

    // Case 3: Cursor after the prefix - split normally and inherit prefix in new content
    const beforeCursor = content.substring(0, position);
    const afterCursor = content.substring(position);

    return {
      beforeContent: beforeCursor,
      afterContent: prefix + afterCursor,
      newNodeCursorPosition: prefixLength
    };
  }

  /**
   * Extract the prefix from content
   * Uses the pattern regex to identify the prefix
   */
  private extractPrefix(content: string, pattern: PatternTemplate): string {
    // If prefixToInherit is explicitly provided, use it
    if (pattern.prefixToInherit) {
      return pattern.prefixToInherit;
    }

    // Otherwise, try to extract from regex match
    const match = pattern.regex.exec(content);
    if (match && match[0]) {
      return match[0]; // Return the full matched prefix
    }

    // Fallback: return empty prefix
    return '';
  }
}
