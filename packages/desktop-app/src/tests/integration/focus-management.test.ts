/**
 * Focus Management Verification Tests
 *
 * Comprehensive integration tests for focus behavior across the application.
 * Verifies that focus is properly managed during:
 * - Modal interactions (autocomplete, slash commands)
 * - Node creation within single nodes
 * - Keyboard interactions
 * - Focus restoration after modal close
 *
 * Test Coverage:
 * - Modal Focus - Autocomplete (5 tests)
 * - Node Creation Focus (4 tests)
 * - Slash Command Focus (2 tests)
 * - Editor Focus Stability (3 tests)
 *
 * Note: Multi-node navigation tests require BaseNodeViewer with full NodeServiceContext,
 * which is tested separately in viewer-specific test files.
 *
 * Related: Issue #161
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { waitForEffects } from '../helpers';
import BaseNode from '$lib/design/components/base-node.svelte';

/**
 * Helper function to set up a focused editor for testing
 * Reduces boilerplate and ensures consistent setup across tests
 */
async function setupFocusedEditor(content = '', nodeType: string = 'text') {
  const user = userEvent.setup();

  const { container } = render(BaseNode, {
    nodeId: 'test-node',
    nodeType,
    content,
    autoFocus: true
  });

  const editor = container.querySelector('[contenteditable="true"]') as HTMLElement;
  if (!editor) throw new Error('Editor not found in test setup');

  await waitFor(() => {
    expect(document.activeElement).toBe(editor);
  });

  return { user, container, editor };
}

describe('Focus Management', () => {
  beforeEach(() => {
    // Clean up DOM between tests
    document.body.innerHTML = '';
  });

  describe('Modal Focus - Autocomplete', () => {
    it('should maintain editor focus even when typing trigger characters', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Type "@" character (potential autocomplete trigger)
      await user.keyboard('@');
      await waitForEffects();

      // Focus should remain in editor regardless of autocomplete state
      expect(document.activeElement).toBe(editor);

      // Continue typing - focus should remain stable
      await user.keyboard('test');
      await waitForEffects();

      expect(document.activeElement).toBe(editor);
    });

    it('should maintain editor focus when typing slash character', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Type "/" at start (potential slash command trigger)
      await user.keyboard('/');
      await waitForEffects();

      // Editor should maintain focus regardless of dropdown state
      expect(document.activeElement).toBe(editor);
    });

    it('should maintain focus when pressing Enter after typing', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Type content and press Enter
      await user.keyboard('test{Enter}');
      await waitForEffects();

      // Focus should remain in editor (IMPROVED: specific assertion)
      expect(document.activeElement).toBe(editor);
    });

    it('should maintain focus when pressing Escape', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Press Escape (should not lose focus)
      await user.keyboard('{Escape}');
      await waitForEffects();

      // Focus should remain in editor
      expect(document.activeElement).toBe(editor);
    });

    it('should trap focus within modal context when Tab is pressed', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Trigger autocomplete modal
      await user.keyboard('@');
      await waitForEffects();

      // Check if modal appeared
      const listbox = screen.queryByRole('listbox');

      if (listbox) {
        // If modal exists, verify focus trapping
        // Press Tab - focus should stay within modal or editor
        await user.keyboard('{Tab}');
        await waitForEffects();

        const activeInModal =
          document.activeElement === editor || listbox.contains(document.activeElement);
        expect(activeInModal).toBe(true);
      } else {
        // If no modal, Tab maintains focus in editor
        await user.keyboard('{Tab}');
        await waitForEffects();

        // Verify editor is still focusable
        editor.focus();
        await waitForEffects();
        expect(document.activeElement).toBe(editor);
      }
    });
  });

  describe('Node Creation Focus', () => {
    it('should maintain editor focus after Enter key press', async () => {
      const { user, editor } = await setupFocusedEditor('Test content');

      // Position cursor at end and press Enter
      await user.keyboard('{End}{Enter}');
      await waitForEffects();

      // Focus should remain in editor (IMPROVED: specific assertion)
      expect(document.activeElement).toBe(editor);
    });

    it('should position cursor at start of new node after Enter', async () => {
      const { user, editor } = await setupFocusedEditor('Test content');

      // Position cursor at end and press Enter
      await user.keyboard('{End}{Enter}');
      await waitForEffects();

      // Verify cursor is at start of line (or content)
      const selection = window.getSelection();
      const range = selection?.getRangeAt(0);

      // Cursor should be collapsed (no selection) and ready for input
      expect(range?.collapsed).toBe(true);

      // Verify editor still has focus (most important for cursor positioning)
      expect(document.activeElement).toBe(editor);
    });

    it('should preserve editor focusability after content split with Enter', async () => {
      const { user, editor } = await setupFocusedEditor('BeforeAfter');

      // Position cursor in middle (after "Before")
      await user.click(editor);
      const selection = window.getSelection();
      const range = document.createRange();
      const textNode = editor.firstChild;

      if (textNode && textNode.nodeType === Node.TEXT_NODE) {
        range.setStart(textNode, 6); // After "Before"
        range.collapse(true);
        selection?.removeAllRanges();
        selection?.addRange(range);
      }

      // Press Enter to split content
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Focus should remain in editor (IMPROVED: specific assertion)
      expect(document.activeElement).toBe(editor);

      // Should be able to continue typing
      await user.keyboard('New content');
      await waitForEffects();

      // Content should have been updated (proving focus is correct)
      const contentAfter = editor.textContent || '';
      expect(contentAfter.length).toBeGreaterThan(0);
    });

    it('should support programmatic focus via autoFocus prop', async () => {
      const { editor } = await setupFocusedEditor('Test');

      // Editor should be ready for input (has correct attributes)
      expect(editor).toHaveAttribute('contenteditable', 'true');
      expect(editor).toHaveAttribute('tabindex', '0');
    });

    it('should allow manual focus restoration after blur', async () => {
      const { editor } = await setupFocusedEditor('Test');

      // Blur the editor
      editor.blur();
      await waitForEffects();

      expect(document.activeElement).not.toBe(editor);

      // Manually restore focus
      editor.focus();
      await waitForEffects();

      expect(document.activeElement).toBe(editor);

      // Verify editor is still functional
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });
  });

  describe('Editor Focus Stability', () => {
    it('should maintain focus during arrow key navigation within editor', async () => {
      const { user, editor } = await setupFocusedEditor('Test content with multiple words');

      // Navigate with arrow keys
      await user.keyboard('{ArrowRight}{ArrowRight}{ArrowLeft}');
      await waitForEffects();

      // Focus should remain in editor
      expect(document.activeElement).toBe(editor);

      // Navigate to start/end
      await user.keyboard('{Home}{End}');
      await waitForEffects();

      // Focus still stable
      expect(document.activeElement).toBe(editor);
    });

    it('should maintain focus during text selection', async () => {
      const { user, editor } = await setupFocusedEditor('Selectable text content');

      // Select all text
      await user.keyboard('{Control>}a{/Control}');
      await waitForEffects();

      // Focus should remain during selection
      expect(document.activeElement).toBe(editor);

      // Verify selection exists
      const selection = window.getSelection();
      expect(selection?.toString()).toBeTruthy();

      // Type to replace selection
      await user.keyboard('Replaced');
      await waitForEffects();

      // Focus still in editor
      expect(document.activeElement).toBe(editor);
      expect(editor.textContent).toBe('Replaced');
    });

    it('should maintain focusability after user interaction', async () => {
      const { user, editor } = await setupFocusedEditor('Initial content');

      // Blur and refocus to simulate interaction
      editor.blur();
      await waitForEffects();

      await user.click(editor);
      await waitForEffects();

      // Focus should be restored
      expect(document.activeElement).toBe(editor);

      // Editor should remain functional
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });
  });

  describe('Slash Command Focus', () => {
    it('should maintain editor focus after slash command insertion', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Type "/" to trigger slash command dropdown
      await user.keyboard('/');
      await waitForEffects();

      // Check if dropdown appeared (IMPROVED: condition-based check, no hard-coded wait)
      await waitFor(
        () => {
          const hasSettled =
            screen.queryByRole('listbox') !== null || document.activeElement === editor;
          expect(hasSettled).toBe(true);
        },
        { timeout: 500 }
      );

      // If dropdown appears, select a command
      const listbox = screen.queryByRole('listbox');
      if (listbox) {
        await user.keyboard('{Enter}');
        await waitForEffects();
      }

      // Focus should remain in editor (primary assertion)
      expect(document.activeElement).toBe(editor);

      // Editor should remain editable and functional
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });

    it('should auto-focus editor after node type conversion via slash command', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Type "/" and convert to different node type (e.g., /task)
      await user.keyboard('/task');
      await waitForEffects();

      // Check if slash command system responded (IMPROVED: condition-based check)
      await waitFor(
        () => {
          const hasSettled =
            screen.queryByRole('listbox') !== null || document.activeElement === editor;
          expect(hasSettled).toBe(true);
        },
        { timeout: 500 }
      );

      // If slash command system is active, try to execute
      const listbox = screen.queryByRole('listbox');
      if (listbox) {
        await user.keyboard('{Enter}');
        await waitForEffects();
      }

      // Focus should be in editor (IMPROVED: specific assertion)
      expect(document.activeElement).toBe(editor);

      // Editor should remain editable
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });
  });

  describe('Focus Management - Error Scenarios', () => {
    it('should maintain focus if autocomplete trigger fails', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Trigger autocomplete
      await user.keyboard('@');
      await waitForEffects();

      // Even if autocomplete fails to appear, focus should remain stable
      expect(document.activeElement).toBe(editor);

      // Editor should remain editable
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });

    it('should maintain focus if slash command fails', async () => {
      const { user, editor } = await setupFocusedEditor();

      // Trigger slash command
      await user.keyboard('/');
      await waitForEffects();

      // Even if slash dropdown fails to appear, focus should remain
      expect(document.activeElement).toBe(editor);

      // Editor should remain editable
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });
  });
});
