/**
 * HorizontalLine Plugin Tests
 *
 * Tests for horizontal line plugin functionality including:
 * - Pattern detection
 * - Plugin registration
 * - Slash command configuration
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('HorizontalLine Plugin', () => {
  describe('Pattern Detection', () => {
    it('should detect --- pattern', () => {
      const content = '---';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('horizontal-line');
    });

    it('should detect ---- (4+ dashes)', () => {
      const content = '----';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('horizontal-line');
    });

    it('should NOT detect -- (less than 3 dashes)', () => {
      const content = '--';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match horizontal-line
      expect(detection?.config.targetNodeType).not.toBe('horizontal-line');
    });

    it('should NOT detect --- with text after', () => {
      const content = '--- text';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match horizontal-line (has trailing text)
      expect(detection?.config.targetNodeType).not.toBe('horizontal-line');
    });
  });

  describe('Plugin Registration', () => {
    it('should have horizontal-line plugin registered', () => {
      expect(pluginRegistry.hasPlugin('horizontal-line')).toBe(true);
    });

    it('should have pattern detection configured', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const hrPattern = patterns.find((p) => p.targetNodeType === 'horizontal-line');

      expect(hrPattern).toBeDefined();
    });

    it('should have node component configured for lazy loading', () => {
      const plugin = pluginRegistry.getPlugin('horizontal-line');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('Slash Command', () => {
    it('should have /hr slash command registered', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const hrCommand = commands.find((cmd) => cmd.id === 'hr');

      expect(hrCommand).toBeDefined();
      expect(hrCommand?.name).toBe('Horizontal Line');
      expect(hrCommand?.shortcut).toBe('---');
      expect(hrCommand?.contentTemplate).toBe('---');
      expect(hrCommand?.nodeType).toBe('horizontal-line');
    });
  });

  describe('Configuration', () => {
    it('should be configured as leaf node (no children)', () => {
      const plugin = pluginRegistry.getPlugin('horizontal-line');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should be configured to allow being a child', () => {
      const plugin = pluginRegistry.getPlugin('horizontal-line');
      expect(plugin?.config.canBeChild).toBe(true);
    });

    it('should not accept content merges', () => {
      const plugin = pluginRegistry.getPlugin('horizontal-line');
      expect(plugin?.acceptsContentMerge).toBe(false);
    });
  });
});
