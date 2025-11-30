/**
 * OrderedListNode Tests
 *
 * Tests for ordered list plugin functionality including:
 * - Pattern detection and content preservation
 * - Plugin registration and configuration
 * - Template system (slash commands and pattern detection consistency)
 * - CSS counter auto-numbering
 * - Plugin feature flags
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('OrderedListNode Plugin', () => {
  describe('Plugin Registration', () => {
    it('should have plugin registered', () => {
      expect(pluginRegistry.hasPlugin('ordered-list')).toBe(true);
    });

    it('should have correct plugin metadata', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin).toBeDefined();
      expect(plugin?.name).toBe('Ordered List Node');
      expect(plugin?.description).toBe('Auto-numbered ordered list items');
      expect(plugin?.version).toBe('1.0.0');
    });
  });

  describe('Plugin Configuration', () => {
    it('should have correct canHaveChildren setting (leaf nodes only)', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should allow being a child node', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin?.config.canBeChild).toBe(true);
    });
  });

  describe('Slash Command', () => {
    it('should register slash command', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.nodeType === 'ordered-list');

      expect(command).toBeDefined();
      expect(command?.id).toBe('ordered-list');
      expect(command?.name).toBe('Ordered List');
    });

    it('should have correct slash command configuration', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      expect(command?.shortcut).toBe('1.');
      expect(command?.contentTemplate).toBe('1. ');
      expect(command?.desiredCursorPosition).toBe(3);
    });
  });

  describe('Pattern Detection', () => {
    it('should detect "1. " pattern', () => {
      const content = '1. Test item';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('ordered-list');
    });

    it('should NOT detect pattern without space after period', () => {
      // Pattern is /^1\.\s/, so "1." without space should not match
      const content = '1.Test item';
      const detection = pluginRegistry.detectPatternInContent(content);

      // This may match a different pattern or null - ordered-list pattern should not match
      if (detection?.config.targetNodeType === 'ordered-list') {
        expect.fail('Should not detect "1." without space');
      }
    });

    it('should preserve content with cleanContent: false (canRevert: true)', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // Issue #667: Pattern now lives on plugin.pattern (new architecture)
      // canRevert: true means cleanContent: false (content preserved for reversion)
      expect(plugin?.pattern?.canRevert).toBe(true);
    });

    it('should NOT use contentTemplate in pattern detection', () => {
      // Pattern detection preserves user's content - no template replacement
      // contentTemplate is only used by slash commands for initial content
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // Verify plugin.pattern exists (new architecture) and has detect regex
      expect(plugin?.pattern?.detect).toBeDefined();
      // canRevert: true means content is preserved (no template replacement)
      expect(plugin?.pattern?.canRevert).toBe(true);
    });

    it('should have correct cursor positioning in pattern detection', () => {
      // Issue #667: Cursor positioning comes from slash command config
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const pattern = patterns.find((p) => p.targetNodeType === 'ordered-list');

      // Cursor should be at position 3 (after "1. ")
      expect(pattern?.desiredCursorPosition).toBe(3);
    });

    it('should have correct pattern priority', () => {
      // Issue #667: Priority is derived from plugin.pattern
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const pattern = patterns.find((p) => p.targetNodeType === 'ordered-list');

      expect(pattern?.priority).toBe(10);
    });
  });

  describe('Template System Consistency', () => {
    it('should produce identical results for slash command and pattern detection', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const slashCommand = commands.find((c) => c.id === 'ordered-list');

      // Issue #667: Pattern config derived from plugin.pattern via getAllPatternDetectionConfigs
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const patternDetection = patterns.find((p) => p.targetNodeType === 'ordered-list');

      // Both should position cursor at same location
      expect(slashCommand?.desiredCursorPosition).toBe(patternDetection?.desiredCursorPosition);
      expect(slashCommand?.desiredCursorPosition).toBe(3);

      // Slash command provides template, pattern detection preserves user content
      expect(slashCommand?.contentTemplate).toBe('1. ');
      expect(patternDetection?.cleanContent).toBe(false);
    });

    it('should have pattern detection extract metadata', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // Issue #667: extractMetadata now lives on plugin.pattern
      expect(plugin?.pattern?.extractMetadata).toBeDefined();

      // For ordered lists, metadata should be empty (no complex parsing needed)
      const metadata = plugin?.pattern?.extractMetadata?.([] as unknown as RegExpMatchArray);
      expect(metadata).toEqual({});
    });
  });

  describe('Plugin Component References', () => {
    it('should have node component defined', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin?.node).toBeDefined();
      expect(plugin?.node?.lazyLoad).toBeDefined();
      expect(plugin?.node?.priority).toBe(1);
    });

    it('should have reference component defined', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin?.reference).toBeDefined();
      expect(plugin?.reference?.priority).toBe(1);
    });
  });

  describe('Feature Validation', () => {
    it('should be available in all slash commands list', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const hasOrderedList = commands.some((c) => c.nodeType === 'ordered-list');

      expect(hasOrderedList).toBe(true);
    });

    it('should be available in all pattern detection configs', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const hasOrderedList = patterns.some((p) => p.targetNodeType === 'ordered-list');

      expect(hasOrderedList).toBe(true);
    });

    it('should have correct pattern regex', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // Issue #667: Pattern regex now lives on plugin.pattern.detect
      const regex = plugin?.pattern?.detect;
      expect(regex).toEqual(/^1\.\s/);

      // Test regex directly
      expect(regex?.test('1. ')).toBe(true);
      expect(regex?.test('1. test')).toBe(true);
      expect(regex?.test('1.test')).toBe(false); // No space
      expect(regex?.test('2. test')).toBe(false); // Different number
    });
  });
});
