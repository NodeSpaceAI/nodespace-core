/**
 * Pattern System Types
 *
 * Defines the core interfaces for the unified markdown pattern system.
 * These types enable declarative pattern definitions in plugins with
 * pluggable splitting strategies.
 */

/**
 * Result of splitting content at a cursor position
 */
export interface SplitResult {
  beforeContent: string;
  afterContent: string;
  newNodeCursorPosition: number;
}

/**
 * Identifies where the cursor should be placed in the new node after splitting
 */
export type CursorPlacement = 'after-prefix' | 'start' | 'end';

/**
 * Strategy for how content should be split when a pattern is detected
 */
export type SplittingStrategy = 'prefix-inheritance' | 'simple-split' | 'wrapper-split';

/**
 * Declarative pattern template that defines:
 * - Pattern matching (regex)
 * - Node type identification
 * - Splitting behavior (which strategy to use)
 * - Cursor positioning rules
 * - Content transformation
 */
export interface PatternTemplate {
  /**
   * Regular expression to match the pattern
   * Used for both detection and content analysis
   */
  regex: RegExp;

  /**
   * Node type this pattern corresponds to
   * Must match a registered node type in the plugin system
   */
  nodeType: string;

  /**
   * Priority for pattern detection when multiple patterns could match
   * Higher priority patterns are checked first (1-100, where 100 is highest)
   */
  priority: number;

  /**
   * Which splitting strategy to use when this pattern is encountered
   * - 'prefix-inheritance': Inherits prefix on new line (headers, lists, quotes)
   * - 'simple-split': Simple split with inline formatting preservation (text)
   * - 'wrapper-split': Handles wrapped content (code blocks - future)
   */
  splittingStrategy: SplittingStrategy;

  /**
   * Prefix to inherit on the new node when using 'prefix-inheritance' strategy
   * For headers: "# ", "## ", etc.
   * For lists: "1. "
   * For quotes: "> "
   *
   * If not provided and using prefix-inheritance, uses first match group from regex
   */
  prefixToInherit?: string;

  /**
   * Where the cursor should be positioned in the new node after splitting
   * - 'after-prefix': After opening prefix markers (for headers, lists, quotes)
   * - 'start': At the very beginning (for regular text)
   * - 'end': At the very end (for trailing content)
   */
  cursorPlacement: CursorPlacement;

  /**
   * Whether to remove the pattern syntax from stored content
   * For headers: typically false (keep "# " for editing)
   * For tasks: typically true (checkbox shown as icon, not in content)
   */
  cleanContent?: boolean;

  /**
   * Optional template for transforming content
   * Can include placeholders for regex match groups
   */
  contentTemplate?: string;

  /**
   * Optional function to extract metadata from regex match
   * Used to capture additional information like header level, language, etc.
   */
  extractMetadata?: (match: RegExpMatchArray) => Record<string, unknown>;
}

/**
 * Strategy interface for content splitting
 * Implementations define how specific pattern types are split
 */
export interface SplittingStrategyImpl {
  /**
   * Split content at the given position using the pattern's rules
   */
  split(content: string, position: number, pattern: PatternTemplate): SplitResult;
}

/**
 * Result of pattern detection in content
 */
export interface PatternDetectionResult {
  /**
   * The matched pattern, or null if no pattern detected
   */
  pattern: PatternTemplate | null;

  /**
   * The regex match result for further processing
   */
  match: RegExpMatchArray | null;

  /**
   * Whether a pattern was found
   */
  found: boolean;
}
