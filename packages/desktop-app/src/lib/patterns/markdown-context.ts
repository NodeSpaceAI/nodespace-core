/**
 * Markdown Context Analysis
 *
 * Extracted from markdown-splitter.ts
 * Analyzes the markdown formatting state at a given position in text.
 * Used to preserve inline formatting when splitting content.
 */

/**
 * Tracks which markdown formats are active at a specific position
 */
export interface MarkdownContext {
  bold: boolean;
  italic: boolean;
  strikethrough: boolean;
  code: boolean;
  openMarkers: string[];
  closeMarkers: string[];
}

/**
 * Analyze markdown context at a given position in text
 * Returns information about which formatting is active at that position
 */
export function analyzeMarkdownContext(text: string, position: number): MarkdownContext {
  const beforeText = text.substring(0, position);
  const context: MarkdownContext = {
    bold: false,
    italic: false,
    strikethrough: false,
    code: false,
    openMarkers: [],
    closeMarkers: []
  };

  // Track markdown state by scanning from beginning to cursor position
  let i = 0;
  let inCode = false;

  while (i < beforeText.length) {
    const char = beforeText[i];
    const nextChar = beforeText[i + 1];

    // Code blocks take precedence - ignore other formatting inside
    if (char === '`' && !inCode) {
      inCode = true;
      context.code = true;
      context.openMarkers.push('`');
      i++;
      continue;
    } else if (char === '`' && inCode) {
      inCode = false;
      context.code = false;
      context.closeMarkers.push('`');
      i++;
      continue;
    }

    // Skip processing other markdown inside code blocks
    if (inCode) {
      i++;
      continue;
    }

    // Bold (**text** or __text__)
    if ((char === '*' && nextChar === '*') || (char === '_' && nextChar === '_')) {
      if (context.bold) {
        context.bold = false;
        context.closeMarkers.push(char + nextChar);
      } else {
        context.bold = true;
        context.openMarkers.push(char + nextChar);
      }
      i += 2;
      continue;
    }

    // Italic (*text* or _text_) - only if not part of bold
    if ((char === '*' && nextChar !== '*') || (char === '_' && nextChar !== '_')) {
      if (context.italic) {
        context.italic = false;
        context.closeMarkers.push(char);
      } else {
        context.italic = true;
        context.openMarkers.push(char);
      }
      i++;
      continue;
    }

    // Strikethrough - check for double tilde first (~~text~~), then single (~text~)
    if (char === '~' && nextChar === '~') {
      if (context.strikethrough) {
        context.strikethrough = false;
        context.closeMarkers.push('~~');
      } else {
        context.strikethrough = true;
        context.openMarkers.push('~~');
      }
      i += 2;
      continue;
    }

    // Single tilde strikethrough (~text~)
    if (char === '~' && nextChar !== '~') {
      if (context.strikethrough) {
        context.strikethrough = false;
        context.closeMarkers.push('~');
      } else {
        context.strikethrough = true;
        context.openMarkers.push('~');
      }
      i++;
      continue;
    }

    i++;
  }

  return context;
}

/**
 * Get closing markers for open formatting
 */
export function getClosingMarkers(context: MarkdownContext): string {
  const markers: string[] = [];

  // Close in reverse order (LIFO - Last In, First Out)
  const openMarkers = [...context.openMarkers].reverse();

  for (const marker of openMarkers) {
    // Map opening markers to their closing equivalents
    switch (marker) {
      case '**':
      case '__':
        if (context.bold) markers.push(marker);
        break;
      case '*':
      case '_':
        if (context.italic) markers.push(marker);
        break;
      case '~~':
        if (context.strikethrough) markers.push('~~');
        break;
      case '~':
        if (context.strikethrough) markers.push('~');
        break;
      case '`':
        if (context.code) markers.push('`');
        break;
    }
  }

  return markers.join('');
}

/**
 * Get opening markers for continuing formatting
 */
export function getOpeningMarkers(context: MarkdownContext): string {
  const markers: string[] = [];

  // Open in original order
  for (const marker of context.openMarkers) {
    switch (marker) {
      case '**':
      case '__':
        if (context.bold) markers.push(marker);
        break;
      case '*':
      case '_':
        if (context.italic) markers.push(marker);
        break;
      case '~~':
        if (context.strikethrough) markers.push('~~');
        break;
      case '~':
        if (context.strikethrough) markers.push('~');
        break;
      case '`':
        if (context.code) markers.push('`');
        break;
    }
  }

  return markers.join('');
}

/**
 * Check if any formatting is active at this position
 */
export function hasActiveFormatting(context: MarkdownContext): boolean {
  return context.bold || context.italic || context.strikethrough || context.code;
}
