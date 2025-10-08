/**
 * WYSIWYG Processing Service
 *
 * Handles hiding markdown syntax and applying visual formatting
 * to ContentEditable elements in real-time.
 */

import type { MarkdownPattern, WYSIWYGTransformation } from '../types/markdown-patterns';
import { WYSIWYG_TRANSFORMATIONS } from '../types/markdown-patterns';

export interface WYSIWYGProcessedResult {
  htmlContent: string;
  rawContent: string;
  appliedTransformations: string[];
}

export class WYSIWYGProcessor {
  private static instance: WYSIWYGProcessor;

  public static getInstance(): WYSIWYGProcessor {
    if (!WYSIWYGProcessor.instance) {
      WYSIWYGProcessor.instance = new WYSIWYGProcessor();
    }
    return WYSIWYGProcessor.instance;
  }

  /**
   * Process content for WYSIWYG display - hide syntax, show formatting
   */
  public processForDisplay(content: string, patterns: MarkdownPattern[]): WYSIWYGProcessedResult {
    if (patterns.length === 0) {
      return {
        htmlContent: this.escapeHtml(content),
        rawContent: content,
        appliedTransformations: []
      };
    }

    // Sort patterns by start position (reverse order for proper replacement)
    const sortedPatterns = [...patterns].sort((a, b) => b.start - a.start);

    let processedContent = content;
    const appliedTransformations: string[] = [];

    // Process each pattern from end to start to maintain position indices
    for (const pattern of sortedPatterns) {
      const transformation = this.getTransformationForPattern(pattern);
      if (transformation && transformation.hideMarkup) {
        const before = processedContent.substring(0, pattern.start);
        const after = processedContent.substring(pattern.end);

        // Create styled content without syntax markers
        const styledContent = this.createStyledContent(pattern, transformation);
        processedContent = before + styledContent + after;

        appliedTransformations.push(`${pattern.type}${pattern.level ? `-${pattern.level}` : ''}`);
      }
    }

    return {
      htmlContent: processedContent,
      rawContent: content,
      appliedTransformations
    };
  }

  /**
   * Apply WYSIWYG styling to ContentEditable element
   */
  public applyToContentEditable(
    element: HTMLElement,
    content: string,
    patterns: MarkdownPattern[]
  ): void {
    const result = this.processForDisplay(content, patterns);

    // Store raw content for later retrieval
    element.dataset.rawContent = result.rawContent;
    element.innerHTML = result.htmlContent;

    // Add CSS classes for styling
    element.classList.remove(...this.getAllTransformationClasses());

    if (patterns.length > 0) {
      const blockPattern = patterns.find((p) =>
        ['header', 'blockquote', 'codeblock'].includes(p.type)
      );
      if (blockPattern) {
        const transformation = this.getTransformationForPattern(blockPattern);
        if (transformation) {
          element.classList.add(transformation.cssClass);
        }
      }
    }
  }

  /**
   * Extract raw content from ContentEditable element with WYSIWYG applied
   */
  public extractRawContent(element: HTMLElement): string {
    // First try to get stored raw content
    const storedRaw = element.dataset.rawContent;
    if (storedRaw) {
      return storedRaw;
    }

    // Fallback: extract from current content (less reliable)
    return element.textContent || '';
  }

  /**
   * Get transformation rules for a specific pattern
   */
  private getTransformationForPattern(pattern: MarkdownPattern) {
    if (pattern.type === 'header' && pattern.level) {
      return WYSIWYG_TRANSFORMATIONS[`header-${pattern.level}`];
    }
    return WYSIWYG_TRANSFORMATIONS[pattern.type];
  }

  /**
   * Create styled content without syntax markers
   */
  private createStyledContent(
    pattern: MarkdownPattern,
    transformation: WYSIWYGTransformation
  ): string {
    const escapedContent = this.escapeHtml(pattern.content);

    if (transformation.htmlTag) {
      return `<${transformation.htmlTag} class="${transformation.cssClass}">${escapedContent}</${transformation.htmlTag}>`;
    } else {
      return `<span class="${transformation.cssClass}">${escapedContent}</span>`;
    }
  }

  /**
   * Get all CSS classes used by transformations
   */
  private getAllTransformationClasses(): string[] {
    return Object.values(WYSIWYG_TRANSFORMATIONS).map((t) => t.cssClass);
  }

  /**
   * Simple HTML escaping for security
   */
  private escapeHtml(text: string): string {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  /**
   * Check if element currently has WYSIWYG formatting applied
   */
  public hasWYSIWYGFormatting(element: HTMLElement): boolean {
    return this.getAllTransformationClasses().some((className) =>
      element.classList.contains(className)
    );
  }

  /**
   * Remove all WYSIWYG formatting from element
   */
  public removeWYSIWYGFormatting(element: HTMLElement): void {
    element.classList.remove(...this.getAllTransformationClasses());
    const rawContent = this.extractRawContent(element);
    element.textContent = rawContent;
    delete element.dataset.rawContent;
  }
}
