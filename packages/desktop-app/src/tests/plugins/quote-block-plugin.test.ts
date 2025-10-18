/**
 * QuoteBlockNode Tests
 *
 * Tests for quote block plugin functionality including:
 * - Pattern detection
 * - Plugin registration
 * - Slash command configuration
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('QuoteBlockNode Plugin', () => {
  describe('Pattern Detection', () => {
    it('should detect "> " pattern at start of content', () => {
      const content = '> Hello world';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('quote-block');
    });

    it('should detect "> " with multiline content', () => {
      const content = '> First line\n> Second line';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('quote-block');
    });

    it('should NOT detect ">" without space', () => {
      const content = '>Hello';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Pattern requires space after >
      expect(detection?.config.targetNodeType).not.toBe('quote-block');
    });

    it('should NOT detect > in middle of content', () => {
      const content = 'Hello > World';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Pattern must be at start
      expect(detection?.config.targetNodeType).not.toBe('quote-block');
    });
  });

  describe('Plugin Registration', () => {
    it('should have quote-block plugin registered', () => {
      expect(pluginRegistry.hasPlugin('quote-block')).toBe(true);
    });

    it('should have pattern detection configured', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const quotePattern = patterns.find((p) => p.targetNodeType === 'quote-block');

      expect(quotePattern).toBeDefined();
      expect(quotePattern?.cleanContent).toBe(false); // Keep > prefix in content
    });

    it('should have node component configured for lazy loading', () => {
      const plugin = pluginRegistry.getPlugin('quote');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('Slash Command', () => {
    it('should have /quote slash command registered', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const quoteCommand = commands.find((cmd) => cmd.id === 'quote');

      expect(quoteCommand).toBeDefined();
      expect(quoteCommand?.name).toBe('Quote Block');
      expect(quoteCommand?.shortcut).toBe('>');
      expect(quoteCommand?.contentTemplate).toBe('> ');
      expect(quoteCommand?.nodeType).toBe('quote-block');
    });
  });

  describe('Configuration', () => {
    it('should be configured to allow children', () => {
      const plugin = pluginRegistry.getPlugin('quote-block');
      expect(plugin?.config.canHaveChildren).toBe(true);
    });

    it('should be configured to allow being a child', () => {
      const plugin = pluginRegistry.getPlugin('quote-block');
      expect(plugin?.config.canBeChild).toBe(true);
    });
  });
});
