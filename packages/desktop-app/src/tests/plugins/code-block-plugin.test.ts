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

    it('should NOT detect ```javascript\\n pattern (prevents content being treated as language)', () => {
      // Pattern only matches ```\n (immediate newline) to avoid treating user content as language
      // e.g., "```Hello\n" should NOT match with "Hello" as language
      const content = '```javascript\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match code-block - language is set via dropdown only
      expect(detection?.config.targetNodeType).not.toBe('code-block');
    });

    it('should NOT detect ```python\\n pattern (language via dropdown only)', () => {
      const content = '```python\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match - prevents "```Hello\n" from treating Hello as language
      expect(detection?.config.targetNodeType).not.toBe('code-block');
    });

    it('should NOT detect ```typescript\\n pattern (language via dropdown only)', () => {
      const content = '```typescript\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should not match - language selection is via dropdown UI
      expect(detection?.config.targetNodeType).not.toBe('code-block');
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

    it('should NOT detect ```Hello\\n (prevents user content being treated as language)', () => {
      // REGRESSION TEST: This was a bug where typing ``` before "Hello\n```"
      // would match "```Hello\n" and treat "Hello" as the language identifier,
      // causing the user's content to disappear
      const content = '```Hello\n```';
      const detection = pluginRegistry.detectPatternInContent(content);

      // Should NOT match code-block - we don't want "Hello" to be parsed as language
      // The correct pattern is ```\n only (language selected via dropdown)
      expect(detection?.config.targetNodeType).not.toBe('code-block');
    });

    it('should detect ```\\nHello\\n``` and preserve content', () => {
      // When properly formatted with newline after opening fence,
      // content should be preserved (not stripped)
      const content = '```\nHello\n```';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('code-block');
      // cleanContent should be false so content is preserved
      expect(detection?.config.cleanContent).toBe(false);
      // Language defaults to plaintext (user selects via dropdown)
      expect(detection?.metadata.language).toBe('plaintext');
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
      expect(codeCommand?.contentTemplate).toBe('```\n\n```');
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

  describe('Auto-completion', () => {
    it('should have cleanContent set to false (keeps fence for auto-completion)', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const codeBlockPattern = patterns.find((p) => p.targetNodeType === 'code-block');

      expect(codeBlockPattern).toBeDefined();
      expect(codeBlockPattern?.cleanContent).toBe(false);
    });

    it('should have contentTemplate configured for auto-completion', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const codeBlockPattern = patterns.find((p) => p.targetNodeType === 'code-block');

      expect(codeBlockPattern).toBeDefined();
      expect(codeBlockPattern?.contentTemplate).toBe('```\n\n```');
    });

    it('should have desiredCursorPosition set to 4 (after opening fence)', () => {
      const patterns = pluginRegistry.getAllPatternDetectionConfigs();
      const codeBlockPattern = patterns.find((p) => p.targetNodeType === 'code-block');

      expect(codeBlockPattern).toBeDefined();
      expect(codeBlockPattern?.desiredCursorPosition).toBe(4);
    });

    it('pattern detection should preserve user content (no contentTemplate)', () => {
      // Pattern detection must NOT use contentTemplate - it should preserve user-typed content
      // contentTemplate is ONLY for slash commands (e.g., /code), not pattern detection
      const content = '```\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.cleanContent).toBe(false);
      // contentTemplate should be undefined during pattern detection
      // User content like "```\nHello" should be preserved, not replaced with template
      expect(detection?.config.contentTemplate).toBeUndefined();
    });

    it('pattern detection for ```\\n should use plaintext language (dropdown for language selection)', () => {
      // Pattern only matches ```\n - language is always plaintext, user selects via dropdown
      // This prevents "```Hello\n" from being parsed as language="Hello"
      const content = '```\n';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.metadata.language).toBe('plaintext');
      expect(detection?.config.cleanContent).toBe(false);
      // contentTemplate should be undefined - user content preserved
      expect(detection?.config.contentTemplate).toBeUndefined();
    });
  });
});
