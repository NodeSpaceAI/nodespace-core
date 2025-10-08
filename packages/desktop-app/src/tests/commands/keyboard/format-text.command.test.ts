/**
 * Unit tests for FormatTextCommand
 *
 * Tests text formatting shortcuts including:
 * - canExecute conditions (correct modifier keys)
 * - Format type detection (bold, italic, underline)
 * - Cross-platform support (Cmd on Mac, Ctrl on Windows/Linux)
 * - Event prevention and toggleFormatting invocation
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { FormatTextCommand } from '$lib/commands/keyboard/format-text.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { ContentEditableControllerExtended } from '$lib/services/keyboard-command-registry';

describe('FormatTextCommand', () => {
  let mockController: Partial<ContentEditableControllerExtended>;
  let toggleFormattingSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    toggleFormattingSpy = vi.fn();

    mockController = {
      isEditing: true,
      toggleFormatting: toggleFormattingSpy
    } as unknown as ContentEditableControllerExtended;
  });

  describe('Bold formatting', () => {
    let command: FormatTextCommand;

    beforeEach(() => {
      command = new FormatTextCommand('bold');
    });

    it('should have correct id and description', () => {
      expect(command.id).toBe('format-text-bold');
      expect(command.description).toBe('Toggle bold formatting');
    });

    it('should execute for Cmd+B on Mac', () => {
      const context = createContext({
        key: 'b',
        metaKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should execute for Ctrl+B on Windows/Linux', () => {
      const context = createContext({
        key: 'b',
        ctrlKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should be case-insensitive for key', () => {
      const context = createContext({
        key: 'B',
        metaKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute without modifier key', () => {
      const context = createContext({
        key: 'b'
      });

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute for wrong key', () => {
      const context = createContext({
        key: 'i',
        metaKey: true
      });

      expect(command.canExecute(context)).toBe(false);
    });

    it('should call toggleFormatting with ** marker', async () => {
      const preventDefaultSpy = vi.fn();
      const context = createContext({
        key: 'b',
        metaKey: true,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
      expect(toggleFormattingSpy).toHaveBeenCalledWith('**');
    });

    it('should return true when executed successfully', async () => {
      const context = createContext({
        key: 'b',
        metaKey: true
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
    });

    it('should return false when controller is not editing', async () => {
      mockController.isEditing = false;

      const context = createContext({
        key: 'b',
        metaKey: true
      });

      const result = await command.execute(context);

      expect(result).toBe(false);
      expect(toggleFormattingSpy).not.toHaveBeenCalled();
    });
  });

  describe('Italic formatting', () => {
    let command: FormatTextCommand;

    beforeEach(() => {
      command = new FormatTextCommand('italic');
    });

    it('should have correct id and description', () => {
      expect(command.id).toBe('format-text-italic');
      expect(command.description).toBe('Toggle italic formatting');
    });

    it('should execute for Cmd+I on Mac', () => {
      const context = createContext({
        key: 'i',
        metaKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should execute for Ctrl+I on Windows/Linux', () => {
      const context = createContext({
        key: 'i',
        ctrlKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should call toggleFormatting with * marker', async () => {
      const context = createContext({
        key: 'i',
        metaKey: true
      });

      await command.execute(context);

      expect(toggleFormattingSpy).toHaveBeenCalledWith('*');
    });
  });

  describe('Underline formatting', () => {
    let command: FormatTextCommand;

    beforeEach(() => {
      command = new FormatTextCommand('underline');
    });

    it('should have correct id and description', () => {
      expect(command.id).toBe('format-text-underline');
      expect(command.description).toBe('Toggle underline formatting');
    });

    it('should execute for Cmd+U on Mac', () => {
      const context = createContext({
        key: 'u',
        metaKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should execute for Ctrl+U on Windows/Linux', () => {
      const context = createContext({
        key: 'u',
        ctrlKey: true
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should call toggleFormatting with __ marker', async () => {
      const context = createContext({
        key: 'u',
        metaKey: true
      });

      await command.execute(context);

      expect(toggleFormattingSpy).toHaveBeenCalledWith('__');
    });
  });

  // Helper function to create mock context
  function createContext(options: {
    key: string;
    metaKey?: boolean;
    ctrlKey?: boolean;
    preventDefault?: () => void;
  }): KeyboardContext {
    const event = {
      key: options.key,
      preventDefault: options.preventDefault || vi.fn(),
      ctrlKey: options.ctrlKey ?? false,
      altKey: false,
      shiftKey: false,
      metaKey: options.metaKey ?? false
    } as unknown as KeyboardEvent;

    return {
      event,
      controller: mockController as ContentEditableControllerExtended,
      nodeId: 'test-node',
      nodeType: 'text',
      content: '',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: {}
    };
  }
});
