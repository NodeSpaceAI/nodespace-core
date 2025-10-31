/**
 * Text formatting utilities for consistent text processing across the application
 */

/**
 * Maximum length for tab titles before truncation
 * This ensures consistent display across all viewers and navigation
 */
export const MAX_TAB_TITLE_LENGTH = 40;

/**
 * Ellipsis character used for truncated text
 */
const ELLIPSIS = '...';

/**
 * Truncates text to fit within tab title constraints
 *
 * Extracts the first line and truncates to MAX_TAB_TITLE_LENGTH characters.
 * Multi-line content is ignored for tab titles to maintain visual consistency.
 *
 * This utility ensures consistent tab title formatting across:
 * - BaseNodeViewer (default header)
 * - NavigationService (tab creation)
 * - Any other component that needs to display tab titles
 *
 * @param content - Full text content (may be multi-line)
 * @param fallback - Text to return if content is empty (default: 'Untitled')
 * @returns Truncated first line with ellipsis if needed, or fallback text
 *
 * @example
 * ```typescript
 * formatTabTitle('Short title')
 * // Returns: 'Short title'
 *
 * formatTabTitle('This is a very long title that exceeds the maximum length allowed')
 * // Returns: 'This is a very long title that exc...'
 *
 * formatTabTitle('First line\nSecond line\nThird line')
 * // Returns: 'First line'
 *
 * formatTabTitle('')
 * // Returns: 'Untitled'
 *
 * formatTabTitle('', 'Custom Fallback')
 * // Returns: 'Custom Fallback'
 * ```
 */
export function formatTabTitle(content: string, fallback: string = 'Untitled'): string {
  // Extract first line and trim whitespace
  const firstLine = content.split('\n')[0].trim();

  // Return fallback if empty
  if (!firstLine) {
    return fallback;
  }

  // Truncate if too long
  if (firstLine.length > MAX_TAB_TITLE_LENGTH) {
    return firstLine.substring(0, MAX_TAB_TITLE_LENGTH - ELLIPSIS.length) + ELLIPSIS;
  }

  return firstLine;
}
