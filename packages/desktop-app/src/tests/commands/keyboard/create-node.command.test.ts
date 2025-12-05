/**
 * Unit tests for CreateNodeCommand
 *
 * Tests the Enter key functionality including:
 * - canExecute conditions
 * - Smart content splitting
 * - Header and syntax awareness
 * - Multiline handling
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { CreateNodeCommand } from '$lib/commands/keyboard/create-node.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('CreateNodeCommand', () => {
  let command: CreateNodeCommand;
  let mockController: Partial<TextareaController>;
  let createNewNodeSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    command = new CreateNodeCommand();
    createNewNodeSpy = vi.fn();

    mockController = {
      events: {
        createNewNode: createNewNodeSpy
      },
      element: {
        textContent: 'test content',
        innerHTML: 'test content'
      },
      setLiveFormattedContent: vi.fn(), // Mock for inline formatting preservation
      getCursorPositionInMarkdown: vi.fn().mockImplementation(function (this: unknown) {
        // Return the cursor position from context (simulating markdown-aware position)
        return (this as { cursorPosition?: number })?.cursorPosition ?? 0;
      })
    } as unknown as TextareaController;
  });

  describe('canExecute', () => {
    it('should execute for regular Enter key', () => {
      const context = createContext({ key: 'Enter' });
      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for Shift+Enter', () => {
      const context = createContext({ key: 'Enter', shiftKey: true });
      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when slash command dropdown is active', () => {
      // Mock DOM query
      const originalQuerySelector = document.querySelector;
      document.querySelector = vi.fn((selector) => {
        if (selector === '[role="listbox"][aria-label="Slash command palette"]') {
          return {} as Element; // Dropdown exists
        }
        return null;
      });

      const context = createContext({ key: 'Enter' });
      expect(command.canExecute(context)).toBe(false);

      // Restore
      document.querySelector = originalQuerySelector;
    });

    it('should not execute when autocomplete dropdown is active', () => {
      // Mock DOM query
      const originalQuerySelector = document.querySelector;
      document.querySelector = vi.fn((selector) => {
        if (selector === '[role="listbox"][aria-label="Node reference autocomplete"]') {
          return {} as Element; // Dropdown exists
        }
        return null;
      });

      const context = createContext({ key: 'Enter' });
      expect(command.canExecute(context)).toBe(false);

      // Restore
      document.querySelector = originalQuerySelector;
    });

    it('should not execute for non-Enter keys', () => {
      const context = createContext({ key: 'a' });
      expect(command.canExecute(context)).toBe(false);
    });
  });

  describe('execute - basic splitting', () => {
    it('should split content in the middle', async () => {
      const context = createContext({
        key: 'Enter',
        content: 'Hello World',
        cursorPosition: 6
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith({
        afterNodeId: 'test-node',
        nodeType: 'text',
        currentContent: 'Hello ',
        newContent: 'World',
        originalContent: 'Hello World',
        cursorAtBeginning: false,
        insertAtBeginning: false,
        newNodeCursorPosition: 0
      });
    });

    it('should create empty node above when at beginning', async () => {
      const context = createContext({
        key: 'Enter',
        content: 'Hello World',
        cursorPosition: 0
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith({
        afterNodeId: 'test-node',
        nodeType: 'text',
        currentContent: 'Hello World',
        newContent: '',
        originalContent: 'Hello World',
        cursorAtBeginning: true,
        insertAtBeginning: true,
        focusOriginalNode: true
      });
    });

    it('should split at end of content', async () => {
      const context = createContext({
        key: 'Enter',
        content: 'Hello World',
        cursorPosition: 11
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith({
        afterNodeId: 'test-node',
        nodeType: 'text',
        currentContent: 'Hello World',
        newContent: '',
        originalContent: 'Hello World',
        cursorAtBeginning: false,
        insertAtBeginning: false,
        newNodeCursorPosition: 0
      });
    });
  });

  describe('execute - header handling', () => {
    it('should create empty node above when cursor is within header syntax', async () => {
      const context = createContext({
        key: 'Enter',
        content: '# Header Text',
        cursorPosition: 1 // Within '#'
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: '# Header Text',
          newContent: '',
          insertAtBeginning: true,
          focusOriginalNode: true
        })
      );
    });

    it('should create empty node above when cursor is right after header syntax', async () => {
      const context = createContext({
        key: 'Enter',
        content: '# Header Text',
        cursorPosition: 2 // Right after '# '
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: '# Header Text',
          newContent: '',
          insertAtBeginning: true,
          focusOriginalNode: true
        })
      );
    });

    it('should split normally when cursor is after header syntax', async () => {
      const context = createContext({
        key: 'Enter',
        content: '# Header Text',
        cursorPosition: 9 // After 'Header'
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: '# Header ', // Includes the space after 'Header'
          newContent: '# Text',
          insertAtBeginning: false,
          newNodeCursorPosition: 2
        })
      );
    });
  });

  describe('execute - inline formatting', () => {
    it('should create empty node above when cursor is within bold syntax at start', async () => {
      const context = createContext({
        key: 'Enter',
        content: '**bold text**',
        cursorPosition: 1 // Within '**'
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: '**bold text**',
          newContent: '',
          insertAtBeginning: true,
          focusOriginalNode: true
        })
      );
    });

    it('should split with preserved formatting when cursor is mid-text', async () => {
      const context = createContext({
        key: 'Enter',
        content: '**bold text**',
        cursorPosition: 7 // After 'bold'
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: '**bold **', // Includes the space after 'bold'
          newContent: '**text**',
          insertAtBeginning: false,
          newNodeCursorPosition: 2
        })
      );
    });
  });

  describe('execute - event prevention', () => {
    it('should call preventDefault on the event', async () => {
      const preventDefaultSpy = vi.fn();
      const context = createContext({
        key: 'Enter',
        content: 'test',
        cursorPosition: 2,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
    });
  });

  describe('execute - multiline content handling', () => {
    it('should use convertHtmlToTextWithNewlines for multiline nodes', async () => {
      const convertSpy = vi.fn().mockReturnValue('Line 1\nLine 2');
      mockController.convertHtmlToTextWithNewlines = convertSpy;
      mockController.element = {
        innerHTML: '<div>Line 1</div><div>Line 2</div>',
        textContent: 'Line 1Line 2'
      } as unknown as TextareaController["element"];

      const context = createContext({
        key: 'Enter',
        content: 'Line 1Line 2',
        cursorPosition: 6
      });
      context.allowMultiline = true;

      await command.execute(context);

      expect(convertSpy).toHaveBeenCalledWith('<div>Line 1</div><div>Line 2</div>');
      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: expect.any(String),
          newContent: expect.any(String)
        })
      );
    });

    it('should fallback to context.content for multiline nodes without element', async () => {
      mockController.element = null as unknown as TextareaController["element"];

      const context = createContext({
        key: 'Enter',
        content: 'test content',
        cursorPosition: 4
      });
      context.allowMultiline = true;

      await command.execute(context);

      expect(createNewNodeSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          currentContent: 'test',
          newContent: ' content'
        })
      );
    });
  });

  // Helper function to create mock context
  function createContext(options: {
    key: string;
    shiftKey?: boolean;
    content?: string;
    cursorPosition?: number;
    preventDefault?: () => void;
  }): KeyboardContext {
    const event = {
      key: options.key,
      shiftKey: options.shiftKey || false,
      preventDefault: options.preventDefault || vi.fn()
    } as unknown as KeyboardEvent;

    // Mock getCursorPositionInMarkdown (used for markdown-aware splitting)
    mockController.getCursorPositionInMarkdown = vi.fn(() => options.cursorPosition || 0);

    return {
      event,
      controller: mockController as TextareaController,
      nodeId: 'test-node',
      nodeType: 'text',
      content: options.content || '',
      cursorPosition: options.cursorPosition || 0,
      selection: null,
      allowMultiline: false,
      paneId: 'default',
      metadata: {}
    };
  }
});
