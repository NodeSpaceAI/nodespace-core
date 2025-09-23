/**
 * Markdown-Aware Content Splitting Utility
 *
 * Handles splitting content while preserving markdown formatting syntax.
 * When Enter is pressed within formatted text, this ensures:
 * 1. Origin node gets completed syntax (e.g., **bold** -> **bold**)
 * 2. New node starts with proper opening syntax (e.g., **continuing...)
 */

export interface MarkdownSplitResult {
  beforeContent: string;
  afterContent: string;
  newNodeCursorPosition: number; // Position where cursor should be placed in the new node
}

interface MarkdownContext {
  bold: boolean;
  italic: boolean;
  strikethrough: boolean;
  code: boolean;
  openMarkers: string[];
  closeMarkers: string[];
}

/**
 * Analyze markdown context at a given position in text
 */
function analyzeMarkdownContext(text: string, position: number): MarkdownContext {
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

    // Strikethrough (~~text~~)
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

    i++;
  }

  return context;
}

/**
 * Get closing markers for open formatting
 */
function getClosingMarkers(context: MarkdownContext): string {
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
function getOpeningMarkers(context: MarkdownContext): string {
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
      case '`':
        if (context.code) markers.push('`');
        break;
    }
  }

  return markers.join('');
}

/**
 * Split content at position while preserving markdown formatting
 */
export function splitMarkdownContent(content: string, position: number): MarkdownSplitResult {
  // Handle edge cases
  if (position <= 0) {
    return {
      beforeContent: '',
      afterContent: content,
      newNodeCursorPosition: 0
    };
  }

  if (position >= content.length) {
    return {
      beforeContent: content,
      afterContent: '',
      newNodeCursorPosition: 0
    };
  }

  // Simple split first
  const beforeCursor = content.substring(0, position);
  const afterCursor = content.substring(position);

  // Analyze markdown context at split position
  const context = analyzeMarkdownContext(content, position);

  // If no active formatting, return simple split
  if (!context.bold && !context.italic && !context.strikethrough && !context.code) {
    return {
      beforeContent: beforeCursor,
      afterContent: afterCursor,
      newNodeCursorPosition: 0
    };
  }

  // Complete the formatting in the before content
  const closingMarkers = getClosingMarkers(context);
  const beforeContent = beforeCursor + closingMarkers;

  // Start new content with opening markers
  const openingMarkers = getOpeningMarkers(context);
  const afterContent = openingMarkers + afterCursor;

  // Calculate cursor position in new node (after opening markers)
  const newNodeCursorPosition = openingMarkers.length;

  return {
    beforeContent,
    afterContent,
    newNodeCursorPosition
  };
}

/**
 * Test if position is within markdown formatting
 */
export function isInMarkdownFormat(content: string, position: number): boolean {
  const context = analyzeMarkdownContext(content, position);
  return context.bold || context.italic || context.strikethrough || context.code;
}