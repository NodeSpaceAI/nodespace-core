/**
 * Table Plugin Tests
 *
 * Tests for table plugin functionality including:
 * - Pattern detection
 * - Plugin registration
 * - Slash command configuration
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('Table Plugin', () => {
  describe('Pattern Detection', () => {
    it('should detect | followed by space', () => {
      const content = '| ';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('table');
    });

    it('should detect | Column 1 | Column 2 |', () => {
      const content = '| Column 1 | Column 2 |';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('table');
    });

    it('should NOT detect | alone (revert pattern)', () => {
      const content = '|';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match table (revert pattern)
      expect(detection?.config.targetNodeType).not.toBe('table');
    });
  });

  describe('Plugin Registration', () => {
    it('should have table plugin registered', () => {
      expect(pluginRegistry.hasPlugin('table')).toBe(true);
    });

    it('should have pattern detection configured', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const tablePattern = patterns.find((p) => p.targetNodeType === 'table');

      expect(tablePattern).toBeDefined();
    });

    it('should have node component configured for lazy loading', () => {
      const plugin = pluginRegistry.getPlugin('table');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('Slash Command', () => {
    it('should have /table slash command registered', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const tableCommand = commands.find((cmd) => cmd.id === 'table');

      expect(tableCommand).toBeDefined();
      expect(tableCommand?.name).toBe('Table');
      expect(tableCommand?.shortcut).toBe('|');
      expect(tableCommand?.contentTemplate).toContain('| Column 1 | Column 2 |');
      expect(tableCommand?.nodeType).toBe('table');
    });

    it('should have multi-line content template', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const tableCommand = commands.find((cmd) => cmd.id === 'table');

      expect(tableCommand?.contentTemplate).toContain('| --- | --- |');
    });
  });

  describe('Configuration', () => {
    it('should be configured as leaf node (no children)', () => {
      const plugin = pluginRegistry.getPlugin('table');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should be configured to allow being a child', () => {
      const plugin = pluginRegistry.getPlugin('table');
      expect(plugin?.config.canBeChild).toBe(true);
    });

    it('should not accept content merges', () => {
      const plugin = pluginRegistry.getPlugin('table');
      expect(plugin?.acceptsContentMerge).toBe(false);
    });
  });
});
