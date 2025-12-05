/**
 * Tests for Markdown Pattern Type Definitions
 *
 * Comprehensive test coverage for markdown pattern types and WYSIWYG transformations.
 */

import { describe, it, expect } from 'vitest';
import {
  type MarkdownPatternType,
  type MarkdownPattern,
  type PatternDetectionResult,
  type WYSIWYGTransformation,
  WYSIWYG_TRANSFORMATIONS
} from '$lib/types/markdown-patterns';

describe('MarkdownPatternType', () => {
  it('includes all block types', () => {
    const blockTypes: MarkdownPatternType[] = ['header', 'bullet', 'blockquote', 'codeblock'];
    blockTypes.forEach((type) => {
      expect(['header', 'bullet', 'blockquote', 'codeblock', 'bold', 'italic', 'inlinecode']).toContain(type);
    });
  });

  it('includes all inline types', () => {
    const inlineTypes: MarkdownPatternType[] = ['bold', 'italic', 'inlinecode'];
    inlineTypes.forEach((type) => {
      expect(['header', 'bullet', 'blockquote', 'codeblock', 'bold', 'italic', 'inlinecode']).toContain(type);
    });
  });
});

describe('MarkdownPattern interface', () => {
  it('accepts valid markdown pattern structure', () => {
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 10,
      syntax: '**',
      content: 'bold text',
      raw: '**bold text**'
    };

    expect(pattern.type).toBe('bold');
    expect(pattern.start).toBe(0);
    expect(pattern.end).toBe(10);
    expect(pattern.syntax).toBe('**');
    expect(pattern.content).toBe('bold text');
    expect(pattern.raw).toBe('**bold text**');
  });

  it('accepts header pattern with level', () => {
    const pattern: MarkdownPattern = {
      type: 'header',
      level: 2,
      start: 0,
      end: 15,
      syntax: '##',
      content: 'Header Text',
      raw: '## Header Text'
    };

    expect(pattern.type).toBe('header');
    expect(pattern.level).toBe(2);
    expect(pattern.content).toBe('Header Text');
  });

  it('accepts pattern without optional level', () => {
    const pattern: MarkdownPattern = {
      type: 'italic',
      start: 5,
      end: 15,
      syntax: '*',
      content: 'italic text',
      raw: '*italic text*'
    };

    expect(pattern.level).toBeUndefined();
    expect(pattern.type).toBe('italic');
  });

  it('handles inline code pattern', () => {
    const pattern: MarkdownPattern = {
      type: 'inlinecode',
      start: 10,
      end: 25,
      syntax: '`',
      content: 'const x = 5;',
      raw: '`const x = 5;`'
    };

    expect(pattern.type).toBe('inlinecode');
    expect(pattern.syntax).toBe('`');
    expect(pattern.content).toBe('const x = 5;');
  });

  it('handles blockquote pattern', () => {
    const pattern: MarkdownPattern = {
      type: 'blockquote',
      start: 0,
      end: 20,
      syntax: '>',
      content: 'quoted text here',
      raw: '> quoted text here'
    };

    expect(pattern.type).toBe('blockquote');
    expect(pattern.syntax).toBe('>');
  });

  it('handles bullet pattern', () => {
    const pattern: MarkdownPattern = {
      type: 'bullet',
      start: 0,
      end: 15,
      syntax: '-',
      content: 'list item',
      raw: '- list item'
    };

    expect(pattern.type).toBe('bullet');
    expect(pattern.syntax).toBe('-');
  });

  it('handles codeblock pattern', () => {
    const pattern: MarkdownPattern = {
      type: 'codeblock',
      start: 0,
      end: 30,
      syntax: '```',
      content: 'function test() {}',
      raw: '```\nfunction test() {}\n```'
    };

    expect(pattern.type).toBe('codeblock');
    expect(pattern.syntax).toBe('```');
  });

  it('handles zero position patterns', () => {
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 0,
      syntax: '**',
      content: '',
      raw: '****'
    };

    expect(pattern.start).toBe(0);
    expect(pattern.end).toBe(0);
  });

  it('handles large position values', () => {
    const pattern: MarkdownPattern = {
      type: 'italic',
      start: 1000,
      end: 1050,
      syntax: '*',
      content: 'text at large position',
      raw: '*text at large position*'
    };

    expect(pattern.start).toBe(1000);
    expect(pattern.end).toBe(1050);
  });
});

describe('PatternDetectionResult interface', () => {
  it('accepts empty pattern result', () => {
    const result: PatternDetectionResult = {
      patterns: [],
      processedContent: 'plain text',
      hasPatterns: false
    };

    expect(result.patterns).toEqual([]);
    expect(result.processedContent).toBe('plain text');
    expect(result.hasPatterns).toBe(false);
  });

  it('accepts result with patterns', () => {
    const result: PatternDetectionResult = {
      patterns: [
        {
          type: 'bold',
          start: 0,
          end: 10,
          syntax: '**',
          content: 'bold',
          raw: '**bold**'
        }
      ],
      processedContent: '<strong>bold</strong>',
      hasPatterns: true
    };

    expect(result.patterns).toHaveLength(1);
    expect(result.hasPatterns).toBe(true);
    expect(result.processedContent).toBe('<strong>bold</strong>');
  });

  it('accepts result with multiple patterns', () => {
    const result: PatternDetectionResult = {
      patterns: [
        {
          type: 'bold',
          start: 0,
          end: 10,
          syntax: '**',
          content: 'bold',
          raw: '**bold**'
        },
        {
          type: 'italic',
          start: 11,
          end: 21,
          syntax: '*',
          content: 'italic',
          raw: '*italic*'
        }
      ],
      processedContent: '<strong>bold</strong> <em>italic</em>',
      hasPatterns: true
    };

    expect(result.patterns).toHaveLength(2);
    expect(result.patterns[0].type).toBe('bold');
    expect(result.patterns[1].type).toBe('italic');
  });

  it('handles empty processed content', () => {
    const result: PatternDetectionResult = {
      patterns: [],
      processedContent: '',
      hasPatterns: false
    };

    expect(result.processedContent).toBe('');
  });
});

describe('WYSIWYGTransformation interface', () => {
  it('accepts transformation without htmlTag', () => {
    const transformation: WYSIWYGTransformation = {
      type: 'bullet',
      cssClass: 'markdown-bullet',
      hideMarkup: true
    };

    expect(transformation.type).toBe('bullet');
    expect(transformation.cssClass).toBe('markdown-bullet');
    expect(transformation.hideMarkup).toBe(true);
    expect(transformation.htmlTag).toBeUndefined();
  });

  it('accepts transformation with htmlTag', () => {
    const transformation: WYSIWYGTransformation = {
      type: 'bold',
      cssClass: 'markdown-bold',
      hideMarkup: true,
      htmlTag: 'strong'
    };

    expect(transformation.htmlTag).toBe('strong');
  });

  it('accepts transformation with level', () => {
    const transformation: WYSIWYGTransformation = {
      type: 'header',
      level: 1,
      cssClass: 'markdown-header-1',
      hideMarkup: true,
      htmlTag: 'h1'
    };

    expect(transformation.level).toBe(1);
    expect(transformation.htmlTag).toBe('h1');
  });

  it('accepts transformation with hideMarkup false', () => {
    const transformation: WYSIWYGTransformation = {
      type: 'italic',
      cssClass: 'markdown-italic',
      hideMarkup: false
    };

    expect(transformation.hideMarkup).toBe(false);
  });
});

describe('WYSIWYG_TRANSFORMATIONS constant', () => {
  it('is defined and exported', () => {
    expect(WYSIWYG_TRANSFORMATIONS).toBeDefined();
    expect(typeof WYSIWYG_TRANSFORMATIONS).toBe('object');
  });

  it('contains header-1 transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS['header-1']).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS['header-1'].type).toBe('header');
    expect(WYSIWYG_TRANSFORMATIONS['header-1'].level).toBe(1);
    expect(WYSIWYG_TRANSFORMATIONS['header-1'].cssClass).toBe('markdown-header-1');
    expect(WYSIWYG_TRANSFORMATIONS['header-1'].hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS['header-1'].htmlTag).toBe('h1');
  });

  it('contains header-2 transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS['header-2']).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS['header-2'].type).toBe('header');
    expect(WYSIWYG_TRANSFORMATIONS['header-2'].level).toBe(2);
    expect(WYSIWYG_TRANSFORMATIONS['header-2'].cssClass).toBe('markdown-header-2');
    expect(WYSIWYG_TRANSFORMATIONS['header-2'].hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS['header-2'].htmlTag).toBe('h2');
  });

  it('contains header-3 transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS['header-3']).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS['header-3'].type).toBe('header');
    expect(WYSIWYG_TRANSFORMATIONS['header-3'].level).toBe(3);
    expect(WYSIWYG_TRANSFORMATIONS['header-3'].cssClass).toBe('markdown-header-3');
    expect(WYSIWYG_TRANSFORMATIONS['header-3'].hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS['header-3'].htmlTag).toBe('h3');
  });

  it('contains bold transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.bold).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.bold.type).toBe('bold');
    expect(WYSIWYG_TRANSFORMATIONS.bold.cssClass).toBe('markdown-bold');
    expect(WYSIWYG_TRANSFORMATIONS.bold.hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS.bold.htmlTag).toBe('strong');
  });

  it('contains italic transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.italic).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.italic.type).toBe('italic');
    expect(WYSIWYG_TRANSFORMATIONS.italic.cssClass).toBe('markdown-italic');
    expect(WYSIWYG_TRANSFORMATIONS.italic.hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS.italic.htmlTag).toBe('em');
  });

  it('contains inlinecode transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode.type).toBe('inlinecode');
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode.cssClass).toBe('markdown-code');
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode.hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode.htmlTag).toBe('code');
  });

  it('contains blockquote transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.blockquote).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.blockquote.type).toBe('blockquote');
    expect(WYSIWYG_TRANSFORMATIONS.blockquote.cssClass).toBe('markdown-blockquote');
    expect(WYSIWYG_TRANSFORMATIONS.blockquote.hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS.blockquote.htmlTag).toBe('blockquote');
  });

  it('contains bullet transformation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.bullet).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.bullet.type).toBe('bullet');
    expect(WYSIWYG_TRANSFORMATIONS.bullet.cssClass).toBe('markdown-bullet');
    expect(WYSIWYG_TRANSFORMATIONS.bullet.hideMarkup).toBe(true);
    expect(WYSIWYG_TRANSFORMATIONS.bullet.htmlTag).toBeUndefined();
  });

  it('all transformations have hideMarkup set to true', () => {
    Object.values(WYSIWYG_TRANSFORMATIONS).forEach((transformation) => {
      expect(transformation.hideMarkup).toBe(true);
    });
  });

  it('all transformations have non-empty cssClass', () => {
    Object.values(WYSIWYG_TRANSFORMATIONS).forEach((transformation) => {
      expect(transformation.cssClass).toBeTruthy();
      expect(transformation.cssClass.length).toBeGreaterThan(0);
    });
  });

  it('all cssClasses start with "markdown-"', () => {
    Object.values(WYSIWYG_TRANSFORMATIONS).forEach((transformation) => {
      expect(transformation.cssClass).toMatch(/^markdown-/);
    });
  });

  it('header transformations have consistent structure', () => {
    const headers = ['header-1', 'header-2', 'header-3'];
    headers.forEach((key, index) => {
      const level = index + 1;
      const transformation = WYSIWYG_TRANSFORMATIONS[key];

      expect(transformation.type).toBe('header');
      expect(transformation.level).toBe(level);
      expect(transformation.cssClass).toBe(`markdown-header-${level}`);
      expect(transformation.htmlTag).toBe(`h${level}`);
      expect(transformation.hideMarkup).toBe(true);
    });
  });

  it('inline transformations have appropriate HTML tags', () => {
    expect(WYSIWYG_TRANSFORMATIONS.bold.htmlTag).toBe('strong');
    expect(WYSIWYG_TRANSFORMATIONS.italic.htmlTag).toBe('em');
    expect(WYSIWYG_TRANSFORMATIONS.inlinecode.htmlTag).toBe('code');
  });

  it('block transformations have appropriate HTML tags', () => {
    expect(WYSIWYG_TRANSFORMATIONS.blockquote.htmlTag).toBe('blockquote');
    expect(WYSIWYG_TRANSFORMATIONS.bullet.htmlTag).toBeUndefined();
  });

  it('has exactly 8 transformation entries', () => {
    const keys = Object.keys(WYSIWYG_TRANSFORMATIONS);
    expect(keys).toHaveLength(8);
  });

  it('all transformation keys are strings', () => {
    Object.keys(WYSIWYG_TRANSFORMATIONS).forEach((key) => {
      expect(typeof key).toBe('string');
    });
  });

  it('can be accessed with bracket notation', () => {
    expect(WYSIWYG_TRANSFORMATIONS['bold']).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS['italic']).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS['header-1']).toBeDefined();
  });

  it('can be accessed with dot notation', () => {
    expect(WYSIWYG_TRANSFORMATIONS.bold).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.italic).toBeDefined();
    expect(WYSIWYG_TRANSFORMATIONS.bullet).toBeDefined();
  });
});

describe('WYSIWYG_TRANSFORMATIONS type validation', () => {
  it('each value is a valid WYSIWYGTransformation', () => {
    Object.values(WYSIWYG_TRANSFORMATIONS).forEach((transformation) => {
      expect(transformation).toHaveProperty('type');
      expect(transformation).toHaveProperty('cssClass');
      expect(transformation).toHaveProperty('hideMarkup');

      // Type should be a valid MarkdownPatternType
      expect(['header', 'bullet', 'blockquote', 'codeblock', 'bold', 'italic', 'inlinecode']).toContain(transformation.type);

      // cssClass should be a string
      expect(typeof transformation.cssClass).toBe('string');

      // hideMarkup should be a boolean
      expect(typeof transformation.hideMarkup).toBe('boolean');

      // If level exists, it should be a number
      if (transformation.level !== undefined) {
        expect(typeof transformation.level).toBe('number');
        expect(transformation.level).toBeGreaterThan(0);
        expect(transformation.level).toBeLessThanOrEqual(6);
      }

      // If htmlTag exists, it should be a string
      if (transformation.htmlTag !== undefined) {
        expect(typeof transformation.htmlTag).toBe('string');
        expect(transformation.htmlTag.length).toBeGreaterThan(0);
      }
    });
  });
});

describe('Integration: Pattern with Transformation', () => {
  it('matches pattern type with transformation type', () => {
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 10,
      syntax: '**',
      content: 'bold text',
      raw: '**bold text**'
    };

    const transformation = WYSIWYG_TRANSFORMATIONS[pattern.type];
    expect(transformation).toBeDefined();
    expect(transformation.type).toBe(pattern.type);
  });

  it('supports header pattern with level matching transformation', () => {
    const pattern: MarkdownPattern = {
      type: 'header',
      level: 2,
      start: 0,
      end: 15,
      syntax: '##',
      content: 'Header',
      raw: '## Header'
    };

    const transformationKey = `header-${pattern.level}`;
    const transformation = WYSIWYG_TRANSFORMATIONS[transformationKey];

    expect(transformation).toBeDefined();
    expect(transformation.type).toBe('header');
    expect(transformation.level).toBe(pattern.level);
  });

  it('creates detection result with matching transformations', () => {
    const result: PatternDetectionResult = {
      patterns: [
        {
          type: 'bold',
          start: 0,
          end: 10,
          syntax: '**',
          content: 'bold',
          raw: '**bold**'
        },
        {
          type: 'italic',
          start: 11,
          end: 21,
          syntax: '*',
          content: 'italic',
          raw: '*italic*'
        }
      ],
      processedContent: '<strong>bold</strong> <em>italic</em>',
      hasPatterns: true
    };

    result.patterns.forEach((pattern) => {
      const transformation = WYSIWYG_TRANSFORMATIONS[pattern.type];
      expect(transformation).toBeDefined();
      expect(transformation.type).toBe(pattern.type);
    });
  });

  it('applies correct CSS classes from transformations', () => {
    const patterns: MarkdownPattern[] = [
      {
        type: 'bold',
        start: 0,
        end: 10,
        syntax: '**',
        content: 'bold',
        raw: '**bold**'
      },
      {
        type: 'italic',
        start: 11,
        end: 21,
        syntax: '*',
        content: 'italic',
        raw: '*italic*'
      }
    ];

    const cssClasses = patterns.map((p) => WYSIWYG_TRANSFORMATIONS[p.type].cssClass);

    expect(cssClasses).toContain('markdown-bold');
    expect(cssClasses).toContain('markdown-italic');
  });

  it('applies correct HTML tags from transformations', () => {
    const patterns: MarkdownPattern[] = [
      {
        type: 'bold',
        start: 0,
        end: 10,
        syntax: '**',
        content: 'bold',
        raw: '**bold**'
      },
      {
        type: 'italic',
        start: 11,
        end: 21,
        syntax: '*',
        content: 'italic',
        raw: '*italic*'
      }
    ];

    const htmlTags = patterns.map((p) => WYSIWYG_TRANSFORMATIONS[p.type].htmlTag);

    expect(htmlTags).toContain('strong');
    expect(htmlTags).toContain('em');
  });
});

describe('Edge cases and boundary conditions', () => {
  it('handles patterns with empty content', () => {
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 4,
      syntax: '**',
      content: '',
      raw: '****'
    };

    expect(pattern.content).toBe('');
    expect(pattern.raw).toBe('****');
  });

  it('handles patterns with very long content', () => {
    const longContent = 'a'.repeat(10000);
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 10004,
      syntax: '**',
      content: longContent,
      raw: `**${longContent}**`
    };

    expect(pattern.content).toHaveLength(10000);
    expect(pattern.raw).toHaveLength(10004);
  });

  it('handles patterns with special characters', () => {
    const specialContent = '!@#$%^&*()_+-=[]{}|;:,.<>?';
    const pattern: MarkdownPattern = {
      type: 'inlinecode',
      start: 0,
      end: specialContent.length + 2,
      syntax: '`',
      content: specialContent,
      raw: `\`${specialContent}\``
    };

    expect(pattern.content).toBe(specialContent);
  });

  it('handles patterns with unicode characters', () => {
    const unicodeContent = '你好世界 🌍🚀✨';
    const pattern: MarkdownPattern = {
      type: 'bold',
      start: 0,
      end: 20,
      syntax: '**',
      content: unicodeContent,
      raw: `**${unicodeContent}**`
    };

    expect(pattern.content).toBe(unicodeContent);
  });

  it('handles patterns with newlines in content', () => {
    const multilineContent = 'Line 1\nLine 2\nLine 3';
    const pattern: MarkdownPattern = {
      type: 'codeblock',
      start: 0,
      end: 30,
      syntax: '```',
      content: multilineContent,
      raw: `\`\`\`\n${multilineContent}\n\`\`\``
    };

    expect(pattern.content).toContain('\n');
    expect(pattern.content.split('\n')).toHaveLength(3);
  });

  it('handles detection result with no patterns but content', () => {
    const result: PatternDetectionResult = {
      patterns: [],
      processedContent: 'This is plain text without any markdown',
      hasPatterns: false
    };

    expect(result.patterns).toHaveLength(0);
    expect(result.hasPatterns).toBe(false);
    expect(result.processedContent).toBeTruthy();
  });

  it('handles all valid header levels', () => {
    for (let level = 1; level <= 3; level++) {
      const key = `header-${level}`;
      const transformation = WYSIWYG_TRANSFORMATIONS[key];

      if (transformation) {
        expect(transformation.level).toBe(level);
        expect(transformation.htmlTag).toBe(`h${level}`);
      }
    }
  });
});
