/**
 * CodeBlockNode Tests
 *
 * Tests for code block plugin functionality including:
 * - Pattern detection
 * - Plugin registration
 * - Slash command configuration
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('CodeBlockNode Plugin', () => {
  describe('Pattern Detection', () => {
    it('should detect ```\\n pattern and extract plaintext language', () => {
      const content = '```\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('code-block');
      expect(detection?.metadata.language).toBe('plaintext');
    });

    it('should detect ```javascript\\n pattern and extract language', () => {
      const content = '```javascript\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('code-block');
      expect(detection?.metadata.language).toBe('javascript');
    });

    it('should detect ```python\\n pattern', () => {
      const content = '```python\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.metadata.language).toBe('python');
    });

    it('should detect ```typescript\\n pattern', () => {
      const content = '```typescript\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.metadata.language).toBe('typescript');
    });

    it('should NOT detect ``` without newline', () => {
      const content = '```';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Pattern requires newline (user must press Shift+Enter)
      expect(detection?.config.targetNodeType).not.toBe('code-block');
    });

    it('should NOT detect ```Hello without newline', () => {
      const content = '```Hello';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match - avoids treating user content as language
      expect(detection?.config.targetNodeType).not.toBe('code-block');
    });
  });

  describe('Plugin Registration', () => {
    it('should have code-block plugin registered', () => {
      expect(pluginRegistry.hasPlugin('code-block')).toBe(true);
    });

    it('should have pattern detection configured', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const codeBlockPattern = patterns.find((p) => p.targetNodeType === 'code-block');

      expect(codeBlockPattern).toBeDefined();
      expect(codeBlockPattern?.cleanContent).toBe(false); // Keep fence for language parsing
    });

    it('should have node component configured for lazy loading', () => {
      const plugin = pluginRegistry.getPlugin('code-block');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('Slash Command', () => {
    it('should have /code slash command registered', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const codeCommand = commands.find((cmd) => cmd.id === 'code');

      expect(codeCommand).toBeDefined();
      expect(codeCommand?.name).toBe('Code Block');
      expect(codeCommand?.shortcut).toBe('```');
      expect(codeCommand?.contentTemplate).toBe('```plaintext\n\n```');
      expect(codeCommand?.nodeType).toBe('code-block');
    });
  });

  describe('Configuration', () => {
    it('should be configured as leaf node (no children)', () => {
      const plugin = pluginRegistry.getPlugin('code-block');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should be configured to allow being a child', () => {
      const plugin = pluginRegistry.getPlugin('code-block');
      expect(plugin?.config.canBeChild).toBe(true);
    });
  });
});
