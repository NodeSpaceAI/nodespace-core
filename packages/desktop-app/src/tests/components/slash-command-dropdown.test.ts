/**
 * SlashCommandDropdown Component Tests
 *
 * Comprehensive tests for the slash command dropdown component covering:
 * - Rendering states (visible, loading, empty, with commands)
 * - Keyboard navigation (ArrowUp, ArrowDown, Enter, Escape)
 * - Event handling and propagation
 * - Component lifecycle (event listener cleanup)
 * - Smart positioning logic
 * - Accessibility (ARIA attributes, tabindex management)
 * - Mouse interactions (click, hover, mouseenter, mousedown blur prevention)
 * - Edge cases and error conditions
 *
 * Total: 40 test cases covering all component functionality
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import SlashCommandDropdown from '$lib/components/ui/slash-command-dropdown/slash-command-dropdown.svelte';
import { createKeyboardEvent, getAriaAttributes } from '../helpers';
import type { SlashCommand } from '$lib/services/slash-command-service';
import { waitForEffects, MOCK_SLASH_COMMANDS } from '../helpers';

describe('SlashCommandDropdown', () => {
  // Use unified mock data
  const mockCommands = MOCK_SLASH_COMMANDS.slice(0, 3);

  const defaultPosition = { x: 100, y: 100 };

  // Event handlers
  let onSelect: ReturnType<typeof vi.fn>;
  let onClose: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    // Reset mocks before each test
    onSelect = vi.fn();
    onClose = vi.fn();
  });

  afterEach(() => {
    // Clean up document event listeners
    vi.clearAllMocks();
  });

  describe('Rendering - Basic States', () => {
    it('should not render when visible is false', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: false,
          position: defaultPosition,
          query: '',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.queryByRole('listbox');
      expect(listbox).not.toBeInTheDocument();
    });

    it('should render listbox when visible is true', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: '',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      expect(listbox).toBeInTheDocument();
      expect(listbox).toHaveAttribute('aria-label', 'Slash command palette');
    });

    it('should render loading state', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          commands: [],
          loading: true
        }
      });

      expect(screen.getByText('Loading commands...')).toBeInTheDocument();
    });
  });

  describe('Rendering - Empty and Commands States', () => {
    it('should render empty state with query', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'nonexistent',
          commands: [],
          loading: false
        }
      });

      expect(screen.getByText('No commands found matching')).toBeInTheDocument();
      expect(screen.getByText('nonexistent')).toBeInTheDocument();
    });

    it('should render empty state without query', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: '',
          commands: [],
          loading: false
        }
      });

      expect(screen.getByText('No commands available')).toBeInTheDocument();
      expect(screen.getByText('Try typing after the / character')).toBeInTheDocument();
    });

    it('should render commands list', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options).toHaveLength(3);
      expect(screen.getByText('Heading 1')).toBeInTheDocument();
      expect(screen.getByText('Heading 2')).toBeInTheDocument();
      expect(screen.getByText('Heading 3')).toBeInTheDocument();
    });
  });

  describe('Keyboard Navigation - Arrow Keys', () => {
    it('should select first item by default', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should move selection down with ArrowDown key', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Dispatch ArrowDown event
      const arrowDownEvent = createKeyboardEvent('ArrowDown');
      document.dispatchEvent(arrowDownEvent);

      await waitForEffects();

      const options = screen.getAllByRole('option');
      expect(options[0]).toHaveAttribute('aria-selected', 'false');
      expect(options[1]).toHaveAttribute('aria-selected', 'true');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should move selection up with ArrowUp key', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Move down first, then up
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();
      document.dispatchEvent(createKeyboardEvent('ArrowUp'));
      await waitForEffects();

      const options = screen.getAllByRole('option');
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
    });

    it('should not go below last item', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Try to go down 10 times (more than items available)
      for (let i = 0; i < 10; i++) {
        document.dispatchEvent(createKeyboardEvent('ArrowDown'));
        await waitForEffects();
      }

      const options = screen.getAllByRole('option');
      // Should stay at last item (index 2)
      expect(options[2]).toHaveAttribute('aria-selected', 'true');
      expect(options[0]).toHaveAttribute('aria-selected', 'false');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
    });

    it('should not go above first item', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Try to go up from first item
      document.dispatchEvent(createKeyboardEvent('ArrowUp'));
      await waitForEffects();

      const options = screen.getAllByRole('option');
      // Should stay at first item (index 0)
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
    });

    it('should reset selection when commands change', async () => {
      const result = render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Move selection to second item
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();

      // Update commands (simulating new search)
      const newCommands: SlashCommand[] = [
        {
          id: 'text',
          name: 'Text',
          description: 'Plain text node',
          nodeType: 'text',
          icon: 'text',
          headerLevel: 0
        }
      ];
      await result.rerender({ commands: newCommands });
      await waitForEffects();

      const options = screen.getAllByRole('option');
      // Selection should reset to first item
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
    });
  });

  describe('Keyboard Navigation - Enter and Escape', () => {
    it('should dispatch select event on Enter key', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect
        }
      });

      // Press Enter
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith(mockCommands[0]);
    });

    it('should select correct command when Enter is pressed after navigation', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect
        }
      });

      // Navigate to second item
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();

      // Press Enter
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      expect(onSelect).toHaveBeenCalledWith(mockCommands[1]);
    });

    it('should dispatch close event on Escape key', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onclose: onClose
        }
      });

      // Press Escape
      document.dispatchEvent(createKeyboardEvent('Escape'));
      await waitForEffects();

      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it('should not handle keyboard events when not visible', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: false,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect,
          onclose: onClose
        }
      });

      // Try pressing Enter and Escape
      document.dispatchEvent(createKeyboardEvent('Enter'));
      document.dispatchEvent(createKeyboardEvent('Escape'));
      await waitForEffects();

      expect(onSelect).not.toHaveBeenCalled();
      expect(onClose).not.toHaveBeenCalled();
    });

    it('should not handle keyboard events when commands are empty', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          commands: [],
          loading: false,
          onselect: onSelect
        }
      });

      // Try navigation and selection with empty commands
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      // onSelect should not be called because there are no commands
      expect(onSelect).not.toHaveBeenCalled();
    });
  });

  describe('Event Propagation - stopPropagation Verification', () => {
    it('should call stopPropagation on ArrowDown event', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const stopPropagationSpy = vi.fn();
      const arrowDownEvent = createKeyboardEvent('ArrowDown');
      arrowDownEvent.stopPropagation = stopPropagationSpy;

      document.dispatchEvent(arrowDownEvent);
      await waitForEffects();

      expect(stopPropagationSpy).toHaveBeenCalled();
    });

    it('should call stopPropagation on ArrowUp event', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const stopPropagationSpy = vi.fn();
      const arrowUpEvent = createKeyboardEvent('ArrowUp');
      arrowUpEvent.stopPropagation = stopPropagationSpy;

      document.dispatchEvent(arrowUpEvent);
      await waitForEffects();

      expect(stopPropagationSpy).toHaveBeenCalled();
    });

    it('should call stopPropagation on Enter event', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const stopPropagationSpy = vi.fn();
      const enterEvent = createKeyboardEvent('Enter');
      enterEvent.stopPropagation = stopPropagationSpy;

      document.dispatchEvent(enterEvent);
      await waitForEffects();

      expect(stopPropagationSpy).toHaveBeenCalled();
    });

    it('should call stopPropagation on Escape event', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const stopPropagationSpy = vi.fn();
      const escapeEvent = createKeyboardEvent('Escape');
      escapeEvent.stopPropagation = stopPropagationSpy;

      document.dispatchEvent(escapeEvent);
      await waitForEffects();

      expect(stopPropagationSpy).toHaveBeenCalled();
    });

    it('should call preventDefault on all handled keyboard events', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const keys = ['ArrowDown', 'ArrowUp', 'Enter', 'Escape'];

      for (const key of keys) {
        const preventDefaultSpy = vi.fn();
        const event = createKeyboardEvent(key);
        event.preventDefault = preventDefaultSpy;

        document.dispatchEvent(event);
        await waitForEffects();

        expect(preventDefaultSpy).toHaveBeenCalled();
      }
    });
  });

  describe('Component Lifecycle - Event Listener Cleanup', () => {
    it('should add keydown event listener on mount', () => {
      const addEventListenerSpy = vi.spyOn(document, 'addEventListener');

      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      expect(addEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });

    it('should remove keydown event listener on unmount', () => {
      const removeEventListenerSpy = vi.spyOn(document, 'removeEventListener');

      const result = render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      result.unmount();

      expect(removeEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });

    it('should not respond to keyboard events after unmount', async () => {
      const { unmount } = render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect
        }
      });

      // Unmount component
      unmount();

      // Try to trigger keyboard events
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      // Should not call onSelect because component is unmounted
      expect(onSelect).not.toHaveBeenCalled();
    });
  });

  describe('Smart Positioning - Viewport Edge Detection', () => {
    beforeEach(() => {
      // Mock window dimensions for viewport calculations
      Object.defineProperty(window, 'innerWidth', {
        writable: true,
        configurable: true,
        value: 1024
      });
      Object.defineProperty(window, 'innerHeight', {
        writable: true,
        configurable: true,
        value: 768
      });
    });

    it('should position below cursor when space available', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: { x: 100, y: 100 }, // Plenty of space below
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const style = listbox.style;

      // Should use top positioning (positioned below cursor)
      expect(style.top).toBeTruthy();
      expect(style.bottom).toBeFalsy();

      // Should be offset from cursor position by spacingFromCursor (20px)
      const topValue = parseInt(style.top);
      expect(topValue).toBeGreaterThan(100); // Below cursor Y position
      expect(topValue).toBeLessThanOrEqual(120); // Within spacing range
    });

    it('should position above cursor when insufficient space below', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: { x: 100, y: 700 }, // Near bottom, insufficient space below
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const style = listbox.style;

      // Should use bottom positioning (positioned above cursor)
      expect(style.bottom).toBeTruthy();
      expect(style.top).toBeFalsy();

      // Bottom value should position modal above cursor
      const bottomValue = parseInt(style.bottom);
      expect(bottomValue).toBeGreaterThan(0);
    });

    it('should prevent horizontal viewport overflow', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: { x: 1000, y: 100 }, // Near right edge
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const style = listbox.style;
      const leftValue = parseInt(style.left);

      // Component min-width is 300px, padding is 16px
      // Left position should be adjusted to keep modal within viewport
      expect(leftValue + 300).toBeLessThanOrEqual(1024 + 16); // Within viewport with padding tolerance
    });
  });

  describe('Edge Cases', () => {
    it('should handle single command', () => {
      const singleCommand: SlashCommand[] = [
        {
          id: 'text',
          name: 'Text',
          description: 'Plain text',
          nodeType: 'text',
          icon: 'text',
          headerLevel: 0
        }
      ];

      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'text',
          commands: singleCommand,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options).toHaveLength(1);
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
    });

    it('should handle commands with missing optional fields', () => {
      const minimalCommands: SlashCommand[] = [
        {
          id: 'minimal',
          name: 'Minimal Command',
          description: 'No optional fields',
          nodeType: 'text',
          icon: 'text'
          // No shortcut or headerLevel
        }
      ];

      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'minimal',
          commands: minimalCommands,
          loading: false
        }
      });

      expect(screen.getByText('Minimal Command')).toBeInTheDocument();
    });

    it('should handle transition from loading to commands', async () => {
      const result = render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: [],
          loading: true
        }
      });

      expect(screen.getByText('Loading commands...')).toBeInTheDocument();

      // Transition to commands
      await result.rerender({ loading: false, commands: mockCommands });
      await waitForEffects();

      expect(screen.queryByText('Loading commands...')).not.toBeInTheDocument();
      expect(screen.getByText('Heading 1')).toBeInTheDocument();
    });

    it('should handle rapid keyboard events', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Dispatch rapid sequential keyboard events
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      document.dispatchEvent(createKeyboardEvent('ArrowUp'));
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));

      await waitForEffects();

      const options = screen.getAllByRole('option');

      // Final state: should be at third item (0 → 1 → 2 → 1 → 2)
      expect(options[0]).toHaveAttribute('aria-selected', 'false');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
      expect(options[2]).toHaveAttribute('aria-selected', 'true');
    });
  });

  describe('Accessibility - ARIA Attributes', () => {
    it('should have correct role on container', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const aria = getAriaAttributes(listbox);

      expect(aria.role).toBe('listbox');
      expect(aria.label).toBe('Slash command palette');
    });

    it('should mark selected option with aria-selected', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');

      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should update aria-selected when selection changes', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      // Move to second option
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();

      const options = screen.getAllByRole('option');

      expect(options[0]).toHaveAttribute('aria-selected', 'false');
      expect(options[1]).toHaveAttribute('aria-selected', 'true');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should set tabindex correctly for keyboard navigation', () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');

      // Selected option should be focusable
      expect(options[0]).toHaveAttribute('tabindex', '0');
      // Other options should not be in tab order
      expect(options[1]).toHaveAttribute('tabindex', '-1');
      expect(options[2]).toHaveAttribute('tabindex', '-1');
    });
  });

  describe('Mouse Interactions', () => {
    it('should select command on click', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect
        }
      });

      const options = screen.getAllByRole('option');
      options[1].click();

      await waitForEffects();

      expect(onSelect).toHaveBeenCalledWith(mockCommands[1]);
    });

    it('should prevent default on mousedown to avoid blur event', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false,
          onselect: onSelect
        }
      });

      const options = screen.getAllByRole('option');

      // Create a mousedown event with preventDefault spy
      const preventDefaultSpy = vi.fn();
      const stopPropagationSpy = vi.fn();
      const mousedownEvent = new MouseEvent('mousedown', {
        bubbles: true,
        cancelable: true
      });
      mousedownEvent.preventDefault = preventDefaultSpy;
      mousedownEvent.stopPropagation = stopPropagationSpy;

      options[1].dispatchEvent(mousedownEvent);
      await waitForEffects();

      // Verify preventDefault was called to prevent blur
      expect(preventDefaultSpy).toHaveBeenCalled();
      expect(stopPropagationSpy).toHaveBeenCalled();
    });

    it('should update selection on hover', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');

      // Hover over second option
      options[1].dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
      await waitForEffects();

      expect(options[0]).toHaveAttribute('aria-selected', 'false');
      expect(options[1]).toHaveAttribute('aria-selected', 'true');
    });

    it('should only update selection if different on mouseenter', async () => {
      render(SlashCommandDropdown, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'heading',
          commands: mockCommands,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');

      // First option is already selected, mouseenter should not cause unnecessary updates
      options[0].dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
      await waitForEffects();

      expect(options[0]).toHaveAttribute('aria-selected', 'true');
    });
  });
});
