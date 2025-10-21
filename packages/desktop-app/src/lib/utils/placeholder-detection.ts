/**
 * Placeholder Detection Utility
 *
 * Determines if a node is a placeholder (has only type-specific prefix, no actual content).
 *
 * Placeholders exist in the UI to provide smooth UX when user creates new nodes,
 * but should not be persisted to the database until user adds actual content.
 *
 * This utility is shared between:
 * - SharedNodeStore (production persistence logic)
 * - Tests (verifying placeholder detection behavior)
 */

/**
 * Node types that are created via pattern conversion (e.g., "> text" → quote-block)
 * These types require atomic batching to prevent race conditions between content and nodeType updates
 */
export const PATTERN_CONVERTED_NODE_TYPES = ['quote-block', 'code-block', 'ordered-list'] as const;

/**
 * Type helper for pattern-converted node types
 */
export type PatternConvertedNodeType = (typeof PATTERN_CONVERTED_NODE_TYPES)[number];

/**
 * Check if a node type requires atomic batching for pattern conversions
 * @param nodeType - Node type to check
 * @returns true if the node type requires batching (was created via pattern conversion)
 */
export function requiresAtomicBatching(nodeType: string): boolean {
  return (PATTERN_CONVERTED_NODE_TYPES as readonly string[]).includes(nodeType);
}

/**
 * Minimal node interface for placeholder detection
 * Only requires the properties needed to determine placeholder status
 */
export interface PlaceholderCheckable {
  nodeType: string;
  content: string;
}

/**
 * Check if a node is a placeholder (has only type-specific prefix, no actual content)
 *
 * @param node - Node to check (must have nodeType and content properties)
 * @returns true if node is a placeholder (should not persist), false otherwise
 *
 * @example
 * ```typescript
 * // Text node with only whitespace
 * isPlaceholderNode({ nodeType: 'text', content: '' }) // true
 *
 * // Quote-block with only "> " prefix
 * isPlaceholderNode({ nodeType: 'quote-block', content: '> ' }) // true
 *
 * // Quote-block with actual content
 * isPlaceholderNode({ nodeType: 'quote-block', content: '> Hello world' }) // false
 *
 * // Ordered-list with only "1. " prefix
 * isPlaceholderNode({ nodeType: 'ordered-list', content: '1. ' }) // true
 *
 * // Ordered-list with actual content
 * isPlaceholderNode({ nodeType: 'ordered-list', content: '1. First item' }) // false
 * ```
 */
export function isPlaceholderNode(node: PlaceholderCheckable): boolean {
  const trimmedContent = node.content.trim();

  switch (node.nodeType) {
    case 'text': {
      // Text nodes: empty content is a placeholder
      if (trimmedContent === '') {
        return true;
      }

      // Text nodes that contain ONLY pattern prefixes are also placeholders
      // These are text nodes in the process of being converted to specialized types
      // Check for common patterns: "> " (quote), "# " (header), "```" (code), "1. " (ordered list)
      if (
        trimmedContent === '>' ||
        trimmedContent.match(/^>\s*$/) || // "> " or ">  " etc
        trimmedContent.match(/^#{1,6}\s*$/) || // "# " or "## " etc
        trimmedContent.match(/^```\w*\s*$/) || // "```" or "```js " etc
        trimmedContent.match(/^\d+\.\s*$/) // "1. " or "2. " etc
      ) {
        return true;
      }

      return false;
    }

    case 'quote-block': {
      // Quote-block nodes: only "> " prefix (no actual content after) is a placeholder
      // Strip "> " or ">" from all lines and check if any content remains
      const contentWithoutPrefix = trimmedContent
        .split('\n')
        .map((line) => line.replace(/^>\s?/, ''))
        .join('\n')
        .trim();
      return contentWithoutPrefix === '';
    }

    case 'header': {
      // Header nodes: only "# " prefix (no actual content after) is a placeholder
      // Strip hashtags and space, check if content remains
      const contentWithoutHashtags = trimmedContent.replace(/^#{1,6}\s*/, '');
      return contentWithoutHashtags === '';
    }

    case 'code-block': {
      // Code-block nodes: only "```" prefix (no actual code) is a placeholder
      // Strip backticks and language identifier, check if code remains
      const contentWithoutBackticks = trimmedContent.replace(/^```\w*\s*/, '').replace(/```$/, '');
      return contentWithoutBackticks.trim() === '';
    }

    case 'ordered-list': {
      // Ordered-list nodes: only "1. " prefix (no actual content after) is a placeholder
      // Strip "1. " prefix and check if content remains
      const contentWithoutPrefix = trimmedContent.replace(/^1\.\s*/, '');
      return contentWithoutPrefix === '';
    }

    case 'task':
      // Task nodes: empty content (regardless of checkbox) is a placeholder
      // The backend validates task description separately
      return trimmedContent === '';

    default:
      // For unknown node types, use simple empty check
      return trimmedContent === '';
  }
}
