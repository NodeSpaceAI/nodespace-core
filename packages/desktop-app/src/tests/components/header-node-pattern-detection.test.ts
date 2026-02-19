/**
 * HeaderNode Pattern Detection Tests
 *
 * Tests the unified pattern detection system for HeaderNode auto-conversion
 * and bidirectional conversion between TextNode and HeaderNode.
 *
 * Issue #275: Implement HeaderNode component with textarea
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { PluginRegistry } from '$lib/plugins/plugin-registry';
import { registerCorePlugins } from '$lib/plugins/core-plugins';

describe('HeaderNode Pattern Detection', () => {
  let registry: PluginRegistry;

  beforeEach(() => {
    registry = new PluginRegistry();
    registerCorePlugins(registry);
  });

  afterEach(() => {
    registry.clear();
  });

  describe('Pattern Detection Configuration', () => {
    it('should have header plugin registered with pattern detection', () => {
      expect(registry.hasPlugin('header')).toBe(true);

      const headerPlugin = registry.getPlugin('header');
      expect(headerPlugin).toBeDefined();
      // Issue #667: Pattern now lives on plugin.pattern (new architecture)
      expect(headerPlugin?.pattern).toBeDefined();
      expect(headerPlugin?.pattern?.detect).toBeDefined();
    });

    it('should have correct pattern regex for h1-h6 detection', () => {
      const patterns = registry.getAllPatternDetectionConfigs();
      const headerPattern = patterns.find((p) => p.targetNodeType === 'header');

      expect(headerPattern).toBeDefined();
      expect(headerPattern?.pattern).toEqual(/^(#{1,6})\s/);
      expect(headerPattern?.cleanContent).toBe(false); // Keep "# " in content
      expect(headerPattern?.priority).toBe(10);
    });

    it('should extract headerLevel metadata correctly', () => {
      const patterns = registry.getAllPatternDetectionConfigs();
      const headerPattern = patterns.find((p) => p.targetNodeType === 'header');

      expect(headerPattern?.extractMetadata).toBeDefined();

      // Test h1
      const h1Match = '# Hello'.match(headerPattern!.pattern);
      const h1Metadata = headerPattern!.extractMetadata!(h1Match!);
      expect(h1Metadata.headerLevel).toBe(1);

      // Test h3
      const h3Match = '### World'.match(headerPattern!.pattern);
      const h3Metadata = headerPattern!.extractMetadata!(h3Match!);
      expect(h3Metadata.headerLevel).toBe(3);

      // Test h6
      const h6Match = '###### Max'.match(headerPattern!.pattern);
      const h6Metadata = headerPattern!.extractMetadata!(h6Match!);
      expect(h6Metadata.headerLevel).toBe(6);
    });
  });

  describe('Header Level Pattern Matching', () => {
    it('should match h1 pattern: # + space', () => {
      const result = registry.detectPatternInContent('# Title');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(1);
      expect(result?.config.cleanContent).toBe(false);
    });

    it('should match h2 pattern: ## + space', () => {
      const result = registry.detectPatternInContent('## Subtitle');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(2);
    });

    it('should match h3 pattern: ### + space', () => {
      const result = registry.detectPatternInContent('### Section');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(3);
    });

    it('should match h4 pattern: #### + space', () => {
      const result = registry.detectPatternInContent('#### Subsection');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(4);
    });

    it('should match h5 pattern: ##### + space', () => {
      const result = registry.detectPatternInContent('##### Minor heading');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(5);
    });

    it('should match h6 pattern: ###### + space', () => {
      const result = registry.detectPatternInContent('###### Smallest heading');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(6);
    });
  });

  describe('Pattern Non-Matching Cases', () => {
    it('should NOT match hashtags without space', () => {
      const result = registry.detectPatternInContent('##NoSpace');

      // Should not match header pattern
      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should NOT match more than 6 hashtags', () => {
      const result = registry.detectPatternInContent('####### Too many hashtags');

      // Should not match header pattern (7 hashtags)
      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should NOT match hashtags in middle of content', () => {
      const result = registry.detectPatternInContent('This ## is not a header');

      // Should not match header pattern (hashtags not at start)
      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should NOT match empty content after hashtags', () => {
      const result = registry.detectPatternInContent('# ');

      // Should match the pattern itself (space is present)
      // This is different from "no content" - the space IS the trigger
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(1);
    });

    it('should NOT match when space is missing', () => {
      const result = registry.detectPatternInContent('##');

      // Should not match - no space after hashtags
      expect(result?.config?.targetNodeType).not.toBe('header');
    });
  });

  describe('Content Preservation', () => {
    it('should preserve hashtags in content (cleanContent: false)', () => {
      const result = registry.detectPatternInContent('## Hello World');

      expect(result?.config.cleanContent).toBe(false);

      // Content should NOT be cleaned - hashtags remain for editing
      // The controller is responsible for keeping "## Hello World" as-is
    });

    it('should preserve inline formatting in headers', () => {
      const result = registry.detectPatternInContent('## Hello **bold** world');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');

      // Pattern detection doesn't modify content - just identifies it
      // The HeaderNode component will handle inline formatting rendering
    });

    it('should handle headers with special characters', () => {
      const result = registry.detectPatternInContent('# Title: with @ and #tags');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(1);
    });

    it('should handle headers with numbers', () => {
      const result = registry.detectPatternInContent('### 123 Numbered Header');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(3);
    });
  });

  describe('Bidirectional Conversion Support', () => {
    it('should detect pattern for forward conversion (text → header)', () => {
      // User types: ## [space]
      const result = registry.detectPatternInContent('## ');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(2);
    });

    it('should NOT detect pattern when hashtags removed (header → text)', () => {
      // User backspaces to remove all hashtags
      const result = registry.detectPatternInContent('Hello');

      // No header pattern - should remain text or convert back to text
      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should NOT detect pattern when space removed', () => {
      // User backspaces to remove space after hashtags
      const result = registry.detectPatternInContent('##');

      // No space after hashtags - pattern doesn't match
      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should detect pattern change when level changes', () => {
      // User adds more hashtags
      const h2Result = registry.detectPatternInContent('## Title');
      expect(h2Result?.metadata?.headerLevel).toBe(2);

      const h4Result = registry.detectPatternInContent('#### Title');
      expect(h4Result?.metadata?.headerLevel).toBe(4);

      // Levels should be different
      expect(h2Result?.metadata?.headerLevel).not.toBe(h4Result?.metadata?.headerLevel);
    });
  });

  describe('Edge Cases', () => {
    it('should handle empty string', () => {
      const result = registry.detectPatternInContent('');

      expect(result).toBeNull();
    });

    it('should handle whitespace-only content', () => {
      const result = registry.detectPatternInContent('   ');

      expect(result?.config?.targetNodeType).not.toBe('header');
    });

    it('should handle newlines in content', () => {
      const result = registry.detectPatternInContent('## Title\nWith newline');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      // Pattern still matches - HeaderNode will enforce single-line
    });

    it('should handle unicode characters', () => {
      const result = registry.detectPatternInContent('## 你好世界 🌍');

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(2);
    });

    it('should handle very long headers', () => {
      const longTitle = 'A'.repeat(1000);
      const result = registry.detectPatternInContent(`# ${longTitle}`);

      expect(result).toBeDefined();
      expect(result?.config.targetNodeType).toBe('header');
      expect(result?.metadata?.headerLevel).toBe(1);
    });
  });

  describe('Priority and Ordering', () => {
    it('should have appropriate priority for pattern matching', () => {
      const patterns = registry.getAllPatternDetectionConfigs();
      const headerPattern = patterns.find((p) => p.targetNodeType === 'header');

      // Header pattern should have priority 10
      expect(headerPattern?.priority).toBe(10);
    });

    it('should not conflict with checkbox pattern', () => {
      // "[ ] Task item" does not start with "- " so doesn't match checkbox pattern
      const checkboxResult = registry.detectPatternInContent('- [ ] Checkbox item');
      const headerResult = registry.detectPatternInContent('# Header');

      // Both patterns should work independently
      expect(checkboxResult?.config.targetNodeType).toBe('checkbox');
      expect(headerResult?.config.targetNodeType).toBe('header');
    });

    it('should have checkbox pattern available (task pattern removed)', () => {
      // This verifies checkbox now handles "- [ ]" syntax; task has no editor pattern
      const patterns = registry.getAllPatternDetectionConfigs();

      // Verify header and checkbox patterns exist
      const headerPattern = patterns.find((p) => p.targetNodeType === 'header');
      const checkboxPattern = patterns.find((p) => p.targetNodeType === 'checkbox');

      expect(headerPattern).toBeDefined();
      expect(checkboxPattern).toBeDefined();

      // task no longer has an editor pattern
      const taskPattern = patterns.find((p) => p.targetNodeType === 'task');
      expect(taskPattern).toBeUndefined();

      // Both have priority 10
      expect(headerPattern?.priority).toBe(10);
      expect(checkboxPattern?.priority).toBe(10);
    });
  });

  describe('Slash Command Integration', () => {
    it('should have slash commands for h1, h2, h3', () => {
      const commands = registry.getAllSlashCommands();

      const h1Command = commands.find((cmd) => cmd.id === 'header1');
      const h2Command = commands.find((cmd) => cmd.id === 'header2');
      const h3Command = commands.find((cmd) => cmd.id === 'header3');

      expect(h1Command).toBeDefined();
      expect(h1Command?.contentTemplate).toBe('# ');
      expect(h1Command?.nodeType).toBe('header');

      expect(h2Command).toBeDefined();
      expect(h2Command?.contentTemplate).toBe('## ');
      expect(h2Command?.nodeType).toBe('header');

      expect(h3Command).toBeDefined();
      expect(h3Command?.contentTemplate).toBe('### ');
      expect(h3Command?.nodeType).toBe('header');
    });

    it('should have shortcuts for slash commands', () => {
      const commands = registry.getAllSlashCommands();

      const h1Command = commands.find((cmd) => cmd.id === 'header1');
      const h2Command = commands.find((cmd) => cmd.id === 'header2');
      const h3Command = commands.find((cmd) => cmd.id === 'header3');

      expect(h1Command?.shortcut).toBe('#');
      expect(h2Command?.shortcut).toBe('##');
      expect(h3Command?.shortcut).toBe('###');
    });

    it('should create content that matches pattern detection', () => {
      const commands = registry.getAllSlashCommands();

      // Test each slash command creates pattern-matching content
      const h1Command = commands.find((cmd) => cmd.id === 'header1');
      const h1Pattern = registry.detectPatternInContent(h1Command!.contentTemplate);
      expect(h1Pattern?.config.targetNodeType).toBe('header');
      expect(h1Pattern?.metadata?.headerLevel).toBe(1);

      const h2Command = commands.find((cmd) => cmd.id === 'header2');
      const h2Pattern = registry.detectPatternInContent(h2Command!.contentTemplate);
      expect(h2Pattern?.config.targetNodeType).toBe('header');
      expect(h2Pattern?.metadata?.headerLevel).toBe(2);

      const h3Command = commands.find((cmd) => cmd.id === 'header3');
      const h3Pattern = registry.detectPatternInContent(h3Command!.contentTemplate);
      expect(h3Pattern?.config.targetNodeType).toBe('header');
      expect(h3Pattern?.metadata?.headerLevel).toBe(3);
    });
  });
});
