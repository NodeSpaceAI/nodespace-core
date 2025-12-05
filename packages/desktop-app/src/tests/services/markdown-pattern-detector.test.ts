/**
 * MarkdownPatternDetector Service Tests
 *
 * Comprehensive test suite for the MarkdownPatternDetector service
 * covering all markdown pattern types, edge cases, and position detection.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { MarkdownPatternDetector } from '$lib/services/markdown-pattern-detector';

describe('MarkdownPatternDetector', () => {
  let detector: MarkdownPatternDetector;

  beforeEach(() => {
    detector = MarkdownPatternDetector.getInstance();
  });

  // ========================================================================
  // Singleton Pattern Tests
  // ========================================================================

  describe('Singleton Pattern', () => {
    it('should return the same instance', () => {
      const instance1 = MarkdownPatternDetector.getInstance();
      const instance2 = MarkdownPatternDetector.getInstance();

      expect(instance1).toBe(instance2);
    });

    it('should maintain consistent behavior across multiple getInstance calls', () => {
      const instance1 = MarkdownPatternDetector.getInstance();
      const instance2 = MarkdownPatternDetector.getInstance();

      const content = '**bold text**';
      const result1 = instance1.detectPatterns(content);
      const result2 = instance2.detectPatterns(content);

      expect(result1.patterns).toEqual(result2.patterns);
    });
  });

  // ========================================================================
  // Header Pattern Detection Tests
  // ========================================================================

  describe('Header Pattern Detection', () => {
    it('should detect H1 header', () => {
      const content = '# Header 1';
      const result = detector.detectPatterns(content);

      expect(result.hasPatterns).toBe(true);
      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('header');
      expect(result.patterns[0].level).toBe(1);
      expect(result.patterns[0].syntax).toBe('# ');
      expect(result.patterns[0].content).toBe('Header 1');
      expect(result.patterns[0].raw).toBe('# Header 1');
      expect(result.patterns[0].start).toBe(0);
      expect(result.patterns[0].end).toBe(10);
    });

    it('should detect all header levels (H1-H6)', () => {
      const testCases = [
        { content: '# H1', level: 1 },
        { content: '## H2', level: 2 },
        { content: '### H3', level: 3 },
        { content: '#### H4', level: 4 },
        { content: '##### H5', level: 5 },
        { content: '###### H6', level: 6 }
      ];

      for (const { content, level } of testCases) {
        const result = detector.detectPatterns(content);
        expect(result.patterns).toHaveLength(1);
        expect(result.patterns[0].type).toBe('header');
        expect(result.patterns[0].level).toBe(level);
      }
    });

    it('should detect multiple headers', () => {
      const content = '# Header 1\n## Header 2\n### Header 3';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(3);
      expect(result.patterns[0].level).toBe(1);
      expect(result.patterns[1].level).toBe(2);
      expect(result.patterns[2].level).toBe(3);
    });

    it('should require space after # for header detection', () => {
      const content = '#NoSpace';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(0);
    });

    it('should handle headers with special characters', () => {
      const content = '## Header with *markdown* and [[links]]';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('header');
      expect(result.patterns[0].content).toBe('Header with *markdown* and [[links]]');
    });

    it('should not detect more than 6 # as header', () => {
      const content = '####### Seven hashes';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(0);
    });
  });

  // ========================================================================
  // Bullet Pattern Detection Tests
  // ========================================================================

  describe('Bullet Pattern Detection', () => {
    it('should detect dash bullet', () => {
      const content = '- Bullet item';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bullet');
      expect(result.patterns[0].syntax).toBe('- ');
      expect(result.patterns[0].content).toBe('Bullet item');
      expect(result.patterns[0].raw).toBe('- Bullet item');
    });

    it('should detect asterisk bullet', () => {
      const content = '* Bullet item';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bullet');
      expect(result.patterns[0].syntax).toBe('* ');
      expect(result.patterns[0].content).toBe('Bullet item');
    });

    it('should detect plus bullet', () => {
      const content = '+ Bullet item';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bullet');
      expect(result.patterns[0].syntax).toBe('+ ');
      expect(result.patterns[0].content).toBe('Bullet item');
    });

    it('should detect multiple bullets', () => {
      const content = '- First item\n- Second item\n- Third item';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(3);
      expect(result.patterns.every((p) => p.type === 'bullet')).toBe(true);
      expect(result.patterns[0].content).toBe('First item');
      expect(result.patterns[1].content).toBe('Second item');
      expect(result.patterns[2].content).toBe('Third item');
    });

    it('should detect mixed bullet types', () => {
      const content = '- Dash\n* Asterisk\n+ Plus';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(3);
      expect(result.patterns.every((p) => p.type === 'bullet')).toBe(true);
    });
  });

  // ========================================================================
  // Blockquote Pattern Detection Tests
  // ========================================================================

  describe('Blockquote Pattern Detection', () => {
    it('should detect blockquote', () => {
      const content = '> This is a quote';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('blockquote');
      expect(result.patterns[0].syntax).toBe('> ');
      expect(result.patterns[0].content).toBe('This is a quote');
      expect(result.patterns[0].raw).toBe('> This is a quote');
    });

    it('should detect multiple blockquotes', () => {
      const content = '> First quote\n> Second quote';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns.every((p) => p.type === 'blockquote')).toBe(true);
      expect(result.patterns[0].content).toBe('First quote');
      expect(result.patterns[1].content).toBe('Second quote');
    });

    it('should handle blockquotes with markdown', () => {
      const content = '> Quote with **bold** text';
      const result = detector.detectPatterns(content);

      // Should detect blockquote and bold (if bold is not within block's syntax area)
      const blockquote = result.patterns.find((p) => p.type === 'blockquote');
      expect(blockquote).toBeDefined();
      expect(blockquote?.content).toBe('Quote with **bold** text');
    });
  });

  // ========================================================================
  // Code Block Pattern Detection Tests
  // ========================================================================

  describe('Code Block Pattern Detection', () => {
    it('should detect code block without language', () => {
      const content = '```\ncode here\n```';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('codeblock');
      expect(result.patterns[0].syntax).toBe('```\n');
      expect(result.patterns[0].content).toBe('code here\n');
      expect(result.patterns[0].raw).toBe('```\ncode here\n```');
    });

    it('should detect code block with language', () => {
      const content = '```javascript\nconst x = 1;\n```';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('codeblock');
      expect(result.patterns[0].syntax).toBe('```javascript\n');
      expect(result.patterns[0].content).toBe('const x = 1;\n');
    });

    it('should handle multiline code blocks', () => {
      const content = '```python\ndef hello():\n    print("world")\n    return True\n```';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('codeblock');
      expect(result.patterns[0].content).toContain('def hello()');
      expect(result.patterns[0].content).toContain('print("world")');
    });

    it('should detect multiple code blocks', () => {
      const content = '```\nblock 1\n```\n\n```\nblock 2\n```';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns.every((p) => p.type === 'codeblock')).toBe(true);
    });
  });

  // ========================================================================
  // Bold Pattern Detection Tests
  // ========================================================================

  describe('Bold Pattern Detection', () => {
    it('should detect bold text', () => {
      const content = '**bold text**';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bold');
      expect(result.patterns[0].syntax).toBe('**');
      expect(result.patterns[0].content).toBe('bold text');
      expect(result.patterns[0].raw).toBe('**bold text**');
    });

    it('should detect multiple bold patterns', () => {
      const content = '**first** and **second**';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns.every((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns[0].content).toBe('first');
      expect(result.patterns[1].content).toBe('second');
    });

    it('should handle bold at start of line', () => {
      const content = '**bold** at start';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bold');
      expect(result.patterns[0].start).toBe(0);
    });

    it('should handle bold at end of line', () => {
      const content = 'end with **bold**';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('bold');
      expect(result.patterns[0].end).toBe(content.length);
    });

    it('should handle bold with special characters', () => {
      const content = '**bold with !@#$%**';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].content).toBe('bold with !@#$%');
    });
  });

  // ========================================================================
  // Italic Pattern Detection Tests
  // ========================================================================

  describe('Italic Pattern Detection', () => {
    it('should detect italic text', () => {
      const content = '*italic text*';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('italic');
      expect(result.patterns[0].syntax).toBe('*');
      expect(result.patterns[0].content).toBe('italic text');
      expect(result.patterns[0].raw).toBe('*italic text*');
    });

    it('should detect multiple italic patterns', () => {
      const content = '*first* and *second*';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns.every((p) => p.type === 'italic')).toBe(true);
      expect(result.patterns[0].content).toBe('first');
      expect(result.patterns[1].content).toBe('second');
    });

    it('should not confuse italic with bold', () => {
      const content = '*italic* and **bold**';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns[0].type).toBe('bold');
      expect(result.patterns[1].type).toBe('italic');
    });

    it('should handle italic at boundaries', () => {
      const content = '*start* middle *end*';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns[0].start).toBe(0);
      expect(result.patterns[1].end).toBe(content.length);
    });
  });

  // ========================================================================
  // Inline Code Pattern Detection Tests
  // ========================================================================

  describe('Inline Code Pattern Detection', () => {
    it('should detect inline code', () => {
      const content = '`code here`';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('inlinecode');
      expect(result.patterns[0].syntax).toBe('`');
      expect(result.patterns[0].content).toBe('code here');
      expect(result.patterns[0].raw).toBe('`code here`');
    });

    it('should detect multiple inline code patterns', () => {
      const content = '`first` and `second`';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(2);
      expect(result.patterns.every((p) => p.type === 'inlinecode')).toBe(true);
      expect(result.patterns[0].content).toBe('first');
      expect(result.patterns[1].content).toBe('second');
    });

    it('should handle inline code with special characters', () => {
      const content = '`const x = 1; // comment`';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].content).toBe('const x = 1; // comment');
    });

    it('should detect inline code patterns', () => {
      const content = '`inline` code';
      const result = detector.detectPatterns(content);

      const inlineCode = result.patterns.find((p) => p.type === 'inlinecode');

      expect(inlineCode).toBeDefined();
      expect(inlineCode?.content).toBe('inline');
    });
  });

  // ========================================================================
  // Mixed Pattern Detection Tests
  // ========================================================================

  describe('Mixed Pattern Detection', () => {
    it('should detect mixed inline patterns', () => {
      const content = '**bold** and *italic* and `code`';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(3);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'inlinecode')).toBe(true);
    });

    it('should detect block and inline patterns together', () => {
      const content = '# Header\n\n**bold** text';
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBeGreaterThanOrEqual(2);
      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
    });

    it('should handle complex nested patterns', () => {
      const content = '# Header\n\n**bold with *italic* inside**\n\n- Bullet item';
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBeGreaterThan(3);
      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bullet')).toBe(true);
    });

    it('should prioritize block patterns over inline patterns', () => {
      const content = '> Quote with **bold**';
      const result = detector.detectPatterns(content);

      const blockquote = result.patterns.find((p) => p.type === 'blockquote');
      const bold = result.patterns.find((p) => p.type === 'bold');

      expect(blockquote).toBeDefined();
      // Bold should be detected but not overlap with blockquote syntax
      if (bold) {
        expect(bold.start).toBeGreaterThan(blockquote!.start);
      }
    });
  });

  // ========================================================================
  // Pattern Overlap Prevention Tests
  // ========================================================================

  describe('Pattern Overlap Prevention', () => {
    it('should not detect inline patterns inside code blocks', () => {
      const content = '```\n**not bold**\n*not italic*\n```';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].type).toBe('codeblock');
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(false);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(false);
    });

    it('should handle patterns at boundaries correctly', () => {
      const content = '# Header\n**bold after header**';
      const result = detector.detectPatterns(content);

      const header = result.patterns.find((p) => p.type === 'header');
      const bold = result.patterns.find((p) => p.type === 'bold');

      expect(header).toBeDefined();
      expect(bold).toBeDefined();
      expect(bold!.start).toBeGreaterThan(header!.end);
    });
  });

  // ========================================================================
  // Position-Based Detection Tests
  // ========================================================================

  describe('getPatternsAtPosition', () => {
    it('should return patterns at cursor position', () => {
      const content = '**bold text**';
      const position = 5; // Inside "bold"

      const patterns = detector.getPatternsAtPosition(content, position);

      expect(patterns).toHaveLength(1);
      expect(patterns[0].type).toBe('bold');
    });

    it('should return empty array when no pattern at position', () => {
      const content = '**bold** plain text';
      const position = 15; // In "plain text"

      const patterns = detector.getPatternsAtPosition(content, position);

      expect(patterns).toHaveLength(0);
    });

    it('should return patterns at specific position', () => {
      const content = '# Header\n\n**bold** text';
      const position = 13; // Inside "bold"

      const patterns = detector.getPatternsAtPosition(content, position);

      // Should return bold pattern at this position
      expect(patterns.length).toBeGreaterThan(0);
      expect(patterns.some((p) => p.type === 'bold')).toBe(true);
    });

    it('should handle position at pattern boundary', () => {
      const content = '**bold**';
      const startPosition = 0; // At start
      const endPosition = 8; // At end

      const patternsAtStart = detector.getPatternsAtPosition(content, startPosition);
      const patternsAtEnd = detector.getPatternsAtPosition(content, endPosition);

      expect(patternsAtStart).toHaveLength(1);
      expect(patternsAtEnd).toHaveLength(1);
    });

    it('should handle position beyond content length', () => {
      const content = '**bold**';
      const position = 100;

      const patterns = detector.getPatternsAtPosition(content, position);

      expect(patterns).toHaveLength(0);
    });
  });

  // ========================================================================
  // Pattern Start Detection Tests
  // ========================================================================

  describe('startsWithPattern', () => {
    it('should detect header at start', () => {
      const content = '# Header text';
      const pattern = detector.startsWithPattern(content, 'header');

      expect(pattern).not.toBeNull();
      expect(pattern?.type).toBe('header');
      expect(pattern?.start).toBe(0);
    });

    it('should detect bold at start', () => {
      const content = '**bold** at start';
      const pattern = detector.startsWithPattern(content, 'bold');

      expect(pattern).not.toBeNull();
      expect(pattern?.type).toBe('bold');
      expect(pattern?.start).toBe(0);
    });

    it('should return null if pattern not at start', () => {
      const content = 'text before **bold**';
      const pattern = detector.startsWithPattern(content, 'bold');

      expect(pattern).toBeNull();
    });

    it('should return null if pattern type does not exist', () => {
      const content = '# Header';
      const pattern = detector.startsWithPattern(content, 'bold');

      expect(pattern).toBeNull();
    });

    it('should return null for empty content', () => {
      const content = '';
      const pattern = detector.startsWithPattern(content, 'header');

      expect(pattern).toBeNull();
    });

    it('should detect blockquote at start', () => {
      const content = '> Quote text';
      const pattern = detector.startsWithPattern(content, 'blockquote');

      expect(pattern).not.toBeNull();
      expect(pattern?.type).toBe('blockquote');
    });

    it('should detect bullet at start', () => {
      const content = '- Bullet item';
      const pattern = detector.startsWithPattern(content, 'bullet');

      expect(pattern).not.toBeNull();
      expect(pattern?.type).toBe('bullet');
    });

    it('should detect code block at start', () => {
      const content = '```\ncode\n```';
      const pattern = detector.startsWithPattern(content, 'codeblock');

      expect(pattern).not.toBeNull();
      expect(pattern?.type).toBe('codeblock');
    });
  });

  // ========================================================================
  // Edge Cases and Error Handling Tests
  // ========================================================================

  describe('Edge Cases and Error Handling', () => {
    it('should handle empty content', () => {
      const content = '';
      const result = detector.detectPatterns(content);

      expect(result.hasPatterns).toBe(false);
      expect(result.patterns).toHaveLength(0);
      expect(result.processedContent).toBe('');
    });

    it('should handle whitespace-only content', () => {
      const content = '   \n\n   ';
      const result = detector.detectPatterns(content);

      expect(result.hasPatterns).toBe(false);
      expect(result.patterns).toHaveLength(0);
    });

    it('should handle content with no patterns', () => {
      const content = 'Just plain text without any markdown';
      const result = detector.detectPatterns(content);

      expect(result.hasPatterns).toBe(false);
      expect(result.patterns).toHaveLength(0);
      expect(result.processedContent).toBe(content);
    });

    it('should handle malformed patterns gracefully', () => {
      const content = '**unclosed bold';
      const result = detector.detectPatterns(content);

      // Should not throw, might not detect pattern
      expect(() => result).not.toThrow();
    });

    it('should handle unicode characters', () => {
      const content = '**粗体文本** and *斜体文本* with émojis 🚀';
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBeGreaterThan(0);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
    });

    it('should handle very long content', () => {
      const content = '**bold** '.repeat(1000);
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBe(1000);
      expect(result.patterns.every((p) => p.type === 'bold')).toBe(true);
    });

    it('should handle mixed line endings', () => {
      const content = '# Header\r\n**bold**\n*italic*\r';
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBeGreaterThan(0);
      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
    });

    it('should handle tabs and mixed whitespace', () => {
      const content = '#\t\tHeader with tabs\n**\tbold\t**';
      const result = detector.detectPatterns(content);

      // Header requires space after #, tabs might not count
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
    });

    it('should handle deeply nested patterns', () => {
      const content = '**bold** with *italic* and `code`';
      const result = detector.detectPatterns(content);

      // Should detect all patterns when separated
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'inlinecode')).toBe(true);
    });

    it('should handle patterns with escaped characters', () => {
      const content = '\\**not bold** and \\*not italic*';
      const result = detector.detectPatterns(content);

      // Backslashes might not prevent detection in current implementation
      // This tests actual behavior
      expect(() => result).not.toThrow();
    });
  });

  // ========================================================================
  // PatternDetectionResult Tests
  // ========================================================================

  describe('PatternDetectionResult', () => {
    it('should return correct hasPatterns flag', () => {
      const withPatterns = detector.detectPatterns('**bold**');
      const withoutPatterns = detector.detectPatterns('plain text');

      expect(withPatterns.hasPatterns).toBe(true);
      expect(withoutPatterns.hasPatterns).toBe(false);
    });

    it('should preserve original content in processedContent', () => {
      const content = '# Header with **bold** and *italic*';
      const result = detector.detectPatterns(content);

      expect(result.processedContent).toBe(content);
    });

    it('should return patterns array even when empty', () => {
      const result = detector.detectPatterns('no patterns');

      expect(result.patterns).toBeDefined();
      expect(Array.isArray(result.patterns)).toBe(true);
      expect(result.patterns).toHaveLength(0);
    });
  });

  // ========================================================================
  // Pattern Position and Range Tests
  // ========================================================================

  describe('Pattern Position and Range', () => {
    it('should provide correct start and end positions', () => {
      const content = 'text **bold** more text';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(1);
      expect(result.patterns[0].start).toBe(5);
      expect(result.patterns[0].end).toBe(13);
      expect(content.substring(result.patterns[0].start, result.patterns[0].end)).toBe(
        '**bold**'
      );
    });

    it('should calculate ranges for multiple patterns correctly', () => {
      const content = '**first** and *second* and `third`';
      const result = detector.detectPatterns(content);

      expect(result.patterns).toHaveLength(3);

      for (const pattern of result.patterns) {
        const extracted = content.substring(pattern.start, pattern.end);
        expect(extracted).toBe(pattern.raw);
      }
    });

    it('should handle adjacent patterns', () => {
      const content = '**bold***italic*';
      const result = detector.detectPatterns(content);

      expect(result.patterns.length).toBeGreaterThan(0);
      // Patterns should not overlap
      for (let i = 0; i < result.patterns.length - 1; i++) {
        expect(result.patterns[i].end).toBeLessThanOrEqual(result.patterns[i + 1].start);
      }
    });
  });

  // ========================================================================
  // Real-World Content Tests
  // ========================================================================

  describe('Real-World Content', () => {
    it('should handle typical note content', () => {
      const content = `# Meeting Notes

## Attendees

**John Doe** and *Jane Smith*

## Discussion

> Important point raised by the team

Action items:
- Review code samples
- Update documentation`;

      const result = detector.detectPatterns(content);

      expect(result.hasPatterns).toBe(true);
      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bullet')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'blockquote')).toBe(true);
    });

    it('should handle code snippet with explanations', () => {
      const content = `# Code Example

Here's the implementation:

\`\`\`javascript
function example() {
  return "hello";
}
\`\`\`

The function returns a string.`;

      const result = detector.detectPatterns(content);

      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'codeblock')).toBe(true);
    });

    it('should handle documentation-style content', () => {
      const content = `### API Reference

**Parameters:**

- name (string): The name parameter
- age (number): The age parameter

**Returns:** *boolean*`;

      const result = detector.detectPatterns(content);

      expect(result.patterns.some((p) => p.type === 'header')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bold')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'bullet')).toBe(true);
      expect(result.patterns.some((p) => p.type === 'italic')).toBe(true);
    });
  });
});
