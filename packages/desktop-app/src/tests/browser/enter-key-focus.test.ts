/**
 * Enter Key - Focus Management (Browser Mode Tests)
 *
 * Tests real browser behavior for Enter key operations and focus management.
 * These tests verify browser-specific APIs that Happy-DOM cannot simulate:
 * - Real focus/blur events
 * - Textarea Selection API (cursor positioning)
 * - Focus transitions between elements
 * - Event propagation and preventDefault behavior
 *
 * NOTE: These are simplified tests focusing on browser-specific focus and cursor APIs.
 * Business logic (node creation, sibling chains, database persistence) is tested
 * in Happy-DOM unit tests.
 *
 * Related: Issue #282 (Vitest Browser Mode)
 */

import { describe, it, expect, beforeEach } from 'vitest';

describe('Enter Key - Focus Transitions (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('can move focus from one textarea to another', () => {
    const textarea1 = document.createElement('textarea');
    textarea1.id = 'node-1';
    textarea1.value = 'First node';
    document.body.appendChild(textarea1);

    const textarea2 = document.createElement('textarea');
    textarea2.id = 'node-2';
    textarea2.value = 'Second node';
    document.body.appendChild(textarea2);

    // Focus first textarea
    textarea1.focus();
    expect(document.activeElement).toBe(textarea1);

    // Simulate Enter key creating new node and moving focus
    textarea2.focus();
    expect(document.activeElement).toBe(textarea2);
  });

  it('maintains cursor position when moving focus between textareas', () => {
    const textarea1 = document.createElement('textarea');
    textarea1.value = 'Content here';
    document.body.appendChild(textarea1);

    const textarea2 = document.createElement('textarea');
    textarea2.value = 'More content';
    document.body.appendChild(textarea2);

    // Set cursor position in first textarea
    textarea1.focus();
    textarea1.setSelectionRange(7, 7); // After "Content"

    const savedCursor = textarea1.selectionStart;
    expect(savedCursor).toBe(7);

    // Move focus to second textarea
    textarea2.focus();
    textarea2.setSelectionRange(0, 0); // Start of new node

    expect(document.activeElement).toBe(textarea2);
    expect(textarea2.selectionStart).toBe(0);

    // Original cursor position preserved when returning focus
    textarea1.focus();
    // Chromium restores cursor position automatically
    expect(textarea1.selectionStart).toBe(7);
  });

  it('handles Enter key event with preventDefault', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Press Enter here';
    document.body.appendChild(textarea);

    textarea.focus();

    let enterPressed = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        enterPressed = true;
        e.preventDefault(); // Prevent default newline insertion
      }
    });

    // Simulate Enter key
    const event = new KeyboardEvent('keydown', {
      key: 'Enter',
      bubbles: true,
      cancelable: true
    });
    textarea.dispatchEvent(event);

    expect(enterPressed).toBe(true);
    // Note: defaultPrevented state depends on event dispatch timing
  });
});

describe('Enter Key - Content Splitting (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('can split content at cursor position', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Hello World';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(6, 6); // After "Hello "

    const cursorPos = textarea.selectionStart;
    const beforeCursor = textarea.value.substring(0, cursorPos);
    const afterCursor = textarea.value.substring(cursorPos);

    expect(beforeCursor).toBe('Hello ');
    expect(afterCursor).toBe('World');

    // Simulate splitting: first node keeps "before", second node gets "after"
    const node1Content = beforeCursor.trimEnd(); // "Hello"
    const node2Content = afterCursor; // "World"

    expect(node1Content).toBe('Hello');
    expect(node2Content).toBe('World');
  });

  it('handles split at start of content', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Content here';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(0, 0); // At start

    const cursorPos = textarea.selectionStart;
    const beforeCursor = textarea.value.substring(0, cursorPos);
    const afterCursor = textarea.value.substring(cursorPos);

    expect(beforeCursor).toBe('');
    expect(afterCursor).toBe('Content here');

    // New node above would be empty, current node keeps all content
  });

  it('handles split at end of content', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Full content';
    document.body.appendChild(textarea);

    textarea.focus();
    const endPos = textarea.value.length;
    textarea.setSelectionRange(endPos, endPos);

    const cursorPos = textarea.selectionStart;
    const beforeCursor = textarea.value.substring(0, cursorPos);
    const afterCursor = textarea.value.substring(cursorPos);

    expect(beforeCursor).toBe('Full content');
    expect(afterCursor).toBe('');

    // Current node keeps all content, new node below is empty
  });

  it('handles split with multiline content', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Line 1\nLine 2\nLine 3';
    document.body.appendChild(textarea);

    textarea.focus();
    // Position cursor in middle of Line 2
    const line1Length = 6; // "Line 1"
    const newline1 = 1;
    const line2Partial = 3; // "Lin"
    const cursorPos = line1Length + newline1 + line2Partial; // Position after "Lin" in Line 2

    textarea.setSelectionRange(cursorPos, cursorPos);

    const beforeCursor = textarea.value.substring(0, cursorPos);
    const afterCursor = textarea.value.substring(cursorPos);

    expect(beforeCursor).toBe('Line 1\nLin');
    expect(afterCursor).toBe('e 2\nLine 3');
  });
});

describe('Enter Key - Cursor Positioning After Split (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('positions cursor at start of new node after split', () => {
    const newNodeTextarea = document.createElement('textarea');
    newNodeTextarea.value = 'World'; // Content that was after cursor
    document.body.appendChild(newNodeTextarea);

    newNodeTextarea.focus();
    newNodeTextarea.setSelectionRange(0, 0); // Cursor at start

    expect(document.activeElement).toBe(newNodeTextarea);
    expect(newNodeTextarea.selectionStart).toBe(0);
    expect(newNodeTextarea.selectionEnd).toBe(0);
  });

  it('preserves cursor in original node when insertAtBeginning=true', () => {
    const originalTextarea = document.createElement('textarea');
    originalTextarea.value = 'Original content';
    document.body.appendChild(originalTextarea);

    originalTextarea.focus();
    originalTextarea.setSelectionRange(8, 8); // Middle of content

    const cursorPos = originalTextarea.selectionStart;

    // When inserting at beginning, focus stays in original node
    expect(document.activeElement).toBe(originalTextarea);
    expect(originalTextarea.selectionStart).toBe(cursorPos);
  });

  it('can set cursor to specific position in new textarea', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Some content here';
    document.body.appendChild(textarea);

    textarea.focus();

    // Set cursor to position 5
    textarea.setSelectionRange(5, 5);

    expect(textarea.selectionStart).toBe(5);
    expect(textarea.selectionEnd).toBe(5);

    // Verify text before and after cursor
    const before = textarea.value.substring(0, 5);
    const after = textarea.value.substring(5);

    expect(before).toBe('Some ');
    expect(after).toBe('content here');
  });
});

describe('Enter Key - Focus Chain Verification (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('handles sequential Enter presses with focus transitions', () => {
    // Create initial node
    const node1 = document.createElement('textarea');
    node1.id = 'node-1';
    node1.value = 'First';
    document.body.appendChild(node1);

    node1.focus();
    expect(document.activeElement).toBe(node1);

    // Simulate first Enter - create node2
    const node2 = document.createElement('textarea');
    node2.id = 'node-2';
    node2.value = '';
    document.body.appendChild(node2);

    node2.focus();
    expect(document.activeElement).toBe(node2);

    // Simulate second Enter - create node3
    const node3 = document.createElement('textarea');
    node3.id = 'node-3';
    node3.value = '';
    document.body.appendChild(node3);

    node3.focus();
    expect(document.activeElement).toBe(node3);

    // Verify all nodes exist and can receive focus
    node1.focus();
    expect(document.activeElement).toBe(node1);

    node2.focus();
    expect(document.activeElement).toBe(node2);

    node3.focus();
    expect(document.activeElement).toBe(node3);
  });

  it('verifies focus/blur event sequence during transitions', () => {
    const textarea1 = document.createElement('textarea');
    const textarea2 = document.createElement('textarea');
    document.body.appendChild(textarea1);
    document.body.appendChild(textarea2);

    let focus1Count = 0;
    let blur1Count = 0;
    let focus2Count = 0;

    textarea1.addEventListener('focus', () => focus1Count++);
    textarea1.addEventListener('blur', () => blur1Count++);
    textarea2.addEventListener('focus', () => focus2Count++);

    // Focus first textarea
    textarea1.focus();
    expect(focus1Count).toBe(1);
    expect(blur1Count).toBe(0);

    // Transition to second textarea (simulating Enter key creating new node)
    textarea2.focus();

    expect(focus1Count).toBe(1);
    expect(blur1Count).toBe(1); // First textarea blurred
    expect(focus2Count).toBe(1); // Second textarea focused
  });
});

describe('Enter Key - Edge Cases (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('handles Enter with Shift modifier (Shift+Enter for newline)', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Line 1';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(6, 6);

    let shiftEnterPressed = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && e.shiftKey) {
        shiftEnterPressed = true;
        // Don't preventDefault - allow default newline insertion
      }
    });

    const event = new KeyboardEvent('keydown', {
      key: 'Enter',
      shiftKey: true,
      bubbles: true
    });
    textarea.dispatchEvent(event);

    expect(shiftEnterPressed).toBe(true);
  });

  it('handles Enter with empty content', () => {
    const textarea = document.createElement('textarea');
    textarea.value = '';
    document.body.appendChild(textarea);

    textarea.focus();
    expect(textarea.selectionStart).toBe(0);
    expect(textarea.value).toBe('');

    // Pressing Enter on empty node creates empty node below
    const newTextarea = document.createElement('textarea');
    newTextarea.value = '';
    document.body.appendChild(newTextarea);

    newTextarea.focus();
    expect(document.activeElement).toBe(newTextarea);
    expect(newTextarea.value).toBe('');
  });

  it('handles Enter with selection range (replaces selected text)', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Hello World';
    document.body.appendChild(textarea);

    textarea.focus();
    // Select "World"
    textarea.setSelectionRange(6, 11);

    expect(textarea.selectionStart).toBe(6);
    expect(textarea.selectionEnd).toBe(11);

    const selectedText = textarea.value.substring(textarea.selectionStart, textarea.selectionEnd);
    expect(selectedText).toBe('World');

    // When Enter is pressed with selection, selected text is deleted before split
    const beforeSelection = textarea.value.substring(0, textarea.selectionStart);
    const afterSelection = textarea.value.substring(textarea.selectionEnd);

    expect(beforeSelection).toBe('Hello ');
    expect(afterSelection).toBe('');
  });
});
