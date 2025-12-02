/**
 * View-to-Edit Position Mapper
 *
 * Maps character positions from view content (markdown syntax stripped)
 * to edit content (with markdown syntax visible).
 *
 * This is essential for click-to-cursor positioning where:
 * - View mode shows rendered text: "Hello world"
 * - Edit mode shows raw markdown: "**Hello** world"
 *
 * When user clicks on 'w' in view, we need to find corresponding position in edit.
 */

/**
 * Markdown syntax markers with their lengths
 * Order matters: longer markers (**, __) must be checked before shorter ones (*, _)
 */
const MARKDOWN_MARKERS: ReadonlyArray<readonly [string, number]> = [
  ['**', 2], // Bold (asterisk)
  ['__', 2], // Bold (underscore)
  ['~~', 2], // Strikethrough
  ['*', 1], // Italic (asterisk)
  ['_', 1], // Italic (underscore)
  ['`', 1] // Code
] as const;

/**
 * Check if the remaining string starts with any markdown marker
 * @param remaining - The substring to check
 * @returns Length of marker to skip, or 0 if no marker found
 */
function getMarkerLength(remaining: string): number {
  for (const [marker, length] of MARKDOWN_MARKERS) {
    if (remaining.startsWith(marker)) {
      return length;
    }
  }
  return 0;
}

/**
 * Get the length of any prefix syntax at the start of a line
 * @param line - The line to check
 * @returns Length of prefix syntax to skip, or 0 if no prefix found
 */
function getPrefixSyntaxLength(line: string): number {
  // Header prefixes: ### Header (1-6 hashes + space)
  const headerMatch = line.match(/^#{1,6}\s+/);
  if (headerMatch) {
    return headerMatch[0].length;
  }

  // Quote prefixes: > Quote (handle multiple levels like >> or > > >)
  const quoteMatch = line.match(/^(?:>\s*)+/);
  if (quoteMatch) {
    return quoteMatch[0].length;
  }

  // Ordered list prefixes: 1. Item, 2. Item, etc.
  const orderedListMatch = line.match(/^\d+\.\s+/);
  if (orderedListMatch) {
    return orderedListMatch[0].length;
  }

  // Unordered list prefixes: - Item, * Item
  const unorderedListMatch = line.match(/^[-*]\s+/);
  if (unorderedListMatch) {
    return unorderedListMatch[0].length;
  }

  return 0;
}

/**
 * Map character position from view content to edit content
 *
 * View content has markdown syntax stripped (e.g., "And again" from "### *And* again")
 * Edit content has markdown syntax visible (e.g., "### *And* again")
 *
 * This function accounts for BOTH prefix syntax (###, >, 1.) AND inline markers (**,*,~~,`).
 *
 * @param viewPosition - Character position in view content (0-based)
 * @param viewContent - The view content (all syntax stripped, from stripAllMarkdown)
 * @param editContent - The edit content (with syntax, from content prop)
 * @returns Character position in edit content (0-based)
 *
 * @example
 * // View: "And again" (click at position 0 = 'A')
 * // Edit: "### *And* again" (should position at 5 = 'A' after ### and *)
 * mapViewPositionToEditPosition(0, "And again", "### *And* again") // → 5
 *
 * @example
 * // View: "Hello world" (click at position 6 = 'w')
 * // Edit: "**Hello** world" (should position at 10 = 'w')
 * mapViewPositionToEditPosition(6, "Hello world", "**Hello** world") // → 10
 */
export function mapViewPositionToEditPosition(
  viewPosition: number,
  viewContent: string,
  editContent: string
): number {
  // Fast path: If no displayContent (view === edit), no mapping needed
  if (viewContent === editContent) {
    return viewPosition;
  }

  // First, skip any prefix syntax at the start of the edit content
  let editIndex = getPrefixSyntaxLength(editContent);

  // Edge case: position 0 - skip leading inline syntax markers after prefix
  if (viewPosition === 0) {
    while (editIndex < editContent.length) {
      const markerLength = getMarkerLength(editContent.substring(editIndex));
      if (markerLength > 0) {
        editIndex += markerLength;
        continue;
      }
      // Found first non-marker character
      break;
    }
    return editIndex;
  }

  // Walk both strings character-by-character for positions > 0
  // Skip inline syntax markers in editContent while advancing viewContent
  let viewIndex = 0;

  while (viewIndex < viewPosition && editIndex < editContent.length) {
    // Check if we're at an inline syntax marker in edit content
    const markerLength = getMarkerLength(editContent.substring(editIndex));

    if (markerLength > 0) {
      // Skip markdown syntax markers (don't advance viewIndex)
      editIndex += markerLength;
      continue;
    }

    // Regular character - advance both indices
    viewIndex++;
    editIndex++;
  }

  // After reaching target view position, skip any OPENING markers
  // (markers that are followed by content, not at end/before closing markers)
  // This ensures cursor lands on actual content, not before an opening marker
  while (editIndex < editContent.length) {
    const markerLength = getMarkerLength(editContent.substring(editIndex));
    if (markerLength > 0) {
      // Check if this is an opening marker (followed by content)
      const afterMarker = editIndex + markerLength;
      if (afterMarker < editContent.length) {
        // If the character after the marker is NOT another marker or end of string,
        // this is an opening marker - skip it
        const nextMarkerLength = getMarkerLength(editContent.substring(afterMarker));
        if (nextMarkerLength === 0) {
          // Opening marker followed by content - skip it
          editIndex += markerLength;
          continue;
        }
      }
    }
    break;
  }

  return editIndex;
}

/**
 * Map character position from edit content to view content
 *
 * Edit content has markdown syntax visible (e.g., "### *And* again")
 * View content has markdown syntax stripped (e.g., "And again")
 *
 * This accounts for BOTH prefix syntax (###, >, 1.) AND inline markers (**,*,~~,`).
 * This is the reverse of mapViewPositionToEditPosition.
 *
 * @param editPosition - Character position in edit content (0-based)
 * @param editContent - The edit content (with syntax)
 * @returns Character position in view content (0-based)
 *
 * @example
 * // Edit: "### *And* again" (cursor at position 5 = 'A')
 * // View: "And again" (corresponds to position 0 = 'A')
 * mapEditPositionToViewPosition(5, "### *And* again") // → 0
 *
 * @example
 * // Edit: "**Hello** world" (cursor at position 10 = 'w')
 * // View: "Hello world" (corresponds to position 6 = 'w')
 * mapEditPositionToViewPosition(10, "**Hello** world") // → 6
 */
export function mapEditPositionToViewPosition(editPosition: number, editContent: string): number {
  if (editPosition === 0) {
    return 0;
  }

  // First, skip any prefix syntax at the start
  const prefixLength = getPrefixSyntaxLength(editContent);

  // If edit position is within the prefix, view position is 0
  if (editPosition <= prefixLength) {
    return 0;
  }

  let viewIndex = 0;
  let editIndex = prefixLength; // Start after prefix

  while (editIndex < editPosition && editIndex < editContent.length) {
    // Check if we're at an inline syntax marker in edit content
    const markerLength = getMarkerLength(editContent.substring(editIndex));

    if (markerLength > 0) {
      // Skip markdown syntax markers (don't advance viewIndex)
      editIndex += markerLength;
      continue;
    }

    // Regular character - advance both indices
    viewIndex++;
    editIndex++;
  }

  return viewIndex;
}

/**
 * Strip inline markdown markers from a line, preserving prefix syntax
 *
 * This strips inline formatting (bold, italic, code, strikethrough) but keeps
 * prefix syntax like headers (###), quotes (>), and list markers (1. -).
 *
 * @param line - The line of text to strip
 * @returns The line with inline formatting removed but prefix preserved
 *
 * @example
 * stripInlineMarkdown("### *And* again") // → "### And again"
 * stripInlineMarkdown("**Hello** world") // → "Hello world"
 */
export function stripInlineMarkdown(line: string): string {
  // Strip only inline formatting markers, not prefix syntax
  return line
    .replace(/\*\*(.*?)\*\*/g, '$1')
    .replace(/__(.*?)__/g, '$1')
    .replace(/~~(.*?)~~/g, '$1')
    .replace(/(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g, '$1')
    .replace(/(?<!_)_(?!_)([^_]+)_(?!_)/g, '$1')
    .replace(/`([^`]+)`/g, '$1');
}

/**
 * Strip all markdown from a line - both prefix syntax and inline formatting
 *
 * This is used for arrow navigation to get the actual view text that users see.
 * In view mode:
 * - Header prefixes (###) are rendered as styled text without the syntax
 * - Quote prefixes (>) are rendered as styled blocks without the syntax
 * - List prefixes (1., -, *) are rendered as list items without the syntax
 * - Inline formatting (**bold**, *italic*, etc.) is rendered without markers
 *
 * @param line - The line of text to strip
 * @returns The line with all markdown syntax removed (what user sees in view mode)
 *
 * @example
 * stripAllMarkdown("### *And* again") // → "And again"
 * stripAllMarkdown("> **Quote** text") // → "Quote text"
 * stripAllMarkdown("1. List **item**") // → "List item"
 */
export function stripAllMarkdown(line: string): string {
  // First strip prefix syntax
  let result = line
    // Header prefixes: ### Header
    .replace(/^#{1,6}\s+/, '')
    // Quote prefixes: > Quote (handle multiple levels)
    .replace(/^(?:>\s*)+/, '')
    // Ordered list prefixes: 1. Item, 2. Item, etc.
    .replace(/^\d+\.\s+/, '')
    // Unordered list prefixes: - Item, * Item
    .replace(/^[-*]\s+/, '');

  // Then strip inline formatting
  result = result
    .replace(/\*\*(.*?)\*\*/g, '$1')
    .replace(/__(.*?)__/g, '$1')
    .replace(/~~(.*?)~~/g, '$1')
    .replace(/(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g, '$1')
    .replace(/(?<!_)_(?!_)([^_]+)_(?!_)/g, '$1')
    .replace(/`([^`]+)`/g, '$1');

  return result;
}
