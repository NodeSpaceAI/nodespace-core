/**
 * Markdown Pattern Type Definitions
 *
 * Defines the data structures for detecting and processing markdown patterns
 * in ContentEditable text editing system.
 */

export type MarkdownPatternType =
  // Block types (4 essential)
  | 'header'
  | 'bullet'
  | 'blockquote'
  | 'codeblock'
  // Inline types (3 essential)
  | 'bold'
  | 'italic'
  | 'inlinecode';

export interface MarkdownPattern {
  type: MarkdownPatternType;
  level?: number; // For headers (1-6)
  start: number; // Character position start
  end: number; // Character position end
  syntax: string; // Original syntax (e.g., "##", "**")
  content: string; // Text without syntax
  raw: string; // Original text with syntax
}

export interface PatternDetectionResult {
  patterns: MarkdownPattern[];
  processedContent: string; // Content with WYSIWYG transformations applied
  hasPatterns: boolean;
}

export interface WYSIWYGTransformation {
  type: MarkdownPatternType;
  level?: number;
  cssClass: string;
  hideMarkup: boolean;
  htmlTag?: string;
}

// Pre-defined WYSIWYG transformation rules
export const WYSIWYG_TRANSFORMATIONS: Record<string, WYSIWYGTransformation> = {
  'header-1': {
    type: 'header',
    level: 1,
    cssClass: 'markdown-header-1',
    hideMarkup: true,
    htmlTag: 'h1'
  },
  'header-2': {
    type: 'header',
    level: 2,
    cssClass: 'markdown-header-2',
    hideMarkup: true,
    htmlTag: 'h2'
  },
  'header-3': {
    type: 'header',
    level: 3,
    cssClass: 'markdown-header-3',
    hideMarkup: true,
    htmlTag: 'h3'
  },
  bold: {
    type: 'bold',
    cssClass: 'markdown-bold',
    hideMarkup: true,
    htmlTag: 'strong'
  },
  italic: {
    type: 'italic',
    cssClass: 'markdown-italic',
    hideMarkup: true,
    htmlTag: 'em'
  },
  inlinecode: {
    type: 'inlinecode',
    cssClass: 'markdown-code',
    hideMarkup: true,
    htmlTag: 'code'
  },
  blockquote: {
    type: 'blockquote',
    cssClass: 'markdown-blockquote',
    hideMarkup: true,
    htmlTag: 'blockquote'
  },
  bullet: {
    type: 'bullet',
    cssClass: 'markdown-bullet',
    hideMarkup: true
  }
};
