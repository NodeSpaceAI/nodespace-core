/**
 * Unit tests for MergeNodesCommand
 *
 * Tests the Backspace/Delete key functionality for merging nodes
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { MergeNodesCommand } from '$lib/commands/keyboard/merge-nodes.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { ContentEditableControllerExtended } from '$lib/services/keyboard-command-registry';

describe('MergeNodesCommand', () => {
  let mockController: ContentEditableControllerExtended;
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
    } as unknown as ContentEditableControllerExtended;
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
      controller: mockController as ContentEditableControllerExtended,
      nodeId: 'test-node',
      nodeType: 'text',
      content: '',
      cursorPosition: options.cursorPosition,
      selection: window.getSelection(),
      allowMultiline: options.allowMultiline,
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
