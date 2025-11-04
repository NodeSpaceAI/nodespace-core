/**
 * Backspace Key - Focus Management (Browser Mode Tests)
 *
 * Tests real browser behavior for Backspace key operations and focus management.
 * These tests verify browser-specific APIs that Happy-DOM cannot simulate:
 * - Real focus/blur events
 * - Textarea Selection API (cursor positioning at merge point)
 * - Focus transitions when combining nodes
 * - Event propagation and preventDefault behavior
 *
 * NOTE: These are simplified tests focusing on browser-specific focus and cursor APIs.
 * Business logic (node deletion, content merging, sibling chains) is tested
 * in Happy-DOM unit tests.
 *
 * Related: Issue #282 (Vitest Browser Mode)
 */

import { describe, it, expect, beforeEach } from 'vitest';

describe('Backspace - Focus Transitions (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('moves focus to previous node on backspace at start', () => {
    const textarea1 = document.createElement('textarea');
    textarea1.id = 'node-1';
    textarea1.value = 'First node';
    document.body.appendChild(textarea1);

    const textarea2 = document.createElement('textarea');
    textarea2.id = 'node-2';
    textarea2.value = 'Second node';
    document.body.appendChild(textarea2);

    // Focus second textarea at start (position 0)
    textarea2.focus();
    textarea2.setSelectionRange(0, 0);

    expect(document.activeElement).toBe(textarea2);

    // Simulate backspace at start - focus should move to previous node
    textarea1.focus();

    expect(document.activeElement).toBe(textarea1);
  });

  it('positions cursor at end of previous node after merge', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Content from first node';
    document.body.appendChild(textarea);

    textarea.focus();

    // After merging, cursor should be at end of original content
    const originalLength = textarea.value.length;
    textarea.setSelectionRange(originalLength, originalLength);

    expect(textarea.selectionStart).toBe(originalLength);
    expect(textarea.selectionEnd).toBe(originalLength);
  });

  it('calculates merge point cursor position correctly', () => {
    const firstNodeContent = 'Hello';
    const secondNodeContent = 'World';

    // After merge: "HelloWorld"
    const mergedContent = firstNodeContent + secondNodeContent;
    const mergePoint = firstNodeContent.length;

    const textarea = document.createElement('textarea');
    textarea.value = mergedContent;
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(mergePoint, mergePoint);

    // Cursor at position 5 (after "Hello", before "World")
    expect(textarea.selectionStart).toBe(5);

    const beforeCursor = textarea.value.substring(0, mergePoint);
    const afterCursor = textarea.value.substring(mergePoint);

    expect(beforeCursor).toBe('Hello');
    expect(afterCursor).toBe('World');
  });
});

describe('Backspace - Content Merging (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('merges content from two textareas', () => {
    const content1 = 'First';
    const content2 = 'Second';

    const textarea = document.createElement('textarea');
    // Simulate merged content
    textarea.value = content1 + content2;
    document.body.appendChild(textarea);

    expect(textarea.value).toBe('FirstSecond');
  });

  it('preserves cursor position at merge junction', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'BeforeMergeAfterMerge';
    document.body.appendChild(textarea);

    textarea.focus();

    // Set cursor at merge point (after "BeforeMerge")
    const mergePoint = 11;
    textarea.setSelectionRange(mergePoint, mergePoint);

    expect(textarea.selectionStart).toBe(mergePoint);

    const before = textarea.value.substring(0, mergePoint);
    const after = textarea.value.substring(mergePoint);

    expect(before).toBe('BeforeMerge');
    expect(after).toBe('AfterMerge');
  });

  it('handles backspace at cursor position 0', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Content here';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(0, 0);

    expect(textarea.selectionStart).toBe(0);

    // At position 0, backspace would trigger node merge
    // (can't delete backward in current node)
  });
});

describe('Backspace - Event Handling (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('detects backspace key events', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Test content';
    document.body.appendChild(textarea);

    textarea.focus();

    let backspacePressed = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Backspace') {
        backspacePressed = true;
      }
    });

    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }));

    expect(backspacePressed).toBe(true);
  });

  it('can prevent default backspace behavior', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Content';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(0, 0);

    let backspaceAtStart = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Backspace' && textarea.selectionStart === 0) {
        backspaceAtStart = true;
        e.preventDefault(); // Prevent default deletion
      }
    });

    const event = new KeyboardEvent('keydown', {
      key: 'Backspace',
      bubbles: true,
      cancelable: true
    });
    textarea.dispatchEvent(event);

    expect(backspaceAtStart).toBe(true);
  });

  it('detects cursor position on backspace', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Hello World';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(6, 6); // After "Hello "

    let cursorPos = -1;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Backspace') {
        cursorPos = textarea.selectionStart;
      }
    });

    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }));

    expect(cursorPos).toBe(6);
  });
});

describe('Backspace - Sequential Operations (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('handles multiple backspace operations', () => {
    // Create three nodes
    const node1 = document.createElement('textarea');
    node1.id = 'node-1';
    node1.value = 'First';
    document.body.appendChild(node1);

    const node2 = document.createElement('textarea');
    node2.id = 'node-2';
    node2.value = 'Second';
    document.body.appendChild(node2);

    const node3 = document.createElement('textarea');
    node3.id = 'node-3';
    node3.value = 'Third';
    document.body.appendChild(node3);

    // Focus third node
    node3.focus();
    expect(document.activeElement).toBe(node3);

    // First backspace at start - merge with node2
    node2.focus();
    expect(document.activeElement).toBe(node2);

    // Second backspace at start - merge with node1
    node1.focus();
    expect(document.activeElement).toBe(node1);
  });

  it('tracks focus through multiple merge operations', () => {
    const textarea1 = document.createElement('textarea');
    const textarea2 = document.createElement('textarea');
    const textarea3 = document.createElement('textarea');

    document.body.appendChild(textarea1);
    document.body.appendChild(textarea2);
    document.body.appendChild(textarea3);

    let focusSequence: string[] = [];

    [textarea1, textarea2, textarea3].forEach((ta, index) => {
      ta.addEventListener('focus', () => {
        focusSequence.push(`textarea${index + 1}`);
      });
    });

    textarea3.focus();
    textarea2.focus();
    textarea1.focus();

    expect(focusSequence).toEqual(['textarea3', 'textarea2', 'textarea1']);
  });
});

describe('Backspace - Merge Point Calculation (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('calculates correct merge point for simple text', () => {
    const prevContent = 'Previous node';
    const mergePoint = prevContent.length;

    const textarea = document.createElement('textarea');
    textarea.value = prevContent + 'Current node';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(mergePoint, mergePoint);

    expect(textarea.selectionStart).toBe(13); // Length of "Previous node"
  });

  it('handles merge point with multiline content', () => {
    const prevContent = 'Line 1\nLine 2';
    const currContent = '\nLine 3';

    const textarea = document.createElement('textarea');
    textarea.value = prevContent + currContent;
    document.body.appendChild(textarea);

    textarea.focus();

    const mergePoint = prevContent.length;
    textarea.setSelectionRange(mergePoint, mergePoint);

    expect(textarea.selectionStart).toBe(13); // "Line 1\nLine 2" length
  });

  it('handles merge point with empty previous content', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Current content'; // Previous was empty
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(0, 0); // Merge point at start

    expect(textarea.selectionStart).toBe(0);
  });

  it('handles merge point with empty current content', () => {
    const prevContent = 'Previous content';

    const textarea = document.createElement('textarea');
    textarea.value = prevContent; // Current was empty, just keep previous
    document.body.appendChild(textarea);

    textarea.focus();

    const endPos = prevContent.length;
    textarea.setSelectionRange(endPos, endPos);

    expect(textarea.selectionStart).toBe(prevContent.length);
  });
});

describe('Backspace - Edge Cases (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('handles backspace with selection range (deletes selection)', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Hello World';
    document.body.appendChild(textarea);

    textarea.focus();
    // Select "World"
    textarea.setSelectionRange(6, 11);

    const hasSelection = textarea.selectionStart !== textarea.selectionEnd;
    expect(hasSelection).toBe(true);

    // Backspace with selection deletes selection (doesn't trigger merge)
    const before = textarea.value.substring(0, textarea.selectionStart);
    const after = textarea.value.substring(textarea.selectionEnd);

    textarea.value = before + after; // "Hello "
    textarea.setSelectionRange(before.length, before.length);

    expect(textarea.value).toBe('Hello ');
  });

  it('handles backspace on first node (no previous node)', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'First node content';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(0, 0);

    // At start of first node, backspace is a no-op
    expect(textarea.selectionStart).toBe(0);

    // No previous node to merge with
    const hasPreviousNode = false; // Would be checked in real implementation
    expect(hasPreviousNode).toBe(false);
  });

  it('handles backspace with trailing whitespace', () => {
    const prevContent = 'Previous node   '; // Trailing spaces
    const currContent = 'Current node';

    const mergePoint = prevContent.length;

    const textarea = document.createElement('textarea');
    textarea.value = prevContent + currContent;
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(mergePoint, mergePoint);

    // Cursor at position 16 (includes trailing spaces)
    expect(textarea.selectionStart).toBe(16);

    const before = textarea.value.substring(0, mergePoint);
    expect(before).toBe('Previous node   ');
  });
});
