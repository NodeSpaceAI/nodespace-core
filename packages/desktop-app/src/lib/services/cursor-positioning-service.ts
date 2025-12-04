/**
 * CursorPositioningService
 *
 * Centralized service for managing cursor positioning across textarea-based editors.
 * Handles focus management, syntax-aware positioning, and multiline support.
 *
 * Key Features:
 * - Focus management with proper timing
 * - Syntax-aware positioning (skip header markers, formatting syntax)
 * - Line-aware positioning for multiline content
 * - Reusable across all node types
 */

import { createLogger } from '$lib/utils/logger';

const log = createLogger('CursorPositioningService');

export interface CursorPosition {
  line: number; // 0-based line index
  column: number; // 0-based character offset within line
}

export interface PositioningOptions {
  /**
   * Whether to focus the textarea before positioning
   * @default true
   */
  focus?: boolean;

  /**
   * Delay in ms before positioning (allows DOM to settle)
   * @default 0
   */
  delay?: number;

  /**
   * Whether to skip syntax when positioning at beginning of line
   * @default true
   */
  skipSyntax?: boolean;
}

export class CursorPositioningService {
  private static instance: CursorPositioningService;

  private constructor() {}

  public static getInstance(): CursorPositioningService {
    if (!CursorPositioningService.instance) {
      CursorPositioningService.instance = new CursorPositioningService();
    }
    return CursorPositioningService.instance;
  }

  /**
   * Set cursor at the beginning of a line, accounting for syntax
   *
   * @param textarea The textarea element
   * @param lineNumber 0-based line number (defaults to 0 for first line)
   * @param options Positioning options
   *
   * @example
   * // Position at beginning of first line, after "### " header syntax
   * service.setCursorAtBeginningOfLine(textarea, 0);
   *
   * // Position at beginning of third line
   * service.setCursorAtBeginningOfLine(textarea, 2);
   */
  public setCursorAtBeginningOfLine(
    textarea: HTMLTextAreaElement,
    lineNumber: number = 0,
    options: PositioningOptions = {}
  ): void {
    const { focus = true, delay = 0, skipSyntax = true } = options;

    const doPosition = () => {
      const lines = textarea.value.split('\n');

      // Validate line number
      if (lineNumber < 0 || lineNumber >= lines.length) {
        log.warn(`Invalid line number ${lineNumber}, using 0`);
        lineNumber = 0;
      }

      // Calculate absolute position
      let position = 0;
      for (let i = 0; i < lineNumber; i++) {
        position += lines[i].length + 1; // +1 for newline
      }

      // Skip syntax if requested
      if (skipSyntax && lineNumber < lines.length) {
        const line = lines[lineNumber];
        const syntaxLength = this.getSyntaxLength(line);
        position += syntaxLength;
      }

      // Focus if requested
      if (focus) {
        textarea.focus();
      }

      // Set cursor position
      textarea.selectionStart = position;
      textarea.selectionEnd = position;
    };

    if (delay > 0) {
      setTimeout(doPosition, delay);
    } else {
      doPosition();
    }
  }

  /**
   * Set cursor at a specific position in the textarea
   *
   * @param textarea The textarea element
   * @param position Absolute character position (0-based)
   * @param options Positioning options
   */
  public setCursorAtPosition(
    textarea: HTMLTextAreaElement,
    position: number,
    options: PositioningOptions = {}
  ): void {
    const { focus = true, delay = 0 } = options;

    const doPosition = () => {
      // Clamp position to valid range
      const maxPosition = textarea.value.length;
      const clampedPosition = Math.max(0, Math.min(position, maxPosition));

      // Focus if requested
      if (focus) {
        textarea.focus();
      }

      // Set cursor position
      textarea.selectionStart = clampedPosition;
      textarea.selectionEnd = clampedPosition;
    };

    if (delay > 0) {
      setTimeout(doPosition, delay);
    } else {
      doPosition();
    }
  }

  /**
   * Set cursor at a specific line and column
   *
   * @param textarea The textarea element
   * @param position Line and column position
   * @param options Positioning options
   */
  public setCursorAtLineColumn(
    textarea: HTMLTextAreaElement,
    position: CursorPosition,
    options: PositioningOptions = {}
  ): void {
    const { focus = true, delay = 0 } = options;

    const doPosition = () => {
      const lines = textarea.value.split('\n');

      // Validate line number
      let { line, column } = position;
      if (line < 0 || line >= lines.length) {
        log.warn(`Invalid line ${line}, using 0`);
        line = 0;
      }

      // Validate column
      const lineContent = lines[line];
      if (column < 0 || column > lineContent.length) {
        log.warn(`Invalid column ${column} for line ${line}, clamping`);
        column = Math.max(0, Math.min(column, lineContent.length));
      }

      // Calculate absolute position
      let absolutePosition = 0;
      for (let i = 0; i < line; i++) {
        absolutePosition += lines[i].length + 1; // +1 for newline
      }
      absolutePosition += column;

      // Focus if requested
      if (focus) {
        textarea.focus();
      }

      // Set cursor position
      textarea.selectionStart = absolutePosition;
      textarea.selectionEnd = absolutePosition;
    };

    if (delay > 0) {
      setTimeout(doPosition, delay);
    } else {
      doPosition();
    }
  }

  /**
   * Get the current cursor position as line and column
   *
   * @param textarea The textarea element
   * @returns Current cursor position
   */
  public getCursorPosition(textarea: HTMLTextAreaElement): CursorPosition {
    const position = textarea.selectionStart;
    const lines = textarea.value.substring(0, position).split('\n');

    return {
      line: lines.length - 1,
      column: lines[lines.length - 1].length
    };
  }

  /**
   * Calculate the length of syntax at the beginning of a line
   * Supports: headers (### ), formatting (**bold**, *italic*, etc.)
   *
   * @param line The line content
   * @returns Number of characters to skip
   */
  private getSyntaxLength(line: string): number {
    // Header syntax: ### (1-6 hashes followed by space)
    const headerMatch = line.match(/^(#{1,6}\s+)/);
    if (headerMatch) {
      return headerMatch[1].length;
    }

    // Inline formatting at beginning of line (less common, but supported)
    const formatPatterns = [
      /^(\*\*)/, // Bold **
      /^(__)/, // Bold __
      /^(\*(?!\*))/, // Italic * (not part of **)
      /^(_(?!_))/, // Italic _ (not part of __)
      /^(~~)/, // Strikethrough ~~
      /^(`)/ // Code `
    ];

    for (const pattern of formatPatterns) {
      const match = line.match(pattern);
      if (match) {
        return match[1].length;
      }
    }

    return 0; // No syntax to skip
  }
}
