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
 * Map character position from view content to edit content
 *
 * View content has markdown syntax stripped (e.g., "Hello world")
 * Edit content has markdown syntax visible (e.g., "**Hello** world")
 *
 * This function accounts for syntax markers when mapping positions.
 *
 * @param viewPosition - Character position in view content (0-based)
 * @param viewContent - The view content (syntax stripped, from displayContent)
 * @param editContent - The edit content (with syntax, from content prop)
 * @returns Character position in edit content (0-based)
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

  // Edge case: position 0 - skip leading syntax markers
  if (viewPosition === 0) {
    let editIndex = 0;
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
  // Skip syntax markers in editContent while advancing viewContent
  let viewIndex = 0;
  let editIndex = 0;

  while (viewIndex < viewPosition && editIndex < editContent.length) {
    // Check if we're at a syntax marker in edit content
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

  return editIndex;
}
