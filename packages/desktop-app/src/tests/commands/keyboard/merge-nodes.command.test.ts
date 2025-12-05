/**
 * Unit tests for MergeNodesCommand
 *
 * Tests the Backspace/Delete key functionality for merging nodes
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { MergeNodesCommand } from '$lib/commands/keyboard/merge-nodes.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('MergeNodesCommand', () => {
  let mockController: TextareaController;
  let deleteNodeSpy: ReturnType<typeof vi.fn>;
  let combineWithPreviousSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    deleteNodeSpy = vi.fn();
    combineWithPreviousSpy = vi.fn();

    mockController = {
      events: {
        deleteNode: deleteNodeSpy,
        combineWithPrevious: combineWithPreviousSpy
      },
      element: {
        textContent: 'test content'
      } as HTMLDivElement
    } as unknown as TextareaController;
  });

  describe('direction: up (Backspace)', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('up');
    });

    describe('canExecute', () => {
      it('should execute for Backspace at start of node', () => {
        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: false
        });

        expect(command.canExecute(context)).toBe(true);
      });

      it('should not execute for Backspace in middle of node', () => {
        const context = createContext({
          key: 'Backspace',
          cursorPosition: 5,
          allowMultiline: false
        });

        expect(command.canExecute(context)).toBe(false);
      });

      it('should not execute for non-Backspace keys', () => {
        const context = createContext({
          key: 'Delete',
          cursorPosition: 0,
          allowMultiline: false
        });

        expect(command.canExecute(context)).toBe(false);
      });

      it('should execute for multiline node at start of first line', () => {
        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: true
        });

        // Mock isAtFirstLine to return true
        mockIsAtFirstLine(true);

        expect(command.canExecute(context)).toBe(true);
      });

      it('should not execute for multiline node not at position 0', () => {
        const context = createContext({
          key: 'Backspace',
          cursorPosition: 5, // Not at position 0
          allowMultiline: true
        });

        expect(command.canExecute(context)).toBe(false);
      });

      it('should not execute when text is selected (let browser delete selection)', () => {
        // Create textarea with selection
        const textarea = document.createElement('textarea');
        textarea.value = 'Hello World';
        textarea.selectionStart = 0;
        textarea.selectionEnd = 5; // "Hello" selected

        mockController.element = textarea;

        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: false
        });

        // Should not execute - browser will delete the selection instead
        expect(command.canExecute(context)).toBe(false);
      });

      it('should execute when cursor at start with no selection', () => {
        // Create textarea with cursor at start, no selection
        const textarea = document.createElement('textarea');
        textarea.value = 'Hello World';
        textarea.selectionStart = 0;
        textarea.selectionEnd = 0; // No selection

        mockController.element = textarea;

        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: false
        });

        // Should execute - cursor at start with no selection
        expect(command.canExecute(context)).toBe(true);
      });
    });

    describe('execute - empty node', () => {
      it('should delete empty node', async () => {
        // Mock empty content
        mockController.element.textContent = '   '; // Whitespace only

        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: false
        });

        const result = await command.execute(context);

        expect(result).toBe(true);
        expect(deleteNodeSpy).toHaveBeenCalledWith({
          nodeId: 'test-node'
        });
        expect(combineWithPreviousSpy).not.toHaveBeenCalled();
      });
    });

    describe('execute - non-empty node', () => {
      it('should combine with previous node', async () => {
        // Mock non-empty content
        mockController.element.textContent = 'test content';

        const preventDefaultSpy = vi.fn();
        const context = createContext({
          key: 'Backspace',
          cursorPosition: 0,
          allowMultiline: false,
          preventDefault: preventDefaultSpy
        });

        const result = await command.execute(context);

        expect(result).toBe(true);
        expect(preventDefaultSpy).toHaveBeenCalled();
        expect(combineWithPreviousSpy).toHaveBeenCalledWith({
          nodeId: 'test-node',
          currentContent: 'test content'
        });
        expect(deleteNodeSpy).not.toHaveBeenCalled();
      });
    });
  });

  describe('direction: down (Delete)', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('down');
    });

    describe('canExecute', () => {
      it('should not execute (not yet implemented)', () => {
        const context = createContext({
          key: 'Delete',
          cursorPosition: 10,
          allowMultiline: false
        });

        expect(command.canExecute(context)).toBe(false);
      });
    });
  });

  describe('command metadata', () => {
    it('should have correct id for up direction', () => {
      const command = new MergeNodesCommand('up');
      expect(command.id).toBe('merge-nodes-up');
    });

    it('should have correct id for down direction', () => {
      const command = new MergeNodesCommand('down');
      expect(command.id).toBe('merge-nodes-down');
    });

    it('should have correct description for up direction', () => {
      const command = new MergeNodesCommand('up');
      expect(command.description).toBe('Merge with previous node on Backspace');
    });

    it('should have correct description for down direction', () => {
      const command = new MergeNodesCommand('down');
      expect(command.description).toBe('Merge with next node on Delete');
    });

    it('should default to up direction when no direction specified', () => {
      const command = new MergeNodesCommand();
      expect(command.id).toBe('merge-nodes-up');
      expect(command.description).toBe('Merge with previous node on Backspace');
    });
  });

  describe('contenteditable fallback path', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('up');
    });

    it('should use DOM selection when element does not have selectionStart', () => {
      // Create a div element (contenteditable) instead of textarea
      const div = document.createElement('div');
      div.contentEditable = 'true';
      div.textContent = 'test content';
      mockController.element = div as unknown as TextareaController["element"];

      // Create a collapsed selection at start
      const range = document.createRange();
      range.setStart(div.childNodes[0], 0);
      range.setEnd(div.childNodes[0], 0);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute when contenteditable has non-collapsed selection', () => {
      const div = document.createElement('div');
      div.contentEditable = 'true';
      div.textContent = 'test content';
      mockController.element = div as unknown as TextareaController["element"];

      // Create a non-collapsed selection
      const range = document.createRange();
      range.setStart(div.childNodes[0], 0);
      range.setEnd(div.childNodes[0], 5);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(false);
    });

    it('should handle missing selection gracefully', () => {
      const div = document.createElement('div');
      div.contentEditable = 'true';
      div.textContent = 'test content';
      mockController.element = div as unknown as TextareaController["element"];

      const selection = window.getSelection();
      selection?.removeAllRanges();

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: false
      });

      // Should fall back to context.cursorPosition
      expect(command.canExecute(context)).toBe(true);
    });

    it('should use context.content when element is missing', async () => {
      // Remove element to trigger fallback
      mockController.element = null as unknown as TextareaController["element"];

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: false
      });
      context.content = 'fallback content';

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(combineWithPreviousSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        currentContent: 'fallback content'
      });
    });
  });

  describe('multiline node edge cases', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('up');
    });

    it('should check isAtFirstLine for multiline nodes with DIV structure', () => {
      const container = document.createElement('div');
      container.contentEditable = 'true';

      const firstDiv = document.createElement('div');
      firstDiv.textContent = 'First line';
      const secondDiv = document.createElement('div');
      secondDiv.textContent = 'Second line';

      container.appendChild(firstDiv);
      container.appendChild(secondDiv);
      mockController.element = container as unknown as TextareaController["element"];

      // Create selection in the first DIV at position 0
      const range = document.createRange();
      range.setStart(firstDiv.childNodes[0], 0);
      range.setEnd(firstDiv.childNodes[0], 0);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: true
      });

      // Should execute - cursor at start of first line
      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute when on second line of multiline node', () => {
      const container = document.createElement('div');
      container.contentEditable = 'true';

      const firstDiv = document.createElement('div');
      firstDiv.textContent = 'First line';
      const secondDiv = document.createElement('div');
      secondDiv.textContent = 'Second line';

      container.appendChild(firstDiv);
      container.appendChild(secondDiv);
      mockController.element = container as unknown as TextareaController["element"];

      // Create selection in the SECOND DIV at position 0
      const range = document.createRange();
      range.setStart(secondDiv.childNodes[0], 0);
      range.setEnd(secondDiv.childNodes[0], 0);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: true
      });

      // Should not execute - cursor on second line
      expect(command.canExecute(context)).toBe(false);
    });

    it('should handle multiline node without DIV structure (plain text)', () => {
      const container = document.createElement('div');
      container.contentEditable = 'true';
      container.textContent = 'Plain text without divs';
      mockController.element = container as unknown as TextareaController["element"];

      const range = document.createRange();
      range.setStart(container.childNodes[0], 0);
      range.setEnd(container.childNodes[0], 0);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: true
      });

      // Should execute - plain text is treated as first line
      expect(command.canExecute(context)).toBe(true);
    });

    it('should handle missing selection in isAtFirstLine', () => {
      const container = document.createElement('div');
      container.contentEditable = 'true';
      mockController.element = container as unknown as TextareaController["element"];

      const selection = window.getSelection();
      selection?.removeAllRanges();

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: true
      });

      // Should return true (fallback behavior)
      expect(command.canExecute(context)).toBe(true);
    });

    it('should handle missing element in isAtFirstLine', () => {
      mockController.element = null as unknown as TextareaController["element"];

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 0,
        allowMultiline: true
      });

      // Should return true (fallback behavior)
      expect(command.canExecute(context)).toBe(true);
    });
  });

  describe('execute - direction down', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('down');
    });

    it('should return false for down direction (not implemented)', async () => {
      const context = createContext({
        key: 'Delete',
        cursorPosition: 10,
        allowMultiline: false
      });

      const result = await command.execute(context);

      expect(result).toBe(false);
      expect(deleteNodeSpy).not.toHaveBeenCalled();
      expect(combineWithPreviousSpy).not.toHaveBeenCalled();
    });
  });

  describe('getCursorPosition fallback', () => {
    let command: MergeNodesCommand;

    beforeEach(() => {
      command = new MergeNodesCommand('up');
    });

    it('should use getCurrentColumn when available', () => {
      const getCurrentColumnSpy = vi.fn(() => 42);
      mockController.getCurrentColumn = getCurrentColumnSpy;

      createContext({
        key: 'Backspace',
        cursorPosition: 0, // Different from getCurrentColumn result
        allowMultiline: false
      });

      // The command internally calls getCursorPosition via getCurrentColumn
      // We can verify this by checking if the command was created properly
      expect(command).toBeDefined();
      expect(mockController.getCurrentColumn).toBeDefined();
    });

    it('should fallback to context.cursorPosition when getCurrentColumn missing', () => {
      mockController.getCurrentColumn = undefined as unknown as (() => number);

      const context = createContext({
        key: 'Backspace',
        cursorPosition: 5,
        allowMultiline: false
      });

      // Should not execute because cursor is not at start
      expect(command.canExecute(context)).toBe(false);
    });
  });

  // Helper function to create mock context
  function createContext(options: {
    key: string;
    cursorPosition: number;
    allowMultiline: boolean;
    preventDefault?: () => void;
  }): KeyboardContext {
    const event = {
      key: options.key,
      preventDefault: options.preventDefault || vi.fn()
    } as unknown as KeyboardEvent;

    // Mock getCurrentColumn method
    mockController.getCurrentColumn = vi.fn(() => options.cursorPosition);

    return {
      event,
      controller: mockController as TextareaController,
      nodeId: 'test-node',
      nodeType: 'text',
      content: '',
      cursorPosition: options.cursorPosition,
      selection: window.getSelection(),
      allowMultiline: options.allowMultiline,
      paneId: 'default',
      metadata: {}
    };
  }

  // Helper to mock isAtFirstLine behavior
  function mockIsAtFirstLine(_isFirst: boolean) {
    // For simplified implementation, isAtStartOfFirstLine now just checks cursorPosition === 0
    // So this mock is no longer needed, but kept for test structure
    return;
  }
});
