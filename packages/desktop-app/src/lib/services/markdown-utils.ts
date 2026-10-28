/**
 * Markdown Utilities
 *
 * Header-level detection and plain-text extraction for markdown content.
 */

/**
 * Header level (1-6) of a line starting with `#` syntax, or 0 when it isn't a header.
 * Requires the space after the hashes and does not trim, so `"# "` mid-typing counts.
 */
export function parseHeaderLevel(content: string): number {
  const match = content.match(/^(#{1,6})\s/);
  return match ? match[1].length : 0;
}

/**
 * Strip all markdown formatting (for plain text extraction)
 */
export function stripMarkdown(markdown: string): string {
  if (!markdown) return '';

  return (
    markdown
      // Remove headings
      .replace(/^#{1,6}\s+/gm, '')
      // Remove bold/italic
      .replace(/\*\*(.*?)\*\*/g, '$1')
      // Underscore emphasis only at word boundaries (snake_case and __init__.py survive)
      .replace(/(?<![\p{L}\p{N}_])__(.+?)__(?=[.,;:!?)\]}"']*(?:\s|$))/gu, '$1')
      .replace(/\*([^*]+)\*/g, '$1')
      .replace(/(?<![\p{L}\p{N}_])_([^_]+)_(?=[.,;:!?)\]}"']*(?:\s|$))/gu, '$1')
      // Remove inline code
      .replace(/`([^`]+)`/g, '$1')
      // Remove links
      .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
      // Normalize whitespace
      .replace(/\s+/g, ' ')
      .trim()
  );
}
