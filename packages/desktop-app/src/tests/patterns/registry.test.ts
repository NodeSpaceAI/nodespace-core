/**
 * Pattern Registry Tests
 *
 * Tests the pattern registry functionality:
 * - Pattern registration and retrieval
 * - Pattern detection with priority ordering
 * - Edge cases (no match, multiple matches, pattern validation)
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { PatternRegistry } from '../../lib/patterns/registry';
import type { PatternTemplate } from '../../lib/patterns/types';

describe('PatternRegistry', () => {
  let registry: PatternRegistry;

  beforeEach(() => {
    registry = PatternRegistry.getInstance();
    registry.clear(); // Clear between tests
  });

  afterEach(() => {
    registry.clear();
  });

  describe('Pattern Registration', () => {
    it('should register a pattern with unique node type', () => {
      const pattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern);

      const stats = registry.getStats();
      expect(stats.patternCount).toBe(1);
      expect(stats.registeredNodeTypes).toContain('header');
    });

    it('should update pattern when registering duplicate node type', () => {
      const pattern1: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const pattern2: PatternTemplate = {
        regex: /^#{1,6}\s/,
        nodeType: 'header',
        priority: 20,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern1);
      registry.register(pattern2);

      const stats = registry.getStats();
      expect(stats.patternCount).toBe(1); // Should still be 1, not 2
      expect(registry.getPattern('header')?.priority).toBe(20); // Should use latest
    });

    it('should throw error when registering pattern without nodeType', () => {
      const invalidPattern = {
        regex: /^# /,
        priority: 10,
        splittingStrategy: 'prefix-inheritance' as const,
        cursorPlacement: 'after-prefix' as const
      } as unknown as PatternTemplate;

      expect(() => registry.register(invalidPattern)).toThrow();
    });

    it('should throw error when registering pattern without regex', () => {
      const invalidPattern = {
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance' as const,
        cursorPlacement: 'after-prefix' as const
      } as unknown as PatternTemplate;

      expect(() => registry.register(invalidPattern)).toThrow();
    });
  });

  describe('Pattern Retrieval', () => {
    it('should retrieve pattern by node type', () => {
      const pattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern);
      const retrieved = registry.getPattern('header');

      expect(retrieved).toBeDefined();
      expect(retrieved?.nodeType).toBe('header');
      expect(retrieved?.priority).toBe(10);
    });

    it('should return undefined for unregistered node type', () => {
      const pattern = registry.getPattern('nonexistent');
      expect(pattern).toBeUndefined();
    });

    it('should retrieve all patterns', () => {
      const headerPattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const textPattern: PatternTemplate = {
        regex: /^/,
        nodeType: 'text',
        priority: 1,
        splittingStrategy: 'simple-split',
        cursorPlacement: 'start'
      };

      registry.register(headerPattern);
      registry.register(textPattern);

      const allPatterns = registry.getAllPatterns();
      expect(allPatterns).toHaveLength(2);
      expect(allPatterns.map((p) => p.nodeType)).toContain('header');
      expect(allPatterns.map((p) => p.nodeType)).toContain('text');
    });
  });

  describe('Pattern Detection', () => {
    beforeEach(() => {
      const headerPattern: PatternTemplate = {
        regex: /^(#{1,6})\s/,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const listPattern: PatternTemplate = {
        regex: /^1\.\s/,
        nodeType: 'ordered-list',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '1. ',
        cursorPlacement: 'after-prefix'
      };

      const quotePattern: PatternTemplate = {
        regex: /^>\s/,
        nodeType: 'quote-block',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '> ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(headerPattern);
      registry.register(listPattern);
      registry.register(quotePattern);
    });

    it('should detect header pattern in content', () => {
      const result = registry.detectPattern('# My Header');

      expect(result.found).toBe(true);
      expect(result.pattern?.nodeType).toBe('header');
      expect(result.match).toBeDefined();
      expect(result.match?.[0]).toBe('# ');
    });

    it('should detect list pattern in content', () => {
      const result = registry.detectPattern('1. First item');

      expect(result.found).toBe(true);
      expect(result.pattern?.nodeType).toBe('ordered-list');
      expect(result.match?.[0]).toBe('1. ');
    });

    it('should detect quote pattern in content', () => {
      const result = registry.detectPattern('> A quote');

      expect(result.found).toBe(true);
      expect(result.pattern?.nodeType).toBe('quote-block');
      expect(result.match?.[0]).toBe('> ');
    });

    it('should return not found for non-matching content', () => {
      const result = registry.detectPattern('Just plain text');

      expect(result.found).toBe(false);
      expect(result.pattern).toBeNull();
      expect(result.match).toBeNull();
    });

    it('should detect pattern for specific node type', () => {
      const result = registry.detectPatternForNodeType('# My Header', 'header');

      expect(result.found).toBe(true);
      expect(result.pattern?.nodeType).toBe('header');
    });

    it('should not detect pattern for wrong node type', () => {
      const result = registry.detectPatternForNodeType('# My Header', 'text');

      expect(result.found).toBe(false);
    });
  });

  describe('Pattern Priority', () => {
    it('should respect pattern priority when multiple patterns could match', () => {
      // Register patterns with different priorities
      const lowPriorityPattern: PatternTemplate = {
        regex: /^#/,
        nodeType: 'text',
        priority: 1,
        splittingStrategy: 'simple-split',
        cursorPlacement: 'start'
      };

      const highPriorityPattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 100,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(lowPriorityPattern);
      registry.register(highPriorityPattern);

      // Both patterns match "# Header", but header should be returned
      const result = registry.detectPattern('# Header');

      expect(result.pattern?.nodeType).toBe('header');
      expect(result.pattern?.priority).toBe(100);
    });

    it('should check patterns in descending priority order', () => {
      const pattern1: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 50,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const pattern2: PatternTemplate = {
        regex: /^# /,
        nodeType: 'emphasis',
        priority: 25,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const pattern3: PatternTemplate = {
        regex: /^# /,
        nodeType: 'strong',
        priority: 75,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern1);
      registry.register(pattern2);
      registry.register(pattern3);

      const result = registry.detectPattern('# Content');

      // Should return the highest priority pattern (strong with priority 75)
      expect(result.pattern?.nodeType).toBe('strong');
    });
  });

  describe('Start-of-Content Detection', () => {
    beforeEach(() => {
      const headerPattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(headerPattern);
    });

    it('should detect pattern at start of content', () => {
      const result = registry.startsWithPattern('# Header');

      expect(result.found).toBe(true);
      expect(result.pattern?.nodeType).toBe('header');
    });

    it('should not detect pattern not at start', () => {
      const result = registry.startsWithPattern('Text # not at start');

      expect(result.found).toBe(false);
    });
  });

  describe('Registry Statistics', () => {
    it('should return accurate statistics', () => {
      const pattern1: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const pattern2: PatternTemplate = {
        regex: /^1\.\s/,
        nodeType: 'ordered-list',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '1. ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern1);
      registry.register(pattern2);

      const stats = registry.getStats();

      expect(stats.patternCount).toBe(2);
      expect(stats.registeredNodeTypes).toContain('header');
      expect(stats.registeredNodeTypes).toContain('ordered-list');
    });
  });

  describe('Check Pattern Existence', () => {
    it('should check if pattern is registered for node type', () => {
      const pattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(pattern);

      expect(registry.hasPattern('header')).toBe(true);
      expect(registry.hasPattern('nonexistent')).toBe(false);
    });
  });

  describe('Regex Handling', () => {
    it('should handle patterns with capture groups', () => {
      const pattern: PatternTemplate = {
        regex: /^(#{1,6})\s+(.+)$/,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix',
        extractMetadata: (match) => ({
          headerLevel: match[1].length,
          title: match[2]
        })
      };

      registry.register(pattern);

      const result = registry.detectPattern('## My Title');

      expect(result.found).toBe(true);
      expect(result.match?.[1]).toBe('##');
      expect(result.match?.[2]).toBe('My Title');
    });

    it('should handle patterns with global flag', () => {
      const pattern: PatternTemplate = {
        regex: /\*\*(.+?)\*\*/g,
        nodeType: 'bold',
        priority: 10,
        splittingStrategy: 'simple-split',
        cursorPlacement: 'start'
      };

      registry.register(pattern);

      // First detection
      let result = registry.detectPattern('Text with **bold** content');
      expect(result.found).toBe(true);
      expect(result.match?.[0]).toBe('**bold**');

      // Should be able to detect again without issues
      result = registry.detectPattern('**bold** at start');
      expect(result.found).toBe(true);
    });
  });
});
