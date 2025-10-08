/**
 * Integration tests for keyboard commands with ContentEditableController
 *
 * Tests the full workflow:
 * 1. Commands registered with registry
 * 2. KeyboardEvent dispatched
 * 3. Registry executes appropriate command
 * 4. Command interacts with controller
 * 5. Controller emits appropriate events
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { KeyboardCommandRegistry } from '$lib/services/keyboard-command-registry';
import { CreateNodeCommand } from '$lib/commands/keyboard/create-node.command';
import { IndentNodeCommand } from '$lib/commands/keyboard/indent-node.command';
import { OutdentNodeCommand } from '$lib/commands/keyboard/outdent-node.command';
import { MergeNodesCommand } from '$lib/commands/keyboard/merge-nodes.command';
import { NavigateUpCommand } from '$lib/commands/keyboard/navigate-up.command';
import { NavigateDownCommand } from '$lib/commands/keyboard/navigate-down.command';
import { FormatTextCommand } from '$lib/commands/keyboard/format-text.command';
import type { ContentEditableControllerExtended } from '$lib/services/keyboard-command-registry';

describe('Keyboard Command Integration', () => {
  let registry: KeyboardCommandRegistry;
  let mockController: ContentEditableControllerExtended;
  let mockEvents: {
    createNewNode: ReturnType<typeof vi.fn>;
    indentNode: ReturnType<typeof vi.fn>;
    outdentNode: ReturnType<typeof vi.fn>;
    combineWithPrevious: ReturnType<typeof vi.fn>;
    deleteNode: ReturnType<typeof vi.fn>;
    navigateArrow: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    registry = KeyboardCommandRegistry.getInstance();
    registry.clearAll();

    // Create mock events
    mockEvents = {
      createNewNode: vi.fn(),
      indentNode: vi.fn(),
      outdentNode: vi.fn(),
      combineWithPrevious: vi.fn(),
      deleteNode: vi.fn(),
      navigateArrow: vi.fn()
    };

    // Create mock controller
    mockController = {
      events: mockEvents,
      element: document.createElement('div'),
      getCurrentColumn: vi.fn(() => 5),
      isEditing: true,
      justCreated: false,
      slashCommandDropdownActive: false,
      autocompleteDropdownActive: false,
      isAtFirstLine: vi.fn(() => true),
      isAtLastLine: vi.fn(() => true),
      getCurrentPixelOffset: vi.fn(() => 100),
      toggleFormatting: vi.fn()
    } as unknown as ContentEditableControllerExtended;

    // Add text content to element
    mockController.element.textContent = 'test content';
    mockController.element.innerHTML = 'test content';

    // Register commands (simulating what ContentEditableController does)
    // Phase 1 & 2 commands
    registry.register({ key: 'Enter' }, new CreateNodeCommand());
    registry.register({ key: 'Tab' }, new IndentNodeCommand());
    registry.register({ key: 'Tab', shift: true }, new OutdentNodeCommand());
    registry.register({ key: 'Backspace' }, new MergeNodesCommand('up'));

    // Phase 3 commands
    registry.register({ key: 'ArrowUp' }, new NavigateUpCommand());
    registry.register({ key: 'ArrowDown' }, new NavigateDownCommand());
    registry.register({ key: 'b', meta: true }, new FormatTextCommand('bold'));
    registry.register({ key: 'b', ctrl: true }, new FormatTextCommand('bold'));
    registry.register({ key: 'i', meta: true }, new FormatTextCommand('italic'));
    registry.register({ key: 'i', ctrl: true }, new FormatTextCommand('italic'));
    registry.register({ key: 'u', meta: true }, new FormatTextCommand('underline'));
    registry.register({ key: 'u', ctrl: true }, new FormatTextCommand('underline'));
  });

  afterEach(() => {
    registry.clearAll();
  });

  describe('Enter Key Integration', () => {
    it('should execute CreateNodeCommand and emit createNewNode event', async () => {
      const event = new KeyboardEvent('keydown', { key: 'Enter' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.createNewNode).toHaveBeenCalled();
    });

    it('should not execute CreateNodeCommand for Shift+Enter', async () => {
      const event = new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(false);
      expect(mockEvents.createNewNode).not.toHaveBeenCalled();
    });
  });

  describe('Tab Key Integration', () => {
    it('should execute IndentNodeCommand and emit indentNode event', async () => {
      const event = new KeyboardEvent('keydown', { key: 'Tab' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.indentNode).toHaveBeenCalledWith({
        nodeId: 'test-node'
      });
    });

    it('should execute OutdentNodeCommand for Shift+Tab and emit outdentNode event', async () => {
      const event = new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.outdentNode).toHaveBeenCalledWith({
        nodeId: 'test-node'
      });
    });
  });

  describe('Backspace Key Integration', () => {
    it('should execute MergeNodesCommand for non-empty node at start', async () => {
      // Mock controller at start position
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.getCurrentColumn.mockReturnValue(0);
      mockController.element.textContent = 'test content';

      const event = new KeyboardEvent('keydown', { key: 'Backspace' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 0,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.combineWithPrevious).toHaveBeenCalledWith({
        nodeId: 'test-node',
        currentContent: 'test content'
      });
    });

    it('should execute MergeNodesCommand and delete empty node', async () => {
      // Mock controller at start position with empty content
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.getCurrentColumn.mockReturnValue(0);
      mockController.element.textContent = '   '; // Whitespace only

      const event = new KeyboardEvent('keydown', { key: 'Backspace' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        cursorPosition: 0,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.deleteNode).toHaveBeenCalledWith({
        nodeId: 'test-node'
      });
      expect(mockEvents.combineWithPrevious).not.toHaveBeenCalled();
    });

    it('should not execute MergeNodesCommand when not at start', async () => {
      // Mock controller NOT at start position
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.getCurrentColumn.mockReturnValue(5);

      const event = new KeyboardEvent('keydown', { key: 'Backspace' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(false);
      expect(mockEvents.combineWithPrevious).not.toHaveBeenCalled();
      expect(mockEvents.deleteNode).not.toHaveBeenCalled();
    });
  });

  describe('Command Priority and Fallback', () => {
    it('should return false for unregistered keys', async () => {
      const event = new KeyboardEvent('keydown', { key: 'a' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(false);
    });

    it('should handle multiple commands registered correctly', async () => {
      // Verify all commands are registered (Phase 1, 2, and 3)
      const commands = registry.getCommands();

      expect(commands.size).toBe(12); // 4 basic + 2 navigation + 6 formatting (3 types x 2 modifiers each)
      expect(commands.has('Enter')).toBe(true);
      expect(commands.has('Tab')).toBe(true);
      expect(commands.has('Shift+Tab')).toBe(true);
      expect(commands.has('Backspace')).toBe(true);
      expect(commands.has('ArrowUp')).toBe(true);
      expect(commands.has('ArrowDown')).toBe(true);
      expect(commands.has('Meta+b')).toBe(true);
      expect(commands.has('Ctrl+b')).toBe(true);
      expect(commands.has('Meta+i')).toBe(true);
      expect(commands.has('Ctrl+i')).toBe(true);
      expect(commands.has('Meta+u')).toBe(true);
      expect(commands.has('Ctrl+u')).toBe(true);
    });
  });

  describe('Arrow Key Navigation Integration', () => {
    it('should execute NavigateUpCommand and emit navigateArrow event', async () => {
      const event = new KeyboardEvent('keydown', { key: 'ArrowUp' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.navigateArrow).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'up',
        pixelOffset: 100
      });
    });

    it('should execute NavigateDownCommand and emit navigateArrow event', async () => {
      const event = new KeyboardEvent('keydown', { key: 'ArrowDown' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.navigateArrow).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'down',
        pixelOffset: 100
      });
    });

    it('should not navigate when dropdown is active', async () => {
      mockController.slashCommandDropdownActive = true;

      const event = new KeyboardEvent('keydown', { key: 'ArrowUp' });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(false);
      expect(mockEvents.navigateArrow).not.toHaveBeenCalled();
    });
  });

  describe('Text Formatting Integration', () => {
    it('should execute FormatTextCommand for Cmd+B (bold)', async () => {
      const event = new KeyboardEvent('keydown', { key: 'b', metaKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockController.toggleFormatting).toHaveBeenCalledWith('**');
    });

    it('should execute FormatTextCommand for Ctrl+B (bold on Windows/Linux)', async () => {
      const event = new KeyboardEvent('keydown', { key: 'b', ctrlKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockController.toggleFormatting).toHaveBeenCalledWith('**');
    });

    it('should execute FormatTextCommand for Cmd+I (italic)', async () => {
      const event = new KeyboardEvent('keydown', { key: 'i', metaKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockController.toggleFormatting).toHaveBeenCalledWith('*');
    });

    it('should execute FormatTextCommand for Cmd+U (underline)', async () => {
      const event = new KeyboardEvent('keydown', { key: 'u', metaKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockController.toggleFormatting).toHaveBeenCalledWith('__');
    });

    it('should not execute FormatTextCommand when not editing', async () => {
      mockController.isEditing = false;

      const event = new KeyboardEvent('keydown', { key: 'b', metaKey: true });
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(false);
      expect(mockController.toggleFormatting).not.toHaveBeenCalled();
    });
  });

  describe('Context Building', () => {
    it('should properly use context data in command execution', async () => {
      const event = new KeyboardEvent('keydown', { key: 'Enter' });

      // Specific context with custom node type
      const context = {
        nodeId: 'custom-node-id',
        nodeType: 'task',
        content: 'custom content',
        cursorPosition: 10,
        allowMultiline: false,
        metadata: {}
      };

      const handled = await registry.execute(event, mockController, context);

      expect(handled).toBe(true);
      expect(mockEvents.createNewNode).toHaveBeenCalledWith(
        expect.objectContaining({
          afterNodeId: 'custom-node-id',
          nodeType: 'task'
        })
      );
    });
  });
});
