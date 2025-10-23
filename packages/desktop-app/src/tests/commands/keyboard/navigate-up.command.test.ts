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
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('NavigateUpCommand', () => {
  // Note: Type casts for mock methods are required because Vitest's vi.fn() returns
  // a generic Mock type that doesn't include mockReturnValue in the type signature.
  // The cast to ReturnType<typeof vi.fn> provides the correct typing at runtime.

  let command: NavigateUpCommand;
  let mockController: TextareaController;
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
    } as unknown as TextareaController;
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
      (mockController.isAtFirstLine as ReturnType<typeof vi.fn>).mockReturnValue(true);

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
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as unknown as Selection);

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
      (mockController.getCurrentPixelOffset as ReturnType<typeof vi.fn>).mockReturnValue(250);

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

  describe('Edge Cases - Blank Line Navigation (Regression Tests)', () => {
    it('should allow navigation through leading blank DIV when cursor in second DIV (ELEMENT_NODE)', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Create scenario: <div><br></div><div>Text</div>
      // Cursor is at start of second DIV (ELEMENT_NODE case)
      const div1 = document.createElement('div');
      const br = document.createElement('br');
      div1.appendChild(br);
      const div2 = document.createElement('div');
      div2.textContent = 'Text';
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);

      // Mock selection where cursor is in the DIV itself (ELEMENT_NODE), not in text node
      const mockRange = {
        startContainer: div2, // ELEMENT_NODE, not TEXT_NODE
        startOffset: 0,
        cloneRange: vi.fn().mockReturnValue({
          selectNodeContents: vi.fn(),
          setEnd: vi.fn(),
          toString: vi.fn().mockReturnValue('\n') // Content before cursor (empty div = newline)
        })
      };
      const mockSelection = {
        rangeCount: 1,
        getRangeAt: vi.fn().mockReturnValue(mockRange)
      };
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as unknown as Selection);

      // Should return false to let browser handle navigation to previous DIV
      expect(command.canExecute(context)).toBe(false);
    });

    it('should allow navigation through leading blank DIV when cursor at start of text in second DIV', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Create scenario: <div><br></div><div>Text</div>
      // Cursor is at start of "Text" (TEXT_NODE case, offset 0)
      const div1 = document.createElement('div');
      const br = document.createElement('br');
      div1.appendChild(br);
      const div2 = document.createElement('div');
      const textNode = document.createTextNode('Text');
      div2.appendChild(textNode);
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);

      // Mock selection where cursor is in text node at offset 0
      const mockRange = {
        startContainer: textNode, // TEXT_NODE
        startOffset: 0,
        cloneRange: vi.fn().mockReturnValue({
          selectNodeContents: vi.fn(),
          setEnd: vi.fn(),
          toString: vi.fn().mockReturnValue('\n') // Content before cursor
        })
      };
      const mockSelection = {
        rangeCount: 1,
        getRangeAt: vi.fn().mockReturnValue(mockRange)
      };
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as unknown as Selection);

      // Should return false to let browser handle navigation to previous DIV
      expect(command.canExecute(context)).toBe(false);
    });

    it('should navigate to previous node when at absolute start (first DIV)', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Create scenario: <div>Text</div><div>More</div>
      // Cursor is at start of first DIV
      const div1 = document.createElement('div');
      const textNode = document.createTextNode('Text');
      div1.appendChild(textNode);
      const div2 = document.createElement('div');
      div2.textContent = 'More';
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);

      // Mock selection where cursor is at start of first DIV's text
      const mockRange = {
        startContainer: textNode,
        startOffset: 0,
        cloneRange: vi.fn().mockReturnValue({
          selectNodeContents: vi.fn(),
          setEnd: vi.fn(),
          toString: vi.fn().mockReturnValue('') // No content before cursor
        })
      };
      const mockSelection = {
        rangeCount: 1,
        getRangeAt: vi.fn().mockReturnValue(mockRange)
      };
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as unknown as Selection);

      // Should return true to navigate to previous node
      expect(command.canExecute(context)).toBe(true);
    });

    it('should handle trailing blank DIVs correctly when navigating up', () => {
      const context = createContext({
        key: 'ArrowUp',
        allowMultiline: true
      });

      // Create scenario: <div>Text</div><div><br></div><div><br></div>
      // Cursor is in last DIV (trailing blank)
      const div1 = document.createElement('div');
      div1.textContent = 'Text';
      const div2 = document.createElement('div');
      div2.innerHTML = '<br>';
      const div3 = document.createElement('div');
      div3.innerHTML = '<br>';
      mockController.element?.appendChild(div1);
      mockController.element?.appendChild(div2);
      mockController.element?.appendChild(div3);

      // Mock selection where cursor is in third DIV
      const mockRange = {
        startContainer: div3,
        startOffset: 0,
        cloneRange: vi.fn().mockReturnValue({
          selectNodeContents: vi.fn(),
          setEnd: vi.fn(),
          toString: vi.fn().mockReturnValue('Text\n\n') // Content before cursor
        })
      };
      const mockSelection = {
        rangeCount: 1,
        getRangeAt: vi.fn().mockReturnValue(mockRange)
      };
      vi.spyOn(window, 'getSelection').mockReturnValue(mockSelection as unknown as Selection);

      // Should return false to let browser handle navigation to previous DIV
      expect(command.canExecute(context)).toBe(false);
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
