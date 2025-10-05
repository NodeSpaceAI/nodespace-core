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
import { eventBus } from './eventBus';
import type { NodeReferenceService, NodeReference, NodespaceLink } from './nodeReferenceService';
import { NodeDecoratorFactory } from './baseNodeDecoration';
import type { DecorationContext } from './baseNodeDecoration';
import type { ComponentDecoration } from '../types/componentDecoration';

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

export interface NodespaceRefNode extends ASTNode {
  type: 'nodespace-ref';
  nodeId: string;
  uri: string;
  displayText: string;
  rawSyntax: string;
  isValid: boolean;
  reference?: NodeReference;
  metadata?: Record<string, unknown>;
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
  hasNodespaceRefs: boolean;
  headerCount: number;
  inlineFormatCount: number;
  nodeRefCount: number;
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
  private readonly serviceName = 'ContentProcessor';
  private nodeReferenceService?: NodeReferenceService;

  // Performance optimization: Cache frequently accessed patterns
  private readonly HEADER_REGEX = /^(#{1,6})\s+(.*)$/gm;
  private readonly WIKILINK_REGEX = /\[\[([^[\]]+(?:\[[^[\]]*\][^[\]]*)*)\]\]/g;
  private readonly NODESPACE_REF_REGEX =
    /\[([^\]]+)\]\(nodespace:\/\/node\/([a-zA-Z0-9_-]+)(?:\?[^)]*)?\)/g;
  private readonly NODESPACE_URI_REGEX =
    /nodespace:\/\/([a-zA-Z0-9_-]+)\/([a-zA-Z0-9_-]+)(?:\/([a-zA-Z0-9_-]+))?/g;
  private readonly BOLD_REGEX = /\*\*(.*?)\*\*/g;
  private readonly ITALIC_REGEX = /(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g;
  private readonly CODE_REGEX = /`([^`]+)`/g;

  // Caching for nodespace references
  private readonly referenceCache = new Map<
    string,
    { reference: NodeReference | null; timestamp: number }
  >();
  private readonly cacheTimeout = 30000; // 30 seconds

  public static getInstance(): ContentProcessor {
    if (!ContentProcessor.instance) {
      ContentProcessor.instance = new ContentProcessor();
    }
    return ContentProcessor.instance;
  }

  private constructor() {
    // Set up EventBus integration for reference coordination
    this.setupEventBusIntegration();
  }

  /**
   * Set the NodeReferenceService for URI resolution
   */
  public setNodeReferenceService(service: NodeReferenceService): void {
    this.nodeReferenceService = service;
  }

  /**
   * Set up EventBus integration for reference coordination
   */
  private setupEventBusIntegration(): void {
    // Listen for references update needed events
    eventBus.subscribe('references:update-needed', (event) => {
      // Type assertion for specific event type
      const refEvent = event as import('./eventTypes').ReferencesUpdateNeededEvent;
      if (refEvent.updateType === 'content' || refEvent.updateType === 'deletion') {
        // When content changes or nodes are deleted, we might need to update references
        this.emitCacheInvalidate('node', refEvent.nodeId, 'reference content changed');

        // Clear reference cache for affected nodes
        this.invalidateReferenceCache(refEvent.nodeId);
      }
    });

    // Listen for node updates to invalidate reference cache
    eventBus.subscribe('node:updated', (event) => {
      const nodeEvent = event as import('./eventTypes').NodeUpdatedEvent;
      if (nodeEvent.updateType === 'content') {
        this.invalidateReferenceCache(nodeEvent.nodeId);
      }
    });

    // Listen for node deletion to clean up references
    eventBus.subscribe('node:deleted', (event) => {
      const nodeEvent = event as import('./eventTypes').NodeDeletedEvent;
      this.invalidateReferenceCache(nodeEvent.nodeId);
    });
  }

  /**
   * Invalidate cached references for a specific node
   */
  private invalidateReferenceCache(nodeId: string): void {
    const urisToRemove: string[] = [];

    for (const [uri, cached] of this.referenceCache) {
      if (cached.reference?.nodeId === nodeId) {
        urisToRemove.push(uri);
      }
    }

    for (const uri of urisToRemove) {
      this.referenceCache.delete(uri);
    }
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
      hasNodespaceRefs: (() => {
        this.NODESPACE_REF_REGEX.lastIndex = 0;
        this.NODESPACE_URI_REGEX.lastIndex = 0;
        return this.NODESPACE_REF_REGEX.test(source) || this.NODESPACE_URI_REGEX.test(source);
      })(),
      headerCount: 0,
      inlineFormatCount: 0,
      nodeRefCount: 0,
      lastModified: Date.now()
    };

    // Reset regex state
    this.NODESPACE_REF_REGEX.lastIndex = 0;
    this.NODESPACE_URI_REGEX.lastIndex = 0;

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

    // Count nodespace references
    metadata.nodeRefCount = this.countNodespaceRefs(source);

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
  public async renderAST(ast: MarkdownAST): Promise<string> {
    if (!ast.children || ast.children.length === 0) {
      return '';
    }

    const nodePromises = ast.children.map((node) => this.renderNode(node));
    const renderedNodes = await Promise.all(nodePromises);
    return renderedNodes.join('');
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
   * Enhanced with nodespace:// URI processing
   */
  public async markdownToDisplay(markdown: string): Promise<string> {
    const ast = this.parseMarkdown(markdown);
    return await this.renderAST(ast);
  }

  /**
   * Enhanced markdown to display with nodespace URI resolution
   * Resolves nodespace:// URIs and caches references for performance
   */
  public async markdownToDisplayWithReferences(
    markdown: string,
    sourceNodeId?: string
  ): Promise<string> {
    const ast = this.parseMarkdown(markdown);

    // Resolve any nodespace references if service is available
    if (this.nodeReferenceService) {
      await this.resolveNodespaceReferences(ast, sourceNodeId);
    }

    return await this.renderAST(ast);
  }

  /**
   * Legacy compatibility: display HTML back to markdown
   * Uses AST as intermediate representation for accuracy
   */
  public displayToMarkdown(html: string): string {
    // Use string-based approach for consistency across all environments
    // Future enhancement: Parse HTML → AST → Markdown
    return html.replace(/<[^>]*>/g, '').trim();
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
        hasNodespaceRefs: false,
        headerCount: 0,
        inlineFormatCount: 0,
        nodeRefCount: 0,
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
      const textNode: TextNode = {
        type: 'text',
        content: text,
        start: start,
        end: start + text.length
      };
      children.push(textNode);
    } else {
      // Mixed content with inline formatting
      let lastEnd = 0;

      for (const pattern of inlinePatterns) {
        // Add text before pattern
        if (pattern.start > lastEnd) {
          const textNode: TextNode = {
            type: 'text',
            content: text.substring(lastEnd, pattern.start),
            start: start + lastEnd,
            end: start + pattern.start
          };
          children.push(textNode);
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
        const textNode: TextNode = {
          type: 'text',
          content: text.substring(lastEnd),
          start: start + lastEnd,
          end: start + text.length
        };
        children.push(textNode);
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

    // Find nodespace:// references (markdown link format)
    this.NODESPACE_REF_REGEX.lastIndex = 0;
    while ((match = this.NODESPACE_REF_REGEX.exec(text)) !== null) {
      const displayText = match[1];
      const nodeId = match[2];
      const uri = match[0].substring(match[0].indexOf('nodespace://'), match[0].lastIndexOf(')'));

      patterns.push({
        type: 'nodespace-ref',
        nodeId,
        uri,
        displayText,
        rawSyntax: match[0],
        isValid: false, // Will be resolved later
        start: match.index,
        end: match.index + match[0].length
      } as NodespaceRefNode);
    }

    // Find plain nodespace:// URIs (not in markdown links)
    this.NODESPACE_URI_REGEX.lastIndex = 0;
    while ((match = this.NODESPACE_URI_REGEX.exec(text)) !== null) {
      // const nodeType = match[1]; // Node type not used in pattern creation
      const nodeId = match[2];
      const nodeName = match[3] || nodeId;
      const uri = match[0];

      patterns.push({
        type: 'nodespace-ref',
        nodeId,
        uri,
        displayText: nodeName.replace(/-/g, ' '), // Convert kebab-case to readable text
        rawSyntax: match[0],
        isValid: false, // Will be resolved later
        start: match.index,
        end: match.index + match[0].length
      } as NodespaceRefNode);
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

    // Sort by start position and remove overlapping patterns
    const sortedPatterns = patterns.sort((a, b) => a.start - b.start);

    // Filter out overlapping patterns, prioritizing by type:
    // 1. Markdown link-style nodespace references ([text](nodespace://...))
    // 2. Plain nodespace URIs (nodespace://...)
    const filteredPatterns: ASTNode[] = [];

    for (const pattern of sortedPatterns) {
      const isOverlapping = filteredPatterns.some((existing) => {
        return (
          (pattern.start >= existing.start && pattern.start < existing.end) ||
          (existing.start >= pattern.start && existing.start < pattern.end)
        );
      });

      if (!isOverlapping) {
        filteredPatterns.push(pattern);
      } else {
        // If overlapping, prioritize markdown link format over plain URI
        const overlappingIndex = filteredPatterns.findIndex((existing) => {
          return (
            (pattern.start >= existing.start && pattern.start < existing.end) ||
            (existing.start >= pattern.start && existing.start < pattern.end)
          );
        });

        if (overlappingIndex >= 0) {
          const existing = filteredPatterns[overlappingIndex];

          // If current pattern is a markdown-style nodespace-ref and existing is plain URI
          if (pattern.type === 'nodespace-ref' && existing.type === 'nodespace-ref') {
            const currentIsMarkdown = (pattern as NodespaceRefNode).rawSyntax?.startsWith('[');
            const existingIsMarkdown = (existing as NodespaceRefNode).rawSyntax?.startsWith('[');

            if (currentIsMarkdown && !existingIsMarkdown) {
              filteredPatterns[overlappingIndex] = pattern;
            }
            // If existing is markdown and current is plain URI, keep existing (do nothing)
          }
        }
      }
    }

    return filteredPatterns;
  }

  private async renderNode(node: ASTNode): Promise<string> {
    switch (node.type) {
      case 'header': {
        const headerNode = node as HeaderNode;
        return `<h${headerNode.level} class="ns-markdown-heading ns-markdown-h${headerNode.level}">${this.escapeHtml(headerNode.content)}</h${headerNode.level}>`;
      }

      case 'paragraph': {
        const paragraphNode = node as ParagraphNode;
        const childPromises = paragraphNode.children.map((child) => this.renderNode(child));
        const childContents = await Promise.all(childPromises);
        const content = childContents.join('');
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

      case 'nodespace-ref': {
        const refNode = node as NodespaceRefNode;

        // Use the rich decoration system if the reference is valid
        if (refNode.isValid && refNode.reference && this.nodeReferenceService) {
          try {
            // Get the actual node to determine its type
            const referencedNode = await this.nodeReferenceService.resolveNodespaceURI(refNode.uri);

            if (referencedNode) {
              // Create decoration context
              const decorationContext: DecorationContext = {
                nodeId: refNode.nodeId,
                nodeType: referencedNode.nodeType,
                title: refNode.reference.title || refNode.displayText,
                content: referencedNode.content,
                uri: refNode.uri,
                metadata: referencedNode.properties || {},
                targetElement: null!, // Will be set when DOM element is created
                displayContext: 'inline'
              };

              // Use NodeDecoratorFactory for rich decoration
              const decoratorFactory = new NodeDecoratorFactory(this.nodeReferenceService);
              const decorationResult = decoratorFactory.decorateReference(decorationContext);

              // Convert ComponentDecoration to HTML with hydration data
              return this.renderComponentDecorationAsHTML(decorationResult, decorationContext);
            }
          } catch (error) {
            console.warn(
              'ContentProcessor: Error rendering rich decoration for nodespace reference',
              { error, refNode }
            );
            // Fall through to basic rendering
          }
        }

        // Fallback to basic rendering for invalid references or if decoration fails
        const statusClass = refNode.isValid ? 'ns-noderef-valid' : 'ns-noderef-invalid';
        const title = refNode.reference?.title || refNode.displayText;
        const tooltip = refNode.isValid
          ? `Navigate to: ${title}`
          : `Broken reference: ${refNode.nodeId}`;

        return `<a class="ns-noderef ${statusClass}" 
                   href="${this.escapeHtml(refNode.uri)}" 
                   data-node-id="${this.escapeHtml(refNode.nodeId)}" 
                   data-uri="${this.escapeHtml(refNode.uri)}" 
                   title="${this.escapeHtml(tooltip)}">${this.escapeHtml(refNode.displayText)}</a>`;
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

      case 'nodespace-ref': {
        const refNode = node as NodespaceRefNode;
        return refNode.rawSyntax;
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

    // Count nodespace references
    count += (
      content.match(/\[([^\]]+)\]\(nodespace:\/\/node\/[a-zA-Z0-9_-]+(?:\?[^)]*)?\)/g) || []
    ).length;

    return count;
  }

  private getWordCount(content: string): number {
    const plainText = stripMarkdown(content);
    if (!plainText) return 0;
    return plainText.split(/\s+/).filter((word) => word.length > 0).length;
  }

  private escapeHtml(text: string): string {
    // Always use string-based approach for now due to DOM environment inconsistencies
    // This ensures consistent behavior across browser, Node.js, and test environments
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  // ============================================================================
  // Nodespace URI Processing Methods
  // ============================================================================

  /**
   * Detect nodespace:// URIs in content
   * Enhanced version of NodeReferenceService.detectNodespaceLinks for ContentProcessor
   */
  public detectNodespaceURIs(content: string): NodespaceLink[] {
    const links: NodespaceLink[] = [];
    const regex = /\[([^\]]+)\]\(nodespace:\/\/node\/([a-zA-Z0-9_-]+)(?:\?[^)]*)?\)/g;

    let match;
    while ((match = regex.exec(content)) !== null) {
      const displayText = match[1];
      const nodeId = match[2];
      const fullMatch = match[0];
      const uriMatch = fullMatch.match(/nodespace:\/\/node\/[a-zA-Z0-9_-]+(?:\?[^)]*)?/);
      const uri = uriMatch ? uriMatch[0] : `nodespace://node/${nodeId}`;

      // Check if reference is valid using cache or NodeReferenceService
      let isValid = false;
      if (this.nodeReferenceService) {
        const resolved = this.nodeReferenceService.resolveNodespaceURI(uri);
        isValid = !!resolved;
      }

      links.push({
        uri,
        startPos: match.index,
        endPos: match.index + fullMatch.length,
        nodeId,
        displayText,
        isValid,
        metadata: {
          fullMatch,
          contentBefore: content.substring(Math.max(0, match.index - 10), match.index),
          contentAfter: content.substring(
            match.index + fullMatch.length,
            Math.min(content.length, match.index + fullMatch.length + 10)
          )
        }
      });
    }

    return links;
  }

  /**
   * Resolve nodespace references in AST
   * Updates NodespaceRefNode objects with resolved references
   */
  private async resolveNodespaceReferences(ast: MarkdownAST, sourceNodeId?: string): Promise<void> {
    if (!this.nodeReferenceService) {
      return;
    }

    const resolveNodeReferences = async (nodes: ASTNode[]): Promise<void> => {
      for (const node of nodes) {
        if (node.type === 'nodespace-ref') {
          const refNode = node as NodespaceRefNode;
          await this.resolveNodeReference(refNode, sourceNodeId);
        } else if (node.children) {
          await resolveNodeReferences(node.children);
        }
      }
    };

    await resolveNodeReferences(ast.children);
  }

  /**
   * Resolve a single nodespace reference with caching
   */
  private async resolveNodeReference(
    refNode: NodespaceRefNode,
    sourceNodeId?: string
  ): Promise<void> {
    if (!this.nodeReferenceService) {
      return;
    }

    // Check cache first
    const cached = this.referenceCache.get(refNode.uri);
    if (cached && Date.now() - cached.timestamp < this.cacheTimeout) {
      refNode.reference = cached.reference || undefined;
      refNode.isValid = !!cached.reference;
      return;
    }

    try {
      // Parse and resolve URI
      const reference = this.nodeReferenceService.parseNodespaceURI(refNode.uri);

      // Cache the result
      this.referenceCache.set(refNode.uri, {
        reference,
        timestamp: Date.now()
      });

      // Update reference node
      refNode.reference = reference || undefined;
      refNode.isValid = !!reference?.isValid;

      // Emit reference resolution event
      if (sourceNodeId) {
        const resolutionEvent: Omit<import('./eventTypes').ReferenceResolutionEvent, 'timestamp'> =
          {
            type: 'reference:resolved',
            namespace: 'coordination',
            source: this.serviceName,
            referenceId: refNode.uri,
            target: refNode.nodeId,
            nodeId: sourceNodeId,
            resolutionResult: refNode.isValid ? 'found' : 'not-found',
            metadata: {
              displayText: refNode.displayText,
              cached: false
            }
          };
        eventBus.emit(resolutionEvent);
      }

      // Add bidirectional reference if valid and sourceNodeId provided
      if (refNode.isValid && sourceNodeId && reference) {
        try {
          await this.nodeReferenceService.addReference(sourceNodeId, refNode.nodeId);
        } catch (error) {
          console.warn('ContentProcessor: Failed to add bidirectional reference', {
            error,
            sourceNodeId,
            targetNodeId: refNode.nodeId
          });
        }
      }
    } catch (error) {
      console.error('ContentProcessor: Error resolving nodespace reference', {
        error,
        uri: refNode.uri
      });

      // Cache null result to avoid repeated failed lookups
      this.referenceCache.set(refNode.uri, {
        reference: null,
        timestamp: Date.now()
      });

      refNode.isValid = false;
    }
  }

  /**
   * Count nodespace references in content
   */
  private countNodespaceRefs(content: string): number {
    const matches = content.match(
      /\[([^\]]+)\]\(nodespace:\/\/node\/[a-zA-Z0-9_-]+(?:\?[^)]*)?\)/g
    );
    return matches ? matches.length : 0;
  }

  /**
   * Clear reference cache
   */
  public clearReferenceCache(): void {
    this.referenceCache.clear();
  }

  /**
   * Get cache statistics for debugging
   */
  public getReferencesCacheStats(): { size: number; hitRate: number; oldestEntry: number } {
    const now = Date.now();
    let oldestTimestamp = now;

    for (const [, cached] of this.referenceCache) {
      if (cached.timestamp < oldestTimestamp) {
        oldestTimestamp = cached.timestamp;
      }
    }

    return {
      size: this.referenceCache.size,
      hitRate: 0, // Would need tracking to calculate
      oldestEntry: now - oldestTimestamp
    };
  }

  // ============================================================================
  // EventBus Integration Methods
  // ============================================================================

  /**
   * Emit cache invalidation event
   */
  private emitCacheInvalidate(
    scope: 'single' | 'node' | 'global',
    nodeId?: string,
    reason?: string
  ): void {
    const cacheEvent: import('./eventTypes').CacheInvalidateEvent = {
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: this.serviceName,
      timestamp: Date.now(),
      cacheKey: nodeId ? `contentProcessor:${nodeId}` : 'contentProcessor:global',
      scope,
      nodeId,
      reason: reason || 'content processing'
    };
    eventBus.emit(cacheEvent);
  }

  /**
   * Process content and emit wikilink detection events
   * Enhanced with nodespace:// URI detection
   */
  public processContentWithEventEmission(content: string, nodeId: string): PreparedContent {
    const prepared = this.prepareBacklinkSyntax(content);

    // Emit events for detected wikilinks (Phase 2+ preparation)
    for (const wikiLink of prepared.wikiLinks) {
      const backlinkEvent: import('./eventTypes').BacklinkDetectedEvent = {
        type: 'backlink:detected',
        namespace: 'phase2',
        source: this.serviceName,
        timestamp: Date.now(),
        sourceNodeId: nodeId,
        targetNodeId: wikiLink.target, // In Phase 2, this will be resolved to actual node ID
        linkType: 'wikilink',
        linkText: wikiLink.displayText || '',
        metadata: {
          startPos: wikiLink.startPos,
          endPos: wikiLink.endPos,
          target: wikiLink.target
        }
      };
      eventBus.emit(backlinkEvent);
    }

    // Detect and emit events for nodespace:// references
    const nodespaceLinks = this.detectNodespaceURIs(content);
    for (const nodeLink of nodespaceLinks) {
      // Emit backlink detected event
      const backlinkEvent: import('./eventTypes').BacklinkDetectedEvent = {
        type: 'backlink:detected',
        namespace: 'phase2',
        source: this.serviceName,
        timestamp: Date.now(),
        sourceNodeId: nodeId,
        targetNodeId: nodeLink.nodeId,
        linkType: 'nodespace-ref',
        linkText: nodeLink.displayText || '',
        metadata: {
          startPos: nodeLink.startPos,
          endPos: nodeLink.endPos,
          uri: nodeLink.uri,
          isValid: nodeLink.isValid
        }
      };
      eventBus.emit(backlinkEvent);

      // Emit reference resolution event
      const resolutionEvent: Omit<import('./eventTypes').ReferenceResolutionEvent, 'timestamp'> = {
        type: 'reference:resolved',
        namespace: 'coordination',
        source: this.serviceName,
        referenceId: nodeLink.uri,
        target: nodeLink.nodeId,
        nodeId: nodeId,
        resolutionResult: nodeLink.isValid ? 'found' : 'not-found',
        metadata: {
          displayText: nodeLink.displayText,
          startPos: nodeLink.startPos,
          endPos: nodeLink.endPos
        }
      };
      eventBus.emit(resolutionEvent);
    }

    return prepared;
  }

  /**
   * Enhanced content processing with full nodespace:// support
   * Processes both wikilinks and nodespace references
   */
  public async processContentWithReferences(
    content: string,
    nodeId: string
  ): Promise<{
    prepared: PreparedContent;
    nodespaceLinks: NodespaceLink[];
    resolved: boolean;
  }> {
    // Process traditional wikilinks
    const prepared = this.prepareBacklinkSyntax(content);

    // Process nodespace references
    const nodespaceLinks = this.detectNodespaceURIs(content);

    // Resolve references if service is available
    let resolved = false;
    if (this.nodeReferenceService) {
      for (const link of nodespaceLinks) {
        try {
          const reference = this.nodeReferenceService.parseNodespaceURI(link.uri);
          if (reference?.isValid) {
            // Add bidirectional reference
            await this.nodeReferenceService.addReference(nodeId, link.nodeId);
            resolved = true;
          }
        } catch (error) {
          console.warn('ContentProcessor: Failed to resolve reference', { error, uri: link.uri });
        }
      }
    }

    // Emit events
    this.processContentWithEventEmission(content, nodeId);

    return {
      prepared,
      nodespaceLinks,
      resolved
    };
  }

  /**
   * Renders a ComponentDecoration as HTML with hydration data
   * This creates placeholder HTML that can be hydrated with Svelte components later
   *
   * Design for plugin architecture:
   * - Core node types (text, task, date, user) are built-in
   * - Plugin node types (pdf, image, etc.) can register their own components
   * - The hydration system will dynamically load the appropriate component
   */
  private renderComponentDecorationAsHTML(
    decoration: ComponentDecoration,
    context: DecorationContext
  ): string {
    const { props, metadata } = decoration;

    // Extract component information for plugin system
    // For Svelte components, use a consistent naming scheme
    const componentName = 'BaseNodeReference'; // All components use BaseNodeReference for now
    const nodeType = props.nodeType || context.nodeType;

    // Safely serialize props and metadata for hydration
    // Use HTML entity encoding for quotes to preserve JSON structure
    const propsJSON = JSON.stringify(props).replace(/"/g, '&quot;');
    const metadataJSON = JSON.stringify(metadata || {}).replace(/"/g, '&quot;');

    // Create placeholder that the hydration system can find and replace
    const displayText = (props.content as string) || context.title || context.content || '';
    return `<span class="ns-component-placeholder" data-component="${componentName}" data-node-type="${nodeType}" data-props="${propsJSON}" data-metadata="${metadataJSON}" data-node-id="${props.nodeId || context.nodeId}" data-hydrate="pending">${this.escapeHtml(displayText)}</span>`;
  }

  /**
   * Reset ContentProcessor state for testing
   * Clears caches and resets dependencies
   */
  resetForTesting(): void {
    this.referenceCache.clear();
    this.nodeReferenceService = undefined;
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
