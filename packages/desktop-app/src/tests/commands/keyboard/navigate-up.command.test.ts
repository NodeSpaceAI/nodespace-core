/**
 * Unit tests for NavigateUpCommand
 *
 * Tests the ArrowUp key navigation functionality including:
 * - canExecute conditions (multiline vs single-line)
 * - Boundary detection (first line vs middle/last lines)
 * - Event prevention and proper event emission
 * - Pixel offset calculation for cursor positioning
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NavigateUpCommand } from '$lib/commands/keyboard/navigate-up.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { ContentEditableControllerExtended } from '$lib/services/keyboard-command-registry';

describe('NavigateUpCommand', () => {
  let command: NavigateUpCommand;
  let mockController: ContentEditableControllerExtended;
  let navigateArrowSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    command = new NavigateUpCommand();
    navigateArrowSpy = vi.fn();

    mockController = {
      events: {
        navigateArrow: navigateArrowSpy
      },
      element: document.createElement('div'),
      justCreated: false,
      slashCommandDropdownActive: false,
      autocompleteDropdownActive: false,
      isAtFirstLine: vi.fn(() => true),
      getCurrentPixelOffset: vi.fn(() => 100)
    } as unknown as ContentEditableControllerExtended;
  });

  describe('canExecute', () => {
    it('should execute for ArrowUp key on single-line node', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should execute for ArrowUp key on multiline node at first line', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Mock that we're at the first line
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.isAtFirstLine.mockReturnValue(true);

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for ArrowUp when not at absolute start in multiline node with multiple lines', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Mock that node has multiple lines (DIVs exist)
      const div1 = document.createElement('div');
      div1.textContent = 'First line';
      const div2 = document.createElement('div');
      div2.textContent = 'Second line';
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);

      // Mock window.getSelection to indicate cursor is in second DIV (not at absolute start)
      const mockRange = {
        startContainer: div2.firstChild,
        startOffset: 0,
        cloneRange: vi.fn().mockReturnValue({
          selectNodeContents: vi.fn(),
          setEnd: vi.fn(),
          toString: vi.fn().mockReturnValue('First line\n') // Content before cursor
        })
      };
      const mockSelection = {
        rangeCount: 1,
        getRangeAt: vi.fn().mockReturnValue(mockRange)
      };
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as any);

      expect(command.canExecute(context)).toBe(false);
    });

    it('should execute for ArrowUp on multiline-capable node with single line (no DIVs)', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // No DIVs in element - multiline-capable but currently single line
      expect(mockController.element?.children.length).toBe(0);

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for non-ArrowUp keys', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when node was just created', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      mockController.justCreated = true;

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when slash command dropdown is active', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      mockController.slashCommandDropdownActive = true;

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when autocomplete dropdown is active', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      mockController.autocompleteDropdownActive = true;

      expect(command.canExecute(context)).toBe(false);
    });
  });

  describe('execute', () => {
    it('should prevent default event behavior', async () => {
      const preventDefaultSpy = vi.fn();
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
    });

    it('should emit navigateArrow event with correct direction', async () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      await command.execute(context);

      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'up',
        pixelOffset: 100
      });
    });

    it('should calculate pixel offset for cursor positioning', async () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      // Mock different pixel offset
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.getCurrentPixelOffset.mockReturnValue(250);

      await command.execute(context);

      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'up',
        pixelOffset: 250
      });
    });

    it('should return true when executed successfully', async () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      const result = await command.execute(context);

      expect(result).toBe(true);
    });
  });

  // Helper function to create mock context
  function createContext(options: {
    key: string;
    allowMultiline?: boolean;
    preventDefault?: () => void;
  }): KeyboardContext {
    const event = {
      key: options.key,
      preventDefault: options.preventDefault || vi.fn(),
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      metaKey: false
    } as unknown as KeyboardEvent;

    return {
      event,
      controller: mockController as ContentEditableControllerExtended,
      nodeId: 'test-node',
      nodeType: 'text',
      content: '',
      cursorPosition: 0,
      selection: null,
      allowMultiline: options.allowMultiline ?? false,
      metadata: {}
    };
  }
});
