/**
 * Unit tests for IndentNodeCommand
 *
 * Tests the Tab key functionality for indenting nodes
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { IndentNodeCommand } from '$lib/commands/keyboard/indent-node.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { ContentEditableControllerExtended } from '$lib/services/keyboard-command-registry';

describe('IndentNodeCommand', () => {
  let command: IndentNodeCommand;
  let mockController: Partial<ContentEditableControllerExtended>;
  let indentNodeSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    command = new IndentNodeCommand();
    indentNodeSpy = vi.fn();

    mockController = {
      events: {
        indentNode: indentNodeSpy
      }
    } as unknown as ContentEditableControllerExtended;
  });

  describe('canExecute', () => {
    it('should execute for Tab key without Shift', () => {
      const context = createContext({ key: 'Tab', shiftKey: false });
      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for Tab key with Shift', () => {
      const context = createContext({ key: 'Tab', shiftKey: true });
      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute for non-Tab keys', () => {
      const context = createContext({ key: 'a', shiftKey: false });
      expect(command.canExecute(context)).toBe(false);
    });
  });

  describe('execute', () => {
    it('should call preventDefault on the event', async () => {
      const preventDefaultSpy = vi.fn();
      const context = createContext({
        key: 'Tab',
        shiftKey: false,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
    });

    it('should emit indentNode event with correct nodeId', async () => {
      const context = createContext({ key: 'Tab', shiftKey: false });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(indentNodeSpy).toHaveBeenCalledWith({
        nodeId: 'test-node'
      });
    });

    it('should return true when executed', async () => {
      const context = createContext({ key: 'Tab', shiftKey: false });

      const result = await command.execute(context);

      expect(result).toBe(true);
    });
  });

  // Helper function to create mock context
  function createContext(options: {
    key: string;
    shiftKey: boolean;
    preventDefault?: () => void;
  }): KeyboardContext {
    const event = {
      key: options.key,
      shiftKey: options.shiftKey,
      preventDefault: options.preventDefault || vi.fn()
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
