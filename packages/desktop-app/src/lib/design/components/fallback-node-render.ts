/**
 * Fallback node rendering helpers
 *
 * Pure transforms used when a node's specialized plugin component has not finished
 * lazy-loading and the viewer renders a plain BaseNode instead. They reproduce the
 * syntax-stripping and metadata the specialized component would normally apply, so
 * view mode never shows raw syntax markers (e.g. code fences).
 *
 * Also includes code-block content normalization used when converting a node to a
 * code-block via slash command or pattern detection.
 */

import { isA } from '$lib/types/core-node-types';

/**
 * Normalize content for code-block conversion by adding a closing fence if missing.
 * Handles the pattern where a user types "```\n" before existing content.
 */
export function normalizeCodeBlockContent(content: string | undefined): string | undefined {
  if (content && !content.endsWith('```')) {
    return content + '\n```';
  }
  return content;
}

/**
 * Extract display content for fallback BaseNode rendering (when the plugin component
 * hasn't loaded yet). Strips syntax markers that the specialized component would
 * normally strip. Returns undefined when no stripping is needed for the type.
 */
export function extractFallbackDisplayContent(
  content: string,
  nodeType: string
): string | undefined {
  if (isA(nodeType, 'code-block')) {
    // Strip code fence markers for view mode (matches code-block-node.svelte logic)
    // Replace ```language with empty, keep content, replace closing ``` with newline
    return content.replace(/^```\w*/, '').replace(/```$/, '\n');
  }

  if (isA(nodeType, 'header')) {
    // Strip leading # symbols for header display (matches header node display)
    return content.replace(/^#+\s*/, '');
  }

  if (isA(nodeType, 'quote-block')) {
    // Strip leading > for quote blocks
    return content.replace(/^>\s*/, '');
  }

  // No stripping needed for other types
  return undefined;
}

/**
 * Get fallback metadata for BaseNode when the plugin component hasn't loaded yet.
 * Provides essential flags like disableMarkdown for code-blocks.
 */
export function extractFallbackMetadata(
  nodeType: string,
  properties: Record<string, unknown> | undefined
): Record<string, unknown> {
  const base = properties || {};

  // Code blocks should not process markdown
  return isA(nodeType, 'code-block') ? { ...base, disableMarkdown: true } : base;
}
