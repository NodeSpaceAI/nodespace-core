/**
 * Unit tests for OutdentNodeCommand
 *
 * Tests the Shift+Tab key functionality for outdenting nodes
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OutdentNodeCommand } from '$lib/commands/keyboard/outdent-node.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('OutdentNodeCommand', () => {
  let command: OutdentNodeCommand;
  let mockController: Partial<TextareaController>;
  let outdentNodeSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    command = new OutdentNodeCommand();
    outdentNodeSpy = vi.fn();

    mockController = {
      events: {
        outdentNode: outdentNodeSpy
      }
    } as unknown as TextareaController;
  });

  describe('canExecute', () => {
    it('should execute for Tab key with Shift', () => {
      const context = createContext({ key: 'Tab', shiftKey: true });
      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for Tab key without Shift', () => {
      const context = createContext({ key: 'Tab', shiftKey: false });
      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute for non-Tab keys', () => {
      const context = createContext({ key: 'a', shiftKey: true });
      expect(command.canExecute(context)).toBe(false);
    });
  });

  describe('execute', () => {
    it('should call preventDefault on the event', async () => {
      const preventDefaultSpy = vi.fn();
      const context = createContext({
        key: 'Tab',
        shiftKey: true,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
    });

    it('should emit outdentNode event with correct nodeId', async () => {
      const context = createContext({ key: 'Tab', shiftKey: true });

      const result = await command.execute(context);

      expect(result).toBe(true);
      expect(outdentNodeSpy).toHaveBeenCalledWith({
        nodeId: 'test-node'
      });
    });

    it('should return true when executed', async () => {
      const context = createContext({ key: 'Tab', shiftKey: true });

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
      controller: mockController as TextareaController,
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
