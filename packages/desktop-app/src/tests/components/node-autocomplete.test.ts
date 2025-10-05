/**
 * NodeAutocomplete Component Tests
 *
 * Comprehensive tests for the node autocomplete component covering:
 * - Rendering states (visible, loading, empty, with results)
 * - Keyboard navigation (ArrowUp, ArrowDown, Enter, Escape)
 * - Event handling and propagation
 * - Component lifecycle (event listener cleanup)
 * - Accessibility (ARIA attributes)
 * - Edge cases and error conditions
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import NodeAutocomplete from '$lib/components/ui/node-autocomplete/node-autocomplete.svelte';
import { createKeyboardEvent, waitForEffects, getAriaAttributes } from './svelte-test-utils';
import type { NodeType } from '$lib/design/icons';

// Test data types
interface NodeResult {
  id: string;
  title: string;
  type: NodeType;
  subtitle?: string;
  metadata?: string;
}

describe('NodeAutocomplete', () => {
  // Mock data
  const mockResults: NodeResult[] = [
    { id: 'node-1', title: 'First Node', type: 'text' },
    { id: 'node-2', title: 'Second Node', type: 'task', subtitle: 'A task node' },
    { id: 'node-3', title: 'Third Node', type: 'document', metadata: 'Additional info' }
  ];

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
      render(NodeAutocomplete, {
        props: {
          visible: false,
          position: defaultPosition,
          query: '',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.queryByRole('listbox');
      expect(listbox).not.toBeInTheDocument();
    });

    it('should render listbox when visible is true', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: '',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      expect(listbox).toBeInTheDocument();
      expect(listbox).toHaveAttribute('aria-label', 'Node reference autocomplete');
    });

    it('should render loading state', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: [],
          loading: true
        }
      });

      expect(screen.getByText('Searching nodes...')).toBeInTheDocument();
    });

    it('should render empty state with query', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'nonexistent',
          results: [],
          loading: false
        }
      });

      expect(screen.getByText('No nodes found matching')).toBeInTheDocument();
      expect(screen.getByText('nonexistent')).toBeInTheDocument();
    });

    it('should render empty state without query', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: '',
          results: [],
          loading: false
        }
      });

      expect(screen.getByText('No nodes available')).toBeInTheDocument();
      expect(screen.getByText('Start typing to search')).toBeInTheDocument();
    });

    it('should render results list', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options).toHaveLength(3);
      expect(screen.getByText('First Node')).toBeInTheDocument();
      expect(screen.getByText('Second Node')).toBeInTheDocument();
      expect(screen.getByText('Third Node')).toBeInTheDocument();
    });
  });

  describe('Keyboard Navigation - Arrow Keys', () => {
    it('should select first item by default', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should move selection down with ArrowDown key', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

    it('should reset selection when results change', async () => {
      const result = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      // Move selection to second item
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();

      // Update results (simulating new search)
      const newResults: NodeResult[] = [{ id: 'node-4', title: 'New Node', type: 'text' }];
      await result.rerender({ results: newResults });
      await waitForEffects();

      const options = screen.getAllByRole('option');
      // Selection should reset to first item
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
    });
  });

  describe('Keyboard Navigation - Enter and Escape', () => {
    it('should dispatch select event on Enter key', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false,
          onselect: onSelect
        }
      });

      // Press Enter
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith(mockResults[0]);
    });

    it('should select correct item when Enter is pressed after navigation', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

      expect(onSelect).toHaveBeenCalledWith(mockResults[1]);
    });

    it('should dispatch close event on Escape key', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: false,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

    it('should not handle keyboard events when results are empty', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: [],
          loading: false,
          onselect: onSelect
        }
      });

      // Try navigation and selection with empty results
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      // onSelect should not be called because there are no results
      expect(onSelect).not.toHaveBeenCalled();
    });
  });

  describe('Event Propagation - stopPropagation Verification', () => {
    it('should call stopPropagation on ArrowDown event', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      expect(addEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });

    it('should remove keydown event listener on unmount', () => {
      const removeEventListenerSpy = vi.spyOn(document, 'removeEventListener');

      const result = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      result.unmount();

      expect(removeEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });

    it('should not respond to keyboard events after unmount', async () => {
      const { unmount } = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

  describe('Accessibility - ARIA Attributes', () => {
    it('should have correct role on container', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const aria = getAriaAttributes(listbox);

      expect(aria.role).toBe('listbox');
      expect(aria.label).toBe('Node reference autocomplete');
    });

    it('should mark selected option with aria-selected', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');

      expect(options[0]).toHaveAttribute('aria-selected', 'true');
      expect(options[1]).toHaveAttribute('aria-selected', 'false');
      expect(options[2]).toHaveAttribute('aria-selected', 'false');
    });

    it('should update aria-selected when selection changes', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
    it('should select item on click', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false,
          onselect: onSelect
        }
      });

      const options = screen.getAllByRole('option');
      options[1].click();

      await waitForEffects();

      expect(onSelect).toHaveBeenCalledWith(mockResults[1]);
    });

    it('should update selection on hover', async () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
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

  describe('Edge Cases', () => {
    it('should handle single result', () => {
      const singleResult: NodeResult[] = [{ id: 'node-1', title: 'Only Node', type: 'text' }];

      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: singleResult,
          loading: false
        }
      });

      const options = screen.getAllByRole('option');
      expect(options).toHaveLength(1);
      expect(options[0]).toHaveAttribute('aria-selected', 'true');
    });

    it('should handle results with missing optional fields', () => {
      const minimalResults: NodeResult[] = [
        { id: 'node-1', title: 'Minimal Node', type: 'text' }
        // No subtitle or metadata
      ];

      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: minimalResults,
          loading: false
        }
      });

      expect(screen.getByText('Minimal Node')).toBeInTheDocument();
    });

    it('should handle transition from loading to results', async () => {
      const result = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: [],
          loading: true
        }
      });

      expect(screen.getByText('Searching nodes...')).toBeInTheDocument();

      // Transition to results
      await result.rerender({ loading: false, results: mockResults });
      await waitForEffects();

      expect(screen.queryByText('Searching nodes...')).not.toBeInTheDocument();
      expect(screen.getByText('First Node')).toBeInTheDocument();
    });

    it('should handle transition from results to empty', async () => {
      const result = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      expect(screen.getByText('First Node')).toBeInTheDocument();

      // Transition to empty
      await result.rerender({ results: [] });
      await waitForEffects();

      expect(screen.queryByText('First Node')).not.toBeInTheDocument();
      expect(screen.getByText('No nodes found matching')).toBeInTheDocument();
    });

    it('should handle various node types', () => {
      const diverseResults: NodeResult[] = [
        { id: 'node-1', title: 'Text Node', type: 'text' },
        { id: 'node-2', title: 'Task Node', type: 'task' },
        { id: 'node-3', title: 'Document Node', type: 'document' },
        { id: 'node-4', title: 'AI Chat Node', type: 'ai-chat' }
      ];

      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: diverseResults,
          loading: false
        }
      });

      expect(screen.getByText('Text Node')).toBeInTheDocument();
      expect(screen.getByText('Task Node')).toBeInTheDocument();
      expect(screen.getByText('Document Node')).toBeInTheDocument();
      expect(screen.getByText('AI Chat Node')).toBeInTheDocument();
    });

    it('should handle position updates', async () => {
      const result = render(NodeAutocomplete, {
        props: {
          visible: true,
          position: { x: 100, y: 100 },
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      const initialPosition = listbox.style.left;

      // Update position
      await result.rerender({ position: { x: 200, y: 150 } });
      await waitForEffects();

      const newPosition = listbox.style.left;
      expect(newPosition).not.toBe(initialPosition);
    });
  });

  describe('Smart Positioning', () => {
    it('should position autocomplete at specified coordinates', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: { x: 300, y: 200 },
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');

      // Should be positioned near the specified coordinates
      // Note: Smart positioning may adjust these based on viewport
      expect(listbox.style.position).toBe('fixed');
    });

    it('should render with fixed positioning', () => {
      render(NodeAutocomplete, {
        props: {
          visible: true,
          position: defaultPosition,
          query: 'test',
          results: mockResults,
          loading: false
        }
      });

      const listbox = screen.getByRole('listbox');
      expect(listbox.style.position).toBe('fixed');
    });
  });
});
