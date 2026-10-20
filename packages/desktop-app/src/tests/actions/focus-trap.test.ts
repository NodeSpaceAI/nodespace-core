/**
 * Tests for the focusTrap Svelte action.
 *
 * Drives REAL keyboard focus (focus an element, dispatch keydown on the focused
 * element) rather than synthetically firing on the overlay — that's the whole
 * point of the fix, so the tests must exercise the same path a user would.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { focusTrap } from '$lib/actions/focus-trap';

/** Build a dialog-like container with `n` buttons, attached to the document. */
function makeDialog(n: number): { node: HTMLElement; buttons: HTMLElement[] } {
  const node = document.createElement('div');
  node.setAttribute('tabindex', '0');
  const buttons: HTMLElement[] = [];
  for (let i = 0; i < n; i++) {
    const b = document.createElement('button');
    b.textContent = `btn-${i}`;
    node.appendChild(b);
    buttons.push(b);
  }
  document.body.appendChild(node);
  return { node, buttons };
}

function tab(target: Element, shift = false) {
  target.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Tab', shiftKey: shift, bubbles: true })
  );
}

describe('focusTrap action', () => {
  let outside: HTMLElement;

  beforeEach(() => {
    // An element outside the trap that "had focus" before the dialog opened.
    outside = document.createElement('button');
    outside.textContent = 'outside';
    document.body.appendChild(outside);
    outside.focus();
  });

  afterEach(() => {
    document.body.innerHTML = '';
    vi.clearAllMocks();
  });

  it('moves focus to the first focusable element on apply', () => {
    const { node, buttons } = makeDialog(2);
    const trap = focusTrap(node, {});
    expect(document.activeElement).toBe(buttons[0]);
    trap.destroy();
  });

  it('focuses the container itself when there are no focusable children', () => {
    const { node } = makeDialog(0);
    const trap = focusTrap(node, {});
    expect(document.activeElement).toBe(node);
    trap.destroy();
  });

  it('wraps Tab from the last element back to the first', () => {
    const { node, buttons } = makeDialog(3);
    const trap = focusTrap(node, {});
    buttons[2].focus();
    tab(buttons[2]);
    expect(document.activeElement).toBe(buttons[0]);
    trap.destroy();
  });

  it('wraps Shift+Tab from the first element to the last', () => {
    const { node, buttons } = makeDialog(3);
    const trap = focusTrap(node, {});
    buttons[0].focus();
    tab(buttons[0], true);
    expect(document.activeElement).toBe(buttons[2]);
    trap.destroy();
  });

  it('invokes onEscape on Escape pressed from inside the trap', () => {
    const onEscape = vi.fn();
    const { node, buttons } = makeDialog(2);
    const trap = focusTrap(node, { onEscape });
    buttons[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(onEscape).toHaveBeenCalledTimes(1);
    trap.destroy();
  });

  it('restores focus to the previously-focused element on destroy', () => {
    const { node } = makeDialog(2);
    const trap = focusTrap(node, {});
    expect(document.activeElement).not.toBe(outside);
    trap.destroy();
    expect(document.activeElement).toBe(outside);
  });

  it('update() swaps the onEscape handler', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { node, buttons } = makeDialog(1);
    const trap = focusTrap(node, { onEscape: first });
    trap.update({ onEscape: second });
    buttons[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    trap.destroy();
  });

  it('stops the Escape event from propagating past the trap', () => {
    const onEscape = vi.fn();
    const ancestorEscape = vi.fn();
    const { node, buttons } = makeDialog(1);
    document.body.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') ancestorEscape();
    });
    const trap = focusTrap(node, { onEscape });
    buttons[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(onEscape).toHaveBeenCalledTimes(1);
    expect(ancestorEscape).not.toHaveBeenCalled();
    trap.destroy();
  });

  describe('focus taken from outside the dialog', () => {
    it('returns focus to the dialog when an outside element grabs it', () => {
      const { node, buttons } = makeDialog(2);
      const editor = document.createElement('textarea');
      document.body.appendChild(editor);
      const trap = focusTrap(node, {});

      editor.focus();

      expect(document.activeElement).toBe(buttons[0]);
      trap.destroy();
    });

    it('lets focus stay inside another aria-modal element', () => {
      const { node } = makeDialog(2);
      const other = document.createElement('div');
      other.setAttribute('aria-modal', 'true');
      const input = document.createElement('input');
      other.appendChild(input);
      document.body.appendChild(other);
      const trap = focusTrap(node, {});

      input.focus();

      expect(document.activeElement).toBe(input);
      trap.destroy();
    });

    it('stops reclaiming focus once destroyed', () => {
      const { node } = makeDialog(2);
      const editor = document.createElement('textarea');
      document.body.appendChild(editor);
      const trap = focusTrap(node, {});
      trap.destroy();

      editor.focus();

      expect(document.activeElement).toBe(editor);
    });
  });
});
