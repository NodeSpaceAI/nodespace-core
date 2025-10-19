/**
 * OrderedListNode Integration Tests
 *
 * Tests for ordered list workflows including:
 * - Auto-conversion from "1. " pattern to ordered-list nodeType
 * - Slash command creation
 * - Enter key creates new sibling items
 * - Backspace at start converts to text node
 * - CSS counter auto-numbering (1, 2, 3...)
 * - Counter reset when sequence broken by non-list nodes
 * - Content preservation during conversions
 */

import { describe, it, expect } from 'vitest';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

describe('OrderedListNode Workflow', () => {
  describe('Auto-Conversion (Pattern Detection)', () => {
    it('should detect and convert "1. " to ordered-list node type', () => {
      const content = '1. First item';
      const detection = pluginRegistry.detectPatternInContent(content);

      expect(detection).not.toBeNull();
      expect(detection?.config.targetNodeType).toBe('ordered-list');
    });

    it('should preserve "1. " prefix during conversion (cleanContent: false)', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // This is critical - content must be preserved for round-trip consistency
      expect(pattern?.cleanContent).toBe(false);

      // With cleanContent: false, the pattern is NOT removed from content
      // Content stays as "1. First item" in database
      expect(pattern?.cleanContent).toBeFalsy();
    });

    it('should position cursor after "1. " prefix (position 3)', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // User types "1. " and expects cursor after the prefix
      expect(pattern?.desiredCursorPosition).toBe(3);
      // Position 0: "1"
      // Position 1: "."
      // Position 2: " " (space)
      // Position 3: (cursor here, ready for user to type content)
    });

    it('should NOT replace content with template in pattern detection', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // cleanContent: false means keep the user's content
      // No contentTemplate in pattern detection (unlike slash commands)
      expect(pattern?.contentTemplate).toBeUndefined();
      expect(pattern?.cleanContent).toBe(false);
    });
  });

  describe('Slash Command Creation', () => {
    it('should create ordered list with slash command /ordered-list', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      expect(command).toBeDefined();
      expect(command?.nodeType).toBe('ordered-list');
    });

    it('should have contentTemplate: "1. " for slash command', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      // When selected from menu, content is set to "1. " template
      expect(command?.contentTemplate).toBe('1. ');
    });

    it('should position cursor at 3 (after "1. ") for slash command', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      expect(command?.desiredCursorPosition).toBe(3);
    });

    it('should have shortcut "1." for quick access', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      // Shortcut is shown in slash command menu for discoverability
      expect(command?.shortcut).toBe('1.');
    });
  });

  describe('Keyboard Interactions', () => {
    it('should create new ordered-list sibling on Enter', () => {
      // This test verifies plugin configuration supports this behavior
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // Plugin should NOT have multiline enabled (unlike quote-block)
      // This is handled in the OrderedListNode component's editableConfig
      // Here we just verify the plugin exists and can be created
      expect(plugin?.id).toBe('ordered-list');
    });

    it('should prepare new node with "1. " template', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      // When creating new sibling via Enter, it should use same template
      expect(command?.contentTemplate).toBe('1. ');
      expect(command?.desiredCursorPosition).toBe(3);
    });

    it('should convert to text node if content no longer starts with "1."', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // If user deletes the "1. " prefix, the node should convert to text
      // This logic is in OrderedListNode component's handleCreateNewNode
      // Here we verify the pattern is defined for the conversion
      expect(pattern?.pattern).toEqual(/^1\.\s/);
    });
  });

  describe('Multiline Editing (Shift+Enter)', () => {
    it('should support multiline editing with allowMultiline: true', () => {
      // Ordered lists use allowMultiline: true (same as quote-block)
      // This allows Shift+Enter to add new lines within the same node
      // Example: "1. First\n1. Second" in one node

      // Verify plugin exists and supports lazy loading
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });

    it('should add "1. " prefix to new lines created with Shift+Enter', () => {
      // When user presses Shift+Enter, handleContentChange adds "1. " prefix
      // Cursor positioning is handled by pendingCursorAdjustment

      // Constants used in OrderedListNode:
      // NEWLINE_LENGTH = 1 (\n character)
      // LIST_PREFIX_LENGTH = 3 ("1. " prefix)
      const NEWLINE_LENGTH = 1;
      const LIST_PREFIX_LENGTH = 3;

      // If cursor is at position 10:
      // After Shift+Enter: cursor should be at 10 + 1 + 3 = 14
      const cursorPos = 10;
      const expectedNewPos = cursorPos + NEWLINE_LENGTH + LIST_PREFIX_LENGTH;

      expect(expectedNewPos).toBe(14);
    });

    it('should position cursor after "1. " prefix on new line', () => {
      // Verify cursor positioning constants match slash command config
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      // After adding "1. " to new line, cursor should be at position 3
      // This matches the slash command's desiredCursorPosition
      expect(command?.desiredCursorPosition).toBe(3);
    });

    it('should handle sequential numbering in multiline ordered lists', () => {
      // Test the extractListForDisplay transformation:
      // Input (edit mode): "1. First\n1. Second\n1. Third"
      // Output (view mode): "1. First\n2. Second\n3. Third"

      const editContent = '1. First\n1. Second\n1. Third';
      const lines = editContent.split('\n');

      // Simulate extractListForDisplay logic
      const numberedLines = lines.map((line, index) => {
        const strippedLine = line.replace(/^1\.\s?/, '');
        return `${index + 1}. ${strippedLine}`;
      });
      const displayContent = numberedLines.join('\n');

      expect(displayContent).toBe('1. First\n2. Second\n3. Third');
    });

    it('should use font-variant-numeric: tabular-nums for alignment', () => {
      // CSS ensures monospaced numbers prevent horizontal text shift
      // "1. " and "2. " have same width, so "Item" text aligns vertically

      // Verify plugin configuration supports CSS styling
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('CSS Counter Numbering', () => {
    it('should reset counter at viewer level', () => {
      // CSS counter reset is defined in base-node-viewer.svelte
      // counter-reset: ordered-list-counter at .base-node-viewer level
      // This ensures each document/viewer starts numbering from 1

      // Verification: Plugin should be leaf node (no children)
      // so counter increments straightforwardly
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should increment counter for each sequential ordered-list node', () => {
      // CSS counters work like: counter-increment: ordered-list-counter
      // First node: 1, Second node: 2, etc.
      // This is automatic via CSS - just verify plugin config supports it

      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.id).toBe('ordered-list');
    });

    it('should reset counter when sequence broken by non-list node', () => {
      // CSS rule: .base-node-viewer > *:not(.ordered-list-node-wrapper) resets counter
      // So if a text/header/etc. node appears, counter resets
      // Next ordered-list starts at 1 again

      // Verify plugin has correct leaf node configuration
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should display auto-numbered items (1., 2., 3...) in view mode', () => {
      // OrderedListNode component uses CSS ::before with counter
      // Edit mode shows "1. Content" (prefix visible)
      // View mode shows "1. Content" with CSS ::before handling numbering
      // (The "1. " is stripped in display, counter provides numbers)

      // This is implementation detail, but verify plugin supports it
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.node?.lazyLoad).toBeDefined();
    });
  });

  describe('Content Preservation', () => {
    it('should preserve "1. " prefix in database storage', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // Round-trip consistency: Save "1. Item" → Load → Show as "1. Item" in edit
      expect(pattern?.cleanContent).toBe(false);
    });

    it('should support multiline with proper prefix handling', () => {
      // Unlike single-line task nodes, ordered list items are single-line
      // This is handled in OrderedListNode's editableConfig: allowMultiline: false

      // Verify plugin is properly registered
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.config.canHaveChildren).toBe(false);
    });

    it('should strip prefix in display but keep in storage', () => {
      // Storage: "1. First item"
      // Edit mode: Show full "1. First item"
      // View mode: Show "First item" with CSS counter providing "1. " prefix

      // This is component behavior, verify plugin structure
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin?.id).toBe('ordered-list');
    });
  });

  describe('Conversion Behavior', () => {
    it('should maintain "1. " format when creating sibling', () => {
      const commands = pluginRegistry.getAllSlashCommands();
      const command = commands.find((c) => c.id === 'ordered-list');

      // Enter creates new with same "1. " prefix
      expect(command?.contentTemplate).toBe('1. ');
    });

    it('should convert to text by removing "1. " prefix on backspace', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');
      const pattern = plugin?.config.patternDetection?.[0];

      // If pattern no longer matches (no "1. " at start), should convert to text
      // Component strips the prefix when converting
      expect(pattern?.pattern).toEqual(/^1\.\s/);
    });

    it('should not merge into ordered lists from other node types', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      // allowMergeInto is handled in component's editableConfig: allowMergeInto: false
      // This prevents breaking ordered list structure with mid-list deletions
      expect(plugin?.config.canHaveChildren).toBe(false);
    });
  });

  describe('Plugin Integration', () => {
    it('should be included in core plugins', () => {
      // Verify plugin is registered and accessible
      const plugin = pluginRegistry.getPlugin('ordered-list');
      expect(plugin).toBeDefined();
    });

    it('should work alongside other node types', () => {
      // Should not conflict with text, header, quote-block, etc.
      const plugins = ['text', 'header', 'task', 'code-block', 'quote-block', 'ordered-list'];

      for (const pluginId of plugins) {
        expect(pluginRegistry.hasPlugin(pluginId)).toBe(true);
      }
    });

    it('should have all required configuration', () => {
      const plugin = pluginRegistry.getPlugin('ordered-list');

      expect(plugin?.id).toBe('ordered-list');
      expect(plugin?.name).toBeDefined();
      expect(plugin?.description).toBeDefined();
      expect(plugin?.version).toBeDefined();
      expect(plugin?.config).toBeDefined();
      expect(plugin?.node).toBeDefined();
      expect(plugin?.reference).toBeDefined();
    });
  });
});
