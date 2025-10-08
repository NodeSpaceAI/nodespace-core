/**
 * Slash Command Integration Tests
 *
 * Tests the complete slash command flow from user interaction through
 * the dropdown component to the parent base-node component.
 *
 * This test suite exists because the bug in #187 was not caught by
 * component unit tests alone. Unit tests verified the dropdown worked
 * in isolation, but missed the integration boundary issue where
 * base-node was using event listeners (on:select) instead of callback
 * props (onselect).
 *
 * Testing Strategy:
 * We test the integration by verifying observable side effects rather
 * than listening to events, since BaseNode is a Svelte 5 component that
 * doesn't support $on(). The key indicator that the callback is working
 * is that the dropdown CLOSES after selection - if the callback wasn't
 * wired up, the dropdown would stay open.
 *
 * Related: Issue #187
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import BaseNode from '$lib/design/components/base-node.svelte';
import { createKeyboardEvent, waitForEffects } from '../helpers';

describe('Slash Command Integration - Regression Tests for #187', () => {
  beforeEach(() => {
    // Clean up DOM between tests
    document.body.innerHTML = '';
  });

  describe('Dropdown Display', () => {
    it('should display slash command dropdown when typing "/"', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');
      expect(contentEditableElement).toBeInTheDocument();

      // Type "/" to trigger slash command dropdown
      await user.click(contentEditableElement!);
      await user.keyboard('/');
      await waitForEffects();

      // Dropdown should be visible
      await waitFor(
        () => {
          const listbox = screen.queryByRole('listbox', { name: /slash command palette/i });
          expect(listbox).toBeInTheDocument();
        },
        { timeout: 2000 }
      );
    });

    it('should filter commands based on query text', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');
      expect(contentEditableElement).toBeInTheDocument();

      // Type "/task" to filter for task command
      await user.click(contentEditableElement!);
      await user.keyboard('/task');
      await waitForEffects();

      // Dropdown should be visible with filtered results
      await waitFor(() => {
        const listbox = screen.queryByRole('listbox');
        expect(listbox).toBeInTheDocument();
      });

      // Get visible options
      const options = screen.queryAllByRole('option');
      expect(options.length).toBeGreaterThan(0);

      // At least one option should contain "task" in its text
      const hasTaskOption = options.some((option) =>
        option.textContent?.toLowerCase().includes('task')
      );
      expect(hasTaskOption).toBe(true);
    });
  });

  describe('Callback Wiring - Critical Test for Bug #187', () => {
    it('should close dropdown after Enter key selection (proves callback is wired)', async () => {
      // This is THE critical test that would have caught bug #187
      // If the onselect callback isn't properly wired from BaseNode to
      // SlashCommandDropdown, the dropdown will NOT close after selection

      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');

      // Type "/" to show dropdown
      await user.click(contentEditableElement!);
      await user.keyboard('/');
      await waitForEffects();

      // Verify dropdown is visible
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Press Enter to select the first command
      document.dispatchEvent(createKeyboardEvent('Enter'));
      await waitForEffects();

      // THE KEY ASSERTION: Dropdown should close after selection
      // This proves the callback chain works:
      // 1. User presses Enter
      // 2. SlashCommandDropdown calls onselect callback
      // 3. BaseNode's handleSlashCommandSelect executes
      // 4. BaseNode sets showSlashCommands = false
      // 5. Dropdown disappears
      await waitFor(
        () => {
          expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
        },
        { timeout: 2000 }
      );
    });

    it('should close dropdown after mouse click selection (proves callback is wired)', async () => {
      // Same test as above but with mouse click instead of keyboard
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');

      // Type "/" to show dropdown
      await user.click(contentEditableElement!);
      await user.keyboard('/');
      await waitForEffects();

      // Verify dropdown is visible
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Click the first option
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(0);
      await user.click(options[0]);
      await waitForEffects();

      // THE KEY ASSERTION: Dropdown should close after selection
      await waitFor(
        () => {
          expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
        },
        { timeout: 2000 }
      );
    });

    it('should close dropdown on Escape without triggering selection', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');

      // Type "/" to show dropdown
      await user.click(contentEditableElement!);
      await user.keyboard('/');
      await waitForEffects();

      // Verify dropdown is visible
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Press Escape
      document.dispatchEvent(createKeyboardEvent('Escape'));
      await waitForEffects();

      // Dropdown should close
      await waitFor(
        () => {
          expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
        },
        { timeout: 2000 }
      );

      // Content should still contain the "/"
      expect(contentEditableElement!.textContent).toContain('/');
    });
  });

  describe('Keyboard Navigation', () => {
    it('should navigate through commands with arrow keys', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const contentEditableElement = container.querySelector('[contenteditable="true"]');

      // Type "/" to show dropdown
      await user.click(contentEditableElement!);
      await user.keyboard('/');
      await waitForEffects();

      // Verify dropdown is visible
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Get initial selection state
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(1);
      expect(options[0]).toHaveAttribute('aria-selected', 'true');

      // Press ArrowDown
      document.dispatchEvent(createKeyboardEvent('ArrowDown'));
      await waitForEffects();

      // Second option should now be selected
      expect(options[1]).toHaveAttribute('aria-selected', 'true');
      expect(options[0]).toHaveAttribute('aria-selected', 'false');
    });
  });
});
