/**
 * Markdown Pattern Detection Service
 *
 * Detects markdown patterns in real-time as users type in ContentEditable elements.
 * Supports 4 block types and 3 inline types as defined in the architecture.
 */

import type {
  MarkdownPattern,
  PatternDetectionResult,
  MarkdownPatternType
} from '$lib/types/markdownPatterns';

export class MarkdownPatternDetector {
  private static instance: MarkdownPatternDetector;

  // Block patterns - must be at line start
  private blockPatterns = [
    {
      regex: /^(#{1,6})\s+(.+)$/gm,
      type: 'header' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        level: match[1].length,
        syntax: match[1] + ' ',
        content: match[2],
        raw: match[0]
      })
    },
    {
      regex: /^[-*+]\s+(.+)$/gm,
      type: 'bullet' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: match[0].substring(0, match[0].indexOf(match[1])),
        content: match[1],
        raw: match[0]
      })
    },
    {
      regex: /^>\s+(.+)$/gm,
      type: 'blockquote' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: '> ',
        content: match[1],
        raw: match[0]
      })
    },
    {
      regex: /^```(\w*)\n([\s\S]*?)```$/gm,
      type: 'codeblock' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: `\`\`\`${match[1]}\n`,
        content: match[2],
        raw: match[0],
        language: match[1] || 'text'
      })
    }
  ];

  // Inline patterns - can appear anywhere
  private inlinePatterns = [
    {
      regex: /\*\*(.*?)\*\*/g,
      type: 'bold' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: '**',
        content: match[1],
        raw: match[0]
      })
    },
    {
      regex: /(?<!\*)\*([^*]+)\*(?!\*)/g, // Negative lookbehind/ahead to avoid ** conflicts
      type: 'italic' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: '*',
        content: match[1],
        raw: match[0]
      })
    },
    {
      regex: /`([^`]+)`/g,
      type: 'inlinecode' as MarkdownPatternType,
      processor: (match: RegExpMatchArray) => ({
        syntax: '`',
        content: match[1],
        raw: match[0]
      })
    }
  ];

  public static getInstance(): MarkdownPatternDetector {
    if (!MarkdownPatternDetector.instance) {
      MarkdownPatternDetector.instance = new MarkdownPatternDetector();
    }
    return MarkdownPatternDetector.instance;
  }

  /**
   * Main detection method - finds all markdown patterns in content
   */
  public detectPatterns(content: string): PatternDetectionResult {
    const patterns: MarkdownPattern[] = [];

    // Detect block patterns first (they have precedence)
    const blockPatterns = this.detectBlockPatterns(content);
    patterns.push(...blockPatterns);

    // Then detect inline patterns (avoiding areas already covered by block patterns)
    const inlinePatterns = this.detectInlinePatterns(content, blockPatterns);
    patterns.push(...inlinePatterns);

    return {
      patterns,
      processedContent: content,
      hasPatterns: patterns.length > 0
    };
  }

  /**
   * Detect block-level patterns (headers, bullets, quotes, code blocks)
   */
  private detectBlockPatterns(content: string): MarkdownPattern[] {
    const patterns: MarkdownPattern[] = [];

    for (const pattern of this.blockPatterns) {
      const regex = new RegExp(pattern.regex.source, pattern.regex.flags);
      let match;

      while ((match = regex.exec(content)) !== null) {
        const processed = pattern.processor(match);

        patterns.push({
          type: pattern.type,
          level: (processed as { level?: number }).level,
          start: match.index,
          end: match.index + match[0].length,
          syntax: processed.syntax,
          content: processed.content,
          raw: processed.raw
        });
      }
    }

    return patterns;
  }

  /**
   * Detect inline patterns (bold, italic, code)
   */
  private detectInlinePatterns(
    content: string,
    blockPatterns: MarkdownPattern[]
  ): MarkdownPattern[] {
    const patterns: MarkdownPattern[] = [];

    for (const pattern of this.inlinePatterns) {
      const regex = new RegExp(pattern.regex.source, pattern.regex.flags);
      let match;

      while ((match = regex.exec(content)) !== null) {
        // Skip if this area is already covered by a block pattern
        if (
          this.isOverlappingWithBlocks(match.index, match.index + match[0].length, blockPatterns)
        ) {
          continue;
        }

        const processed = pattern.processor(match);

        patterns.push({
          type: pattern.type,
          start: match.index,
          end: match.index + match[0].length,
          syntax: processed.syntax,
          content: processed.content,
          raw: processed.raw
        });
      }
    }

    return patterns;
  }

  /**
   * Check if inline pattern overlaps with existing block patterns
   */
  private isOverlappingWithBlocks(
    start: number,
    end: number,
    blockPatterns: MarkdownPattern[]
  ): boolean {
    return blockPatterns.some((block) => {
      // Check for any overlap
      return start < block.end && end > block.start;
    });
  }

  /**
   * Get patterns at a specific cursor position (useful for editing)
   */
  public getPatternsAtPosition(content: string, position: number): MarkdownPattern[] {
    const result = this.detectPatterns(content);
    return result.patterns.filter(
      (pattern) => position >= pattern.start && position <= pattern.end
    );
  }

  /**
   * Check if content starts with a specific pattern type
   */
  public startsWithPattern(
    content: string,
    patternType: MarkdownPatternType
  ): MarkdownPattern | null {
    const result = this.detectPatterns(content);
    const firstPattern = result.patterns.find((p) => p.start === 0 && p.type === patternType);
    return firstPattern || null;
  }
}
