/**
 * Markdown Context Tests
 *
 * Tests markdown formatting detection and marker generation
 * for inline formatting preservation during content splitting
 */

import { describe, it, expect } from 'vitest';
import {
  analyzeMarkdownContext,
  getClosingMarkers,
  getOpeningMarkers,
  hasActiveFormatting
} from '../../lib/patterns/markdown-context';

describe('Markdown Context Analysis', () => {
  describe('analyzeMarkdownContext - Bold', () => {
    it('should detect bold formatting with **', () => {
      const content = '**bold text**';
      const context = analyzeMarkdownContext(content, 5); // **bol|

      expect(context.bold).toBe(true);
      expect(context.openMarkers).toContain('**');
    });

    it('should detect bold formatting with __', () => {
      const content = '__bold text__';
      const context = analyzeMarkdownContext(content, 5); // __bol|

      expect(context.bold).toBe(true);
      expect(context.openMarkers).toContain('__');
    });

    it('should close bold formatting when closing marker encountered', () => {
      const content = '**bold** text';
      const context = analyzeMarkdownContext(content, 9); // **bold** |text

      expect(context.bold).toBe(false);
    });
  });

  describe('analyzeMarkdownContext - Italic', () => {
    it('should detect italic formatting with *', () => {
      const content = '*italic text*';
      const context = analyzeMarkdownContext(content, 5); // *ital|

      expect(context.italic).toBe(true);
      expect(context.openMarkers).toContain('*');
    });

    it('should detect italic formatting with _', () => {
      const content = '_italic text_';
      const context = analyzeMarkdownContext(content, 5); // _ital|

      expect(context.italic).toBe(true);
      expect(context.openMarkers).toContain('_');
    });

    it('should close italic formatting', () => {
      const content = '*italic* text';
      const context = analyzeMarkdownContext(content, 9); // *italic* |

      expect(context.italic).toBe(false);
    });

    it('should distinguish italic * from bold **', () => {
      const content = '**bold** and *italic*';
      const context = analyzeMarkdownContext(content, 15); // **bold** and *|

      expect(context.italic).toBe(true);
      expect(context.bold).toBe(false);
    });
  });

  describe('analyzeMarkdownContext - Code', () => {
    it('should detect code formatting', () => {
      const content = '`code snippet`';
      const context = analyzeMarkdownContext(content, 5); // `code|

      expect(context.code).toBe(true);
      expect(context.openMarkers).toContain('`');
    });

    it('should close code formatting', () => {
      const content = '`code` text';
      const context = analyzeMarkdownContext(content, 7); // `code` |

      expect(context.code).toBe(false);
    });

    it('should ignore other markdown inside code blocks', () => {
      const content = '`**bold**` text';
      const context = analyzeMarkdownContext(content, 5); // `**bol|

      expect(context.code).toBe(true);
      expect(context.bold).toBe(false); // Bold should not be detected inside code
    });
  });

  describe('analyzeMarkdownContext - Strikethrough', () => {
    it('should detect double tilde strikethrough', () => {
      const content = '~~struck~~';
      const context = analyzeMarkdownContext(content, 5); // ~~stru|

      expect(context.strikethrough).toBe(true);
      expect(context.openMarkers).toContain('~~');
    });

    it('should detect single tilde strikethrough', () => {
      const content = '~struck~';
      const context = analyzeMarkdownContext(content, 3); // ~st|

      expect(context.strikethrough).toBe(true);
      expect(context.openMarkers).toContain('~');
    });

    it('should close strikethrough', () => {
      const content = '~~struck~~';
      const context = analyzeMarkdownContext(content, 10); // ~~struck~~|

      expect(context.strikethrough).toBe(false);
    });
  });

  describe('analyzeMarkdownContext - Mixed Formatting', () => {
    it('should handle bold and italic together', () => {
      const content = '***bold-italic***';
      const context = analyzeMarkdownContext(content, 10); // ***bold-ita|

      expect(context.bold).toBe(true);
      expect(context.italic).toBe(true);
    });

    it('should handle bold and code together', () => {
      const content = '**bold with `code` inside**';
      const context = analyzeMarkdownContext(content, 15); // **bold with `code|

      expect(context.code).toBe(true);
      expect(context.bold).toBe(true);
    });

    it('should handle multiple separate formats', () => {
      const content = '**bold** and *italic* and `code`';
      const context = analyzeMarkdownContext(content, 20); // **bold** and *italic|

      expect(context.italic).toBe(true);
      expect(context.bold).toBe(false);
      expect(context.code).toBe(false);
    });
  });

  describe('getClosingMarkers', () => {
    it('should generate closing markers for active formatting', () => {
      const content = '**bold text**';
      const context = analyzeMarkdownContext(content, 5); // **bol|

      const closing = getClosingMarkers(context);
      expect(closing).toBe('**');
    });

    it('should close markers in LIFO order', () => {
      const content = '***bold-italic***';
      const context = analyzeMarkdownContext(content, 10); // ***bold-ita|

      const closing = getClosingMarkers(context);
      // Both bold and italic are open, should close in reverse order
      expect(closing).toContain('*');
      expect(closing).toContain('**');
    });

    it('should not generate markers when no formatting', () => {
      const context = analyzeMarkdownContext('plain text', 5);
      const closing = getClosingMarkers(context);
      expect(closing).toBe('');
    });

    it('should handle code formatting closing', () => {
      const content = '`code text`';
      const context = analyzeMarkdownContext(content, 5); // `code|

      const closing = getClosingMarkers(context);
      expect(closing).toBe('`');
    });
  });

  describe('getOpeningMarkers', () => {
    it('should generate opening markers for continued formatting', () => {
      const content = '**bold text**';
      const context = analyzeMarkdownContext(content, 5); // **bol|

      const opening = getOpeningMarkers(context);
      expect(opening).toBe('**');
    });

    it('should open markers in original order', () => {
      const content = '***bold-italic***';
      const context = analyzeMarkdownContext(content, 10); // ***bold-ita|

      const opening = getOpeningMarkers(context);
      // Should open in original order: ** then *
      expect(opening).toBe('***');
    });

    it('should not generate markers when no formatting', () => {
      const context = analyzeMarkdownContext('plain text', 5);
      const opening = getOpeningMarkers(context);
      expect(opening).toBe('');
    });
  });

  describe('hasActiveFormatting', () => {
    it('should return true when bold is active', () => {
      const context = analyzeMarkdownContext('**bold**', 4); // **bo|
      expect(hasActiveFormatting(context)).toBe(true);
    });

    it('should return true when italic is active', () => {
      const context = analyzeMarkdownContext('*italic*', 4); // *ita|
      expect(hasActiveFormatting(context)).toBe(true);
    });

    it('should return true when code is active', () => {
      const context = analyzeMarkdownContext('`code`', 3); // `co|
      expect(hasActiveFormatting(context)).toBe(true);
    });

    it('should return false when no formatting', () => {
      const context = analyzeMarkdownContext('plain text', 5);
      expect(hasActiveFormatting(context)).toBe(false);
    });

    it('should return false after all formatting closes', () => {
      const context = analyzeMarkdownContext('**bold** text', 9); // **bold** |
      expect(hasActiveFormatting(context)).toBe(false);
    });
  });

  describe('Edge Cases', () => {
    it('should handle position at start of content', () => {
      const context = analyzeMarkdownContext('**bold**', 0); // |**bold**
      expect(hasActiveFormatting(context)).toBe(false);
    });

    it('should handle position at end of content', () => {
      const context = analyzeMarkdownContext('**bold**', 8); // **bold**|
      expect(hasActiveFormatting(context)).toBe(false);
    });

    it('should handle empty content', () => {
      const context = analyzeMarkdownContext('', 0);
      expect(hasActiveFormatting(context)).toBe(false);
    });

    it('should handle nested formatting', () => {
      const content = '**bold with *italic inside* bold**';
      const context = analyzeMarkdownContext(content, 20); // **bold with *italic|

      expect(context.bold).toBe(true);
      expect(context.italic).toBe(true);
    });

    it('should handle unmatched opening marker', () => {
      const context = analyzeMarkdownContext('**bold without closing', 5);
      expect(context.bold).toBe(true);
    });

    it('should handle escaped characters', () => {
      // Note: The analyzer treats \* as regular characters
      const content = 'Text with \\* escaped';
      const context = analyzeMarkdownContext(content, 10); // Text with |
      expect(context.italic).toBe(false);
    });
  });

  describe('Real-world Scenarios', () => {
    it('should handle documentation comment', () => {
      const content = 'Use `Array.prototype.map()` to **transform** arrays';
      const context = analyzeMarkdownContext(content, 20); // Use `Array.prototype|

      expect(context.code).toBe(true);
      expect(context.bold).toBe(false);
    });

    it('should handle warning with bold', () => {
      const content = '**WARNING:** Do not use in production';
      const context = analyzeMarkdownContext(content, 12); // **WARNING:**| Do

      expect(context.bold).toBe(false);
      expect(hasActiveFormatting(context)).toBe(false);
    });

    it('should handle list item with emphasis', () => {
      const content = '- **Important** action required';
      const context = analyzeMarkdownContext(content, 16); // - **Important**| a

      expect(context.bold).toBe(false);
    });
  });
});
