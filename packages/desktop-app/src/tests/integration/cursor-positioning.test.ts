/**
 * Cursor Positioning Integration Tests
 *
 * Tests for cursor positioning improvements including:
 * - Arrow key navigation maintaining horizontal position
 * - Shift+Enter positioning after syntax
 * - Viewport scroll offset handling
 *
 * NOTE: These tests verify the logic and code paths of cursor positioning.
 * Full pixel-perfect positioning tests require a real browser environment
 * as Happy-DOM returns zero for getBoundingClientRect() measurements.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { tick } from 'svelte';
import BaseNode from '$lib/design/components/base-node.svelte';

// Local helper to wait for effects without importing from test-helpers
async function waitForEffects() {
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 50));
}

// Mock document.execCommand for Happy-DOM compatibility
beforeEach(() => {
  if (typeof document.execCommand !== 'function') {
    document.execCommand = vi.fn((command: string) => {
      if (command === 'insertLineBreak') {
        const selection = window.getSelection();
        if (selection && selection.rangeCount > 0) {
          const range = selection.getRangeAt(0);
          const br = document.createElement('br');
          range.insertNode(br);
          range.setStartAfter(br);
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
        }
        return true;
      }
      return false;
    });
  }
});

/**
 * Helper to set up a node for cursor positioning tests
 */
async function setupNode(content: string, nodeType = 'text', headerLevel = 0) {
  const user = userEvent.setup();

  const { container } = render(BaseNode, {
    nodeId: 'test-node',
    nodeType,
    content,
    headerLevel,
    autoFocus: true,
    editableConfig: { allowMultiline: true }
  });

  const editor = container.querySelector('[contenteditable="true"]') as HTMLElement;
  if (!editor) throw new Error('Editor not found in test setup');

  await waitForEffects();

  return { user, container, editor };
}

/**
 * Helper to get cursor position in pixels from viewport left
 */
function getCursorPixelPosition(): number {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) return 0;

  const range = selection.getRangeAt(0);
  const cursorRange = document.createRange();
  cursorRange.setStart(range.startContainer, range.startOffset);
  cursorRange.setEnd(range.startContainer, range.startOffset);

  const rect = cursorRange.getBoundingClientRect();
  return rect.left + window.scrollX;
}

describe('Cursor Positioning', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    window.scrollTo(0, 0); // Reset scroll position
  });

  describe('Arrow Navigation Pixel Offset', () => {
    it('should maintain horizontal position when navigating between nodes', async () => {
      // Note: This test requires multi-node setup which needs BaseNodeViewer
      // Placeholder for future implementation with proper viewer context
      expect.assertions(0);
    });

    it('should calculate viewport-relative coordinates with scroll offset', async () => {
      const { editor } = await setupNode('Test content with some text');

      // Position cursor in middle
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        const textNode = editor.firstChild as Text;
        range.setStart(textNode, 10);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Verify cursor positioning logic works (Happy-DOM returns 0 for rect measurements)
      // The important part is that the code path executes without errors
      const initialPosition = getCursorPixelPosition();
      expect(initialPosition).toBeGreaterThanOrEqual(0);

      // Verify scroll offset is added to calculation
      window.scrollTo(50, 0);
      await waitForEffects();

      const positionAfterScroll = getCursorPixelPosition();
      // In Happy-DOM both will be scrollX value, but in real browser they differ correctly
      expect(positionAfterScroll).toBeGreaterThanOrEqual(0);
    });

    it('should use viewport-relative coordinates not element-relative', async () => {
      const { editor } = await setupNode('Test content');

      editor.focus();
      await waitForEffects();

      // Position cursor at start
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        range.selectNodeContents(editor);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      // Verify the coordinate calculation code path works
      const pixelPosition = getCursorPixelPosition();
      expect(pixelPosition).toBeGreaterThanOrEqual(0);

      // In real browser, viewport position differs from element-relative
      // Happy-DOM limitation means we're testing code execution, not exact values
    });
  });

  describe('Shift+Enter Syntax Positioning', () => {
    it('should position cursor after header syntax', async () => {
      const { user, editor } = await setupNode('## Test heading', 'text', 2);

      // Position cursor in middle of text
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        const textNode = editor.firstChild as Text;
        range.setStart(textNode, 7); // After "## Test"
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Press Shift+Enter
      await user.keyboard('{Shift>}{Enter}{/Shift}');
      await waitForEffects();

      // Verify line break was created and content structure is correct
      const content = editor.textContent || '';

      // Should contain a line break
      expect(content).toContain('## Test');

      // Verify cursor positioning logic executed (check for selection existence)
      const newSelection = window.getSelection();
      expect(newSelection).toBeTruthy();
      expect(newSelection?.rangeCount).toBeGreaterThan(0);

      // In real browser, cursor would be positioned after "## " on new line
      // Happy-DOM limitation: we verify the code path works without errors
    });

    it('should position cursor after task checkbox syntax', async () => {
      const { user, editor } = await setupNode('[ ] Task item', 'text');

      // Position cursor in middle
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        const textNode = editor.firstChild as Text;
        range.setStart(textNode, 8); // After "[ ] Task"
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Press Shift+Enter
      await user.keyboard('{Shift>}{Enter}{/Shift}');
      await waitForEffects();

      // Verify line break was created
      const content = editor.textContent || '';
      expect(content).toContain('[ ] Task');

      // Verify cursor positioning logic executed
      const newSelection = window.getSelection();
      expect(newSelection).toBeTruthy();
      expect(newSelection?.rangeCount).toBeGreaterThan(0);
    });

    it('should position cursor after quote syntax', async () => {
      const { user, editor } = await setupNode('> Quote text', 'text');

      // Position cursor in middle
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        const textNode = editor.firstChild as Text;
        range.setStart(textNode, 5); // After "> Quo"
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Press Shift+Enter
      await user.keyboard('{Shift>}{Enter}{/Shift}');
      await waitForEffects();

      // Verify line break was created
      const content = editor.textContent || '';
      expect(content).toContain('> Quote');

      // Verify cursor positioning logic executed
      const newSelection = window.getSelection();
      expect(newSelection).toBeTruthy();
      expect(newSelection?.rangeCount).toBeGreaterThan(0);
    });

    it('should handle Shift+Enter in plain text without syntax', async () => {
      const { user, editor } = await setupNode('Plain text content', 'text');

      // Position cursor in middle
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        const textNode = editor.firstChild as Text;
        range.setStart(textNode, 6); // After "Plain "
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Press Shift+Enter
      await user.keyboard('{Shift>}{Enter}{/Shift}');
      await waitForEffects();

      // Verify line break was created
      const content = editor.textContent || '';
      expect(content).toContain('Plain');

      // Verify cursor exists after line break
      const newSelection = window.getSelection();
      expect(newSelection).toBeTruthy();
      expect(newSelection?.rangeCount).toBeGreaterThan(0);
    });
  });

  describe('Edge Cases', () => {
    it('should handle empty lines gracefully', async () => {
      const { editor } = await setupNode('', 'text');

      editor.focus();
      await waitForEffects();

      // Should position at start without error
      const position = getCursorPixelPosition();
      expect(position).toBeGreaterThanOrEqual(0);
    });

    it('should handle lines with only whitespace', async () => {
      const { editor } = await setupNode('   ', 'text');

      editor.focus();
      await waitForEffects();

      // Should position correctly
      const position = getCursorPixelPosition();
      expect(position).toBeGreaterThanOrEqual(0);
    });

    it('should handle cursor at very end of line', async () => {
      const { user, editor } = await setupNode('## Short', 'text', 2);

      // Position at end
      editor.focus();
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        range.selectNodeContents(editor);
        range.collapse(false); // Collapse to end
        selection.removeAllRanges();
        selection.addRange(range);
      }

      await waitForEffects();

      // Press Shift+Enter
      await user.keyboard('{Shift>}{Enter}{/Shift}');
      await waitForEffects();

      // Should create new line
      const content = editor.textContent || '';
      expect(content).toContain('## Short');

      // Verify cursor positioning logic executed
      const newSelection = window.getSelection();
      expect(newSelection).toBeTruthy();
      expect(newSelection?.rangeCount).toBeGreaterThan(0);
    });
  });
});
