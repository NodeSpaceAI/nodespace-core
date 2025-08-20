/**
 * Enhanced ContentProcessor Service
 *
 * Implements Logseq-inspired dual-representation pattern (Source ↔ AST ↔ Display)
 * as the foundation for Issue #70 (Phase 1.1 of Epic #69: Text Editor Architecture Enhancement).
 *
 * Key Features:
 * - Lossless conversions between source markdown, AST, and display HTML
 * - Wikilink detection and preparation for backlinking (Phase 2 foundation)
 * - Content validation and security
 * - Performance optimization for large documents
 * - Extensible AST for future AI and collaboration features
 */

import { stripMarkdown, validateMarkdown } from './markdownUtils.js';

// ============================================================================
// Core AST Types for Dual-Representation
// ============================================================================

export interface ASTNode {
  type: string;
  start: number;
  end: number;
  children?: ASTNode[];
  properties?: Record<string, unknown>;
}

export interface DocumentNode extends ASTNode {
  type: 'document';
  children: ASTNode[];
  metadata: ContentMetadata;
}

export interface HeaderNode extends ASTNode {
  type: 'header';
  level: number;
  content: string;
  rawSyntax: string;
}

export interface TextNode extends ASTNode {
  type: 'text';
  content: string;
}

export interface WikiLinkNode extends ASTNode {
  type: 'wikilink';
  target: string;
  displayText: string;
  rawSyntax: string;
}

export interface InlineNode extends ASTNode {
  type: 'bold' | 'italic' | 'code';
  content: string;
  rawSyntax: string;
}

export interface ParagraphNode extends ASTNode {
  type: 'paragraph';
  children: ASTNode[];
}

export type MarkdownAST = DocumentNode;

// ============================================================================
// Supporting Types
// ============================================================================

export interface ContentMetadata {
  totalCharacters: number;
  wordCount: number;
  hasWikiLinks: boolean;
  headerCount: number;
  inlineFormatCount: number;
  lastModified: number;
}

export interface ValidationResult {
  isValid: boolean;
  errors: ValidationError[];
  warnings: ValidationWarning[];
}

export interface ValidationError {
  type: 'syntax' | 'security' | 'structure';
  message: string;
  position: number;
  suggestion?: string;
}

export interface ValidationWarning {
  type: 'performance' | 'formatting' | 'accessibility';
  message: string;
  position: number;
}

export interface WikiLink {
  text: string;
  target: string;
  startPos: number;
  endPos: number;
  displayText?: string;
}

export interface PreparedContent {
  originalContent: string;
  wikiLinks: WikiLink[];
  processedContent: string;
  linkPositions: Map<string, number[]>;
}

// ============================================================================
// ContentProcessor Service Implementation
// ============================================================================

export class ContentProcessor {
  private static instance: ContentProcessor;

  // Performance optimization: Cache frequently accessed patterns
  private readonly HEADER_REGEX = /^(#{1,6})\s+(.*)$/gm;
  private readonly WIKILINK_REGEX = /\[\[([^[\]]+(?:\[[^[\]]*\][^[\]]*)*)\]\]/g;
  private readonly BOLD_REGEX = /\*\*(.*?)\*\*/g;
  private readonly ITALIC_REGEX = /(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g;
  private readonly CODE_REGEX = /`([^`]+)`/g;

  public static getInstance(): ContentProcessor {
    if (!ContentProcessor.instance) {
      ContentProcessor.instance = new ContentProcessor();
    }
    return ContentProcessor.instance;
  }

  // ========================================================================
  // Dual-Representation Core Methods (Logseq-inspired)
  // ========================================================================

  /**
   * Parse markdown source into structured AST
   * Core of the dual-representation pattern
   */
  public parseMarkdown(source: string): MarkdownAST {
    if (!source) {
      return this.createEmptyDocument();
    }

    const metadata: ContentMetadata = {
      totalCharacters: source.length,
      wordCount: this.getWordCount(source),
      hasWikiLinks: this.WIKILINK_REGEX.test(source),
      headerCount: 0,
      inlineFormatCount: 0,
      lastModified: Date.now()
    };

    const children: ASTNode[] = [];
    let position = 0;

    // Parse line by line first, then group into blocks
    const lines = source.split('\n');
    let currentBlock = '';
    let blockStartPos = 0;
    let inParagraph = false;

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const lineStart = source.indexOf(line, position);

      // Check if this line is a header
      const headerMatch = line.match(/^(#{1,6})\s+(.*)$/);

      if (headerMatch) {
        // If we have accumulated paragraph content, process it first
        if (currentBlock.trim()) {
          const paragraphNode = this.parseParagraph(currentBlock.trim(), blockStartPos);
          children.push(paragraphNode);
          metadata.inlineFormatCount += this.countInlineFormats(currentBlock);
          currentBlock = '';
        }

        // Process header
        const headerNode: HeaderNode = {
          type: 'header',
          level: headerMatch[1].length,
          content: headerMatch[2],
          rawSyntax: headerMatch[1],
          start: lineStart,
          end: lineStart + line.length
        };
        children.push(headerNode);
        metadata.headerCount++;
        inParagraph = false;
      } else if (line.trim() === '') {
        // Empty line - end current paragraph if we're in one
        if (inParagraph && currentBlock.trim()) {
          const paragraphNode = this.parseParagraph(currentBlock.trim(), blockStartPos);
          children.push(paragraphNode);
          metadata.inlineFormatCount += this.countInlineFormats(currentBlock);
          currentBlock = '';
          inParagraph = false;
        }
      } else {
        // Regular content line
        if (!inParagraph) {
          blockStartPos = lineStart;
          inParagraph = true;
        } else {
          currentBlock += '\n';
        }
        currentBlock += line;
      }

      position = lineStart + line.length + 1; // +1 for newline
    }

    // Process any remaining paragraph content
    if (currentBlock.trim()) {
      const paragraphNode = this.parseParagraph(currentBlock.trim(), blockStartPos);
      children.push(paragraphNode);
      metadata.inlineFormatCount += this.countInlineFormats(currentBlock);
    }

    return {
      type: 'document',
      start: 0,
      end: source.length,
      children,
      metadata
    };
  }

  /**
   * Render AST back to HTML for display
   * Maintains lossless conversion capability
   */
  public renderAST(ast: MarkdownAST): string {
    if (!ast.children || ast.children.length === 0) {
      return '';
    }

    return ast.children.map((node) => this.renderNode(node)).join('');
  }

  /**
   * Convert AST back to source markdown
   * Completes the lossless round-trip: Source → AST → Source
   */
  public astToMarkdown(ast: MarkdownAST): string {
    if (!ast.children || ast.children.length === 0) {
      return '';
    }

    return ast.children.map((node) => this.nodeToMarkdown(node)).join('\n\n');
  }

  // ========================================================================
  // Legacy Compatibility Methods
  // ========================================================================

  /**
   * Legacy compatibility: markdown to display HTML
   * Bridges old markdownUtils.ts with new AST system
   */
  public markdownToDisplay(markdown: string): string {
    const ast = this.parseMarkdown(markdown);
    return this.renderAST(ast);
  }

  /**
   * Legacy compatibility: display HTML back to markdown
   * Uses AST as intermediate representation for accuracy
   */
  public displayToMarkdown(html: string): string {
    // For now, fall back to text extraction
    // Future enhancement: Parse HTML → AST → Markdown
    if (typeof document !== 'undefined') {
      const tempDiv = document.createElement('div');
      tempDiv.innerHTML = html;
      return tempDiv.textContent || '';
    } else {
      // Fallback for Node.js/test environment - simple HTML tag removal
      return html.replace(/<[^>]*>/g, '').trim();
    }
  }

  // ========================================================================
  // Content Validation and Security
  // ========================================================================

  /**
   * Comprehensive content validation
   * Prevents XSS, malformed content, and performance issues
   */
  public validateContent(content: string): ValidationResult {
    const errors: ValidationError[] = [];
    const warnings: ValidationWarning[] = [];

    // Security validation
    if (this.containsSuspiciousContent(content)) {
      errors.push({
        type: 'security',
        message: 'Content contains potentially unsafe HTML or JavaScript',
        position: 0,
        suggestion: 'Remove script tags and unsafe HTML attributes'
      });
    }

    // Syntax validation using existing validator
    const syntaxValidation = validateMarkdown(content);
    if (!syntaxValidation.isValid) {
      errors.push(
        ...syntaxValidation.errors.map((error) => ({
          type: 'syntax' as const,
          message: error,
          position: 0,
          suggestion: 'Fix markdown syntax'
        }))
      );
    }

    // Performance warnings
    if (content.length > 50000) {
      warnings.push({
        type: 'performance',
        message: 'Content is very large and may impact editor performance',
        position: 0
      });
    }

    // Structure validation
    const headerLevels = this.extractHeaderLevels(content);
    if (headerLevels.length > 0) {
      const maxGap = Math.max(...headerLevels.slice(1).map((level, i) => level - headerLevels[i]));
      if (maxGap > 1) {
        warnings.push({
          type: 'formatting',
          message: 'Header levels skip numbers (e.g., H1 → H3)',
          position: 0
        });
      }
    }

    return {
      isValid: errors.length === 0,
      errors,
      warnings
    };
  }

  /**
   * Sanitize content while preserving markdown
   * Removes dangerous content while maintaining text structure
   */
  public sanitizeContent(content: string): string {
    return (
      content
        // Remove script tags and content
        .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '')
        // Remove event handlers (more comprehensive)
        .replace(/\s*on\w+\s*=\s*["'][^"']*["']/gi, '')
        .replace(/\s*on\w+\s*=\s*[^"'\s>]*/gi, '')
        // Remove javascript: and vbscript: protocols completely
        .replace(/javascript:[^"'\s>]*/gi, '')
        .replace(/vbscript:[^"'\s>]*/gi, '')
        // Remove data: URLs (potential XSS vector)
        .replace(/data:[^;]*;base64[^"'\s>]*/gi, 'removed-data-url')
        // Remove potentially dangerous HTML elements
        .replace(/<iframe\b[^>]*>/gi, '')
        .replace(/<object\b[^>]*>/gi, '')
        .replace(/<embed\b[^>]*>/gi, '')
        .replace(/<link\b[^>]*>/gi, '')
        .replace(/<meta\b[^>]*>/gi, '')
        // Remove style attributes that could contain CSS injection
        .replace(/\s*style\s*=\s*["'][^"']*["']/gi, '')
    );
  }

  // ========================================================================
  // Header Detection and Management
  // ========================================================================

  /**
   * Parse header level from content
   * Enhanced version of TextNode's header detection
   */
  public parseHeaderLevel(content: string): number {
    // Don't trim - we need to preserve trailing spaces for detection
    if (!content.startsWith('#')) {
      return 0;
    }

    const match = content.match(/^(#{1,6})\s/);
    return match ? match[1].length : 0;
  }

  /**
   * Strip header syntax from content
   * Returns display text without markdown syntax
   */
  public stripHeaderSyntax(content: string): string {
    const match = content.match(/^#{1,6}\s+(.*)$/);
    return match ? match[1] : content;
  }

  // ========================================================================
  // Backlink Preparation (Phase 2 Foundation)
  // ========================================================================

  /**
   * Detect wikilink syntax [[text]] in content
   * Foundation for Phase 2 backlinking system
   */
  public detectWikiLinks(content: string): WikiLink[] {
    const wikiLinks: WikiLink[] = [];
    let match;

    // Reset regex state
    this.WIKILINK_REGEX.lastIndex = 0;

    while ((match = this.WIKILINK_REGEX.exec(content)) !== null) {
      const fullMatch = match[0];
      const linkContent = match[1];

      // Support display text: [[target|display]]
      const parts = linkContent.split('|');
      const target = parts[0].trim();
      const displayText = parts[1]?.trim() || target;

      wikiLinks.push({
        text: linkContent,
        target,
        displayText,
        startPos: match.index,
        endPos: match.index + fullMatch.length
      });
    }

    return wikiLinks;
  }

  /**
   * Prepare content for backlinking system
   * Creates processed content with link metadata
   */
  public prepareBacklinkSyntax(content: string): PreparedContent {
    const wikiLinks = this.detectWikiLinks(content);
    const linkPositions = new Map<string, number[]>();

    // Group positions by target
    for (const link of wikiLinks) {
      const positions = linkPositions.get(link.target) || [];
      positions.push(link.startPos);
      linkPositions.set(link.target, positions);
    }

    // For now, processed content is the same as original
    // Phase 2 will implement actual link resolution and rendering
    const processedContent = content;

    return {
      originalContent: content,
      wikiLinks,
      processedContent,
      linkPositions
    };
  }

  // ========================================================================
  // Private Helper Methods
  // ========================================================================

  private createEmptyDocument(): MarkdownAST {
    return {
      type: 'document',
      start: 0,
      end: 0,
      children: [],
      metadata: {
        totalCharacters: 0,
        wordCount: 0,
        hasWikiLinks: false,
        headerCount: 0,
        inlineFormatCount: 0,
        lastModified: Date.now()
      }
    };
  }

  private parseParagraph(content: string, start: number): ParagraphNode {
    const children: ASTNode[] = [];
    const text = content;

    // Parse inline elements: wikilinks, bold, italic, code
    const inlinePatterns = this.findInlinePatterns(text);

    if (inlinePatterns.length === 0) {
      // Plain text
      children.push({
        type: 'text',
        content: text,
        start: start,
        end: start + text.length
      });
    } else {
      // Mixed content with inline formatting
      let lastEnd = 0;

      for (const pattern of inlinePatterns) {
        // Add text before pattern
        if (pattern.start > lastEnd) {
          children.push({
            type: 'text',
            content: text.substring(lastEnd, pattern.start),
            start: start + lastEnd,
            end: start + pattern.start
          });
        }

        // Add the pattern node
        children.push({
          ...pattern,
          start: start + pattern.start,
          end: start + pattern.end
        });

        lastEnd = pattern.end;
      }

      // Add remaining text
      if (lastEnd < text.length) {
        children.push({
          type: 'text',
          content: text.substring(lastEnd),
          start: start + lastEnd,
          end: start + text.length
        });
      }
    }

    return {
      type: 'paragraph',
      children,
      start,
      end: start + content.length
    };
  }

  private findInlinePatterns(text: string): ASTNode[] {
    const patterns: ASTNode[] = [];

    // Find wikilinks
    let match;
    this.WIKILINK_REGEX.lastIndex = 0;
    while ((match = this.WIKILINK_REGEX.exec(text)) !== null) {
      const linkContent = match[1];
      const parts = linkContent.split('|');
      const target = parts[0].trim();
      const displayText = parts[1]?.trim() || target;

      patterns.push({
        type: 'wikilink',
        target,
        displayText,
        rawSyntax: match[0],
        start: match.index,
        end: match.index + match[0].length
      } as WikiLinkNode);
    }

    // Find bold text
    this.BOLD_REGEX.lastIndex = 0;
    while ((match = this.BOLD_REGEX.exec(text)) !== null) {
      patterns.push({
        type: 'bold',
        content: match[1],
        rawSyntax: match[0],
        start: match.index,
        end: match.index + match[0].length
      } as InlineNode);
    }

    // Find italic text
    this.ITALIC_REGEX.lastIndex = 0;
    while ((match = this.ITALIC_REGEX.exec(text)) !== null) {
      patterns.push({
        type: 'italic',
        content: match[1],
        rawSyntax: match[0],
        start: match.index,
        end: match.index + match[0].length
      } as InlineNode);
    }

    // Find code text
    this.CODE_REGEX.lastIndex = 0;
    while ((match = this.CODE_REGEX.exec(text)) !== null) {
      patterns.push({
        type: 'code',
        content: match[1],
        rawSyntax: match[0],
        start: match.index,
        end: match.index + match[0].length
      } as InlineNode);
    }

    // Sort by start position
    return patterns.sort((a, b) => a.start - b.start);
  }

  private renderNode(node: ASTNode): string {
    switch (node.type) {
      case 'header': {
        const headerNode = node as HeaderNode;
        return `<h${headerNode.level} class="ns-markdown-heading ns-markdown-h${headerNode.level}">${this.escapeHtml(headerNode.content)}</h${headerNode.level}>`;
      }

      case 'paragraph': {
        const paragraphNode = node as ParagraphNode;
        const content = paragraphNode.children.map((child) => this.renderNode(child)).join('');
        return `<p class="ns-markdown-paragraph">${content}</p>`;
      }

      case 'text': {
        const textNode = node as TextNode;
        return this.escapeHtml(textNode.content);
      }

      case 'wikilink': {
        const wikiNode = node as WikiLinkNode;
        // For now, render as plain text. Phase 2 will add actual linking
        return `<span class="ns-wikilink" data-target="${this.escapeHtml(wikiNode.target)}">${this.escapeHtml(wikiNode.displayText)}</span>`;
      }

      case 'bold': {
        const boldNode = node as InlineNode;
        return `<strong class="ns-markdown-bold">${this.escapeHtml(boldNode.content)}</strong>`;
      }

      case 'italic': {
        const italicNode = node as InlineNode;
        return `<em class="ns-markdown-italic">${this.escapeHtml(italicNode.content)}</em>`;
      }

      case 'code': {
        const codeNode = node as InlineNode;
        return `<code class="ns-markdown-code">${this.escapeHtml(codeNode.content)}</code>`;
      }

      default:
        return '';
    }
  }

  private nodeToMarkdown(node: ASTNode): string {
    switch (node.type) {
      case 'header': {
        const headerNode = node as HeaderNode;
        return `${headerNode.rawSyntax} ${headerNode.content}`;
      }

      case 'paragraph': {
        const paragraphNode = node as ParagraphNode;
        return paragraphNode.children.map((child) => this.nodeToMarkdown(child)).join('');
      }

      case 'text': {
        const textNode = node as TextNode;
        return textNode.content;
      }

      case 'wikilink': {
        const wikiNode = node as WikiLinkNode;
        return wikiNode.rawSyntax;
      }

      case 'bold':
      case 'italic':
      case 'code': {
        const inlineNode = node as InlineNode;
        return inlineNode.rawSyntax;
      }

      default:
        return '';
    }
  }

  private containsSuspiciousContent(content: string): boolean {
    const suspiciousPatterns = [
      /<script/i,
      /javascript:/i,
      /on\w+\s*=/i,
      /data:[^;]*;base64/i,
      /<iframe/i,
      /<object/i,
      /<embed/i
    ];

    return suspiciousPatterns.some((pattern) => pattern.test(content));
  }

  private extractHeaderLevels(content: string): number[] {
    const levels: number[] = [];
    const lines = content.split('\n');

    for (const line of lines) {
      const level = this.parseHeaderLevel(line);
      if (level > 0) {
        levels.push(level);
      }
    }

    return levels;
  }

  private countInlineFormats(content: string): number {
    let count = 0;

    // Count bold
    count += (content.match(/\*\*(.*?)\*\*/g) || []).length;

    // Count italic
    count += (content.match(/(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g) || []).length;

    // Count code
    count += (content.match(/`([^`]+)`/g) || []).length;

    // Count wikilinks
    count += (content.match(/\[\[([^\]]+)\]\]/g) || []).length;

    return count;
  }

  private getWordCount(content: string): number {
    const plainText = stripMarkdown(content);
    if (!plainText) return 0;
    return plainText.split(/\s+/).filter((word) => word.length > 0).length;
  }

  private escapeHtml(text: string): string {
    // Handle both Node.js and browser environments
    if (typeof document !== 'undefined') {
      const div = document.createElement('div');
      div.textContent = text;
      return div.innerHTML;
    } else {
      // Fallback for Node.js/test environment
      return text
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
    }
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

/**
 * Singleton instance for application-wide use
 * Maintains performance through instance reuse
 */
export const contentProcessor = ContentProcessor.getInstance();

// ============================================================================
// Default Export for Easy Import
// ============================================================================

export default ContentProcessor;
