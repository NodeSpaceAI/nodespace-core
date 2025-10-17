/**
 * Unit tests for NavigateDownCommand
 *
 * Tests the ArrowDown key navigation functionality including:
 * - canExecute conditions (multiline vs single-line)
 * - Boundary detection (last line vs beginning/middle lines)
 * - Event prevention and proper event emission
 * - Pixel offset calculation for cursor positioning
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NavigateDownCommand } from '$lib/commands/keyboard/navigate-down.command';
import type { KeyboardContext } from '$lib/services/keyboard-command-registry';
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('NavigateDownCommand', () => {
  let command: NavigateDownCommand;
  let mockController: TextareaController;
  let navigateArrowSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    command = new NavigateDownCommand();
    navigateArrowSpy = vi.fn();

    mockController = {
      events: {
        navigateArrow: navigateArrowSpy
      },
      element: document.createElement('div'),
      justCreated: false,
      slashCommandDropdownActive: false,
      autocompleteDropdownActive: false,
      isAtLastLine: vi.fn(() => true),
      getCurrentPixelOffset: vi.fn(() => 100)
    } as unknown as TextareaController;
  });

  describe('canExecute', () => {
    it('should execute for ArrowDown key on single-line node', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(true);
    });

    it('should execute for ArrowDown key on multiline node at last line', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: true
      });

      // Mock that we're at the last line
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.isAtLastLine.mockReturnValue(true);

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for ArrowDown when not at last line in multiline node with multiple lines', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: true
      });

      // Mock that node has multiple lines (DIVs exist)
      const div1 = document.createElement('div');
      const div2 = document.createElement('div');
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);

      // Mock that we're NOT at last line
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.isAtLastLine.mockReturnValue(false);

      expect(command.canExecute(context)).toBe(false);
    });

    it('should execute for ArrowDown on multiline-capable node with single line (no DIVs)', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: true
      });

      // No DIVs in element - multiline-capable but currently single line
      expect(mockController.element?.children.length).toBe(0);

      expect(command.canExecute(context)).toBe(true);
    });

    it('should not execute for non-ArrowDown keys', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: false
      });

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when node was just created', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      mockController.justCreated = true;

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when slash command dropdown is active', () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      mockController.slashCommandDropdownActive = true;

      expect(command.canExecute(context)).toBe(false);
    });

    it('should not execute when autocomplete dropdown is active', () => {
      const context = createContext({
        key: 'ArrowDown',
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
        key: 'ArrowDown',
        allowMultiline: false,
        preventDefault: preventDefaultSpy
      });

      await command.execute(context);

      expect(preventDefaultSpy).toHaveBeenCalled();
    });

    it('should emit navigateArrow event with correct direction', async () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      await command.execute(context);

      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'down',
        pixelOffset: 100
      });
    });

    it('should calculate pixel offset for cursor positioning', async () => {
      const context = createContext({
        key: 'ArrowDown',
        allowMultiline: false
      });

      // Mock different pixel offset
      // @ts-expect-error - vi.fn() creates a mock with mockReturnValue
      mockController.getCurrentPixelOffset.mockReturnValue(250);

      await command.execute(context);

      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'down',
        pixelOffset: 250
      });
    });

    it('should return true when executed successfully', async () => {
      const context = createContext({
        key: 'ArrowDown',
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
      controller: mockController as TextareaController,
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
