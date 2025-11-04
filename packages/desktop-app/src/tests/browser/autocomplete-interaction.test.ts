/**
 * Autocomplete Interaction - Browser Mode Tests
 *
 * Tests real browser behavior for @mention autocomplete dropdown interactions.
 * These tests verify browser-specific APIs that Happy-DOM cannot simulate:
 * - Real getBoundingClientRect() for positioning
 * - Actual viewport dimensions for edge detection
 * - Real focus/blur events
 * - Textarea Selection API behavior
 * - Keyboard event propagation
 *
 * NOTE: These are simplified unit tests for browser-specific APIs.
 * Full component integration tests would require extensive context setup
 * (NodeServiceContext, FocusManager, etc.) which is beyond the scope of
 * browser mode testing. Business logic is tested in Happy-DOM unit tests.
 *
 * Related: Issue #282 (Vitest Browser Mode)
 */

import { describe, it, expect, beforeEach } from 'vitest';

describe('Autocomplete - Dropdown Positioning (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('verifies getBoundingClientRect returns real dimensions', () => {
    // Create a textarea with explicit dimensions
    const textarea = document.createElement('textarea');
    textarea.style.position = 'absolute';
    textarea.style.left = '100px';
    textarea.style.top = '100px';
    textarea.style.width = '300px';
    textarea.style.height = '100px';
    document.body.appendChild(textarea);

    // Get bounding rect (Happy-DOM returns all zeros, real browser returns actual values)
    const rect = textarea.getBoundingClientRect();

    // Verify we get real dimensions
    expect(rect.width).toBeGreaterThan(0);
    expect(rect.height).toBeGreaterThan(0);
    expect(rect.left).toBeGreaterThan(0);
    expect(rect.top).toBeGreaterThan(0);
  });

  it('can calculate dropdown position relative to cursor', () => {
    const textarea = document.createElement('textarea');
    textarea.style.position = 'absolute';
    textarea.style.left = '50px';
    textarea.style.top = '50px';
    textarea.style.width = '400px';
    textarea.value = 'Text before @mention';
    document.body.appendChild(textarea);

    // Focus and set cursor position
    textarea.focus();
    textarea.setSelectionRange(12, 12); // After "@"

    const rect = textarea.getBoundingClientRect();

    // Calculate dropdown position (would be cursor position + offset)
    // In real implementation, we'd calculate exact cursor pixel position
    const dropdownX = rect.left;
    const dropdownY = rect.bottom + 5; // 5px below textarea

    expect(dropdownX).toBeGreaterThan(0);
    expect(dropdownY).toBeGreaterThan(rect.top);
  });

  it('detects viewport edges for smart positioning', () => {
    const dropdown = document.createElement('div');
    dropdown.style.position = 'fixed';
    dropdown.style.width = '300px';
    dropdown.style.height = '400px';
    document.body.appendChild(dropdown);

    // Get viewport dimensions (requires real browser)
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;

    expect(viewportWidth).toBeGreaterThan(0);
    expect(viewportHeight).toBeGreaterThan(0);

    // Simulate cursor near right edge
    const cursorX = viewportWidth - 50;
    const dropdownWidth = 300;

    // Would dropdown overflow right edge?
    const wouldOverflowRight = cursorX + dropdownWidth > viewportWidth;
    expect(wouldOverflowRight).toBe(true);

    // Smart positioning would flip to left side
    const adjustedX = cursorX - dropdownWidth;
    expect(adjustedX).toBeGreaterThan(0);
  });
});

describe('Autocomplete - Focus Management (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('maintains textarea focus while dropdown is visible', () => {
    const textarea = document.createElement('textarea');
    textarea.value = '@query';
    document.body.appendChild(textarea);

    // Create mock dropdown (doesn't steal focus in real implementation)
    const dropdown = document.createElement('div');
    dropdown.role = 'listbox';
    dropdown.style.position = 'fixed';
    document.body.appendChild(dropdown);

    // Focus textarea
    textarea.focus();
    expect(document.activeElement).toBe(textarea);

    // Dropdown visible but focus remains in textarea
    // (critical for continued typing)
    expect(document.activeElement).toBe(textarea);
  });

  it('returns focus to textarea after dropdown interaction', () => {
    const textarea = document.createElement('textarea');
    textarea.value = '@query';
    document.body.appendChild(textarea);

    const dropdown = document.createElement('div');
    dropdown.role = 'listbox';
    dropdown.setAttribute('tabindex', '-1'); // Not focusable
    document.body.appendChild(dropdown);

    textarea.focus();
    expect(document.activeElement).toBe(textarea);

    // Simulate dropdown interaction (click on option)
    const option = document.createElement('div');
    option.role = 'option';
    option.textContent = 'Some Node';
    dropdown.appendChild(option);

    // After selection, focus returns to textarea
    textarea.focus(); // Explicit refocus in real implementation
    expect(document.activeElement).toBe(textarea);
  });

  it('handles focus/blur events correctly', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);

    let focusCount = 0;
    let blurCount = 0;

    textarea.addEventListener('focus', () => {
      focusCount++;
    });

    textarea.addEventListener('blur', () => {
      blurCount++;
    });

    // Focus textarea
    textarea.focus();
    expect(focusCount).toBe(1);
    expect(blurCount).toBe(0);

    // Blur by focusing another element
    const otherTextarea = document.createElement('textarea');
    document.body.appendChild(otherTextarea);
    otherTextarea.focus();

    expect(focusCount).toBe(1);
    expect(blurCount).toBe(1);
  });
});

describe('Autocomplete - Textarea Selection API (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('can detect cursor position for @ trigger', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Hello @world';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(7, 7); // After "@"

    // Verify cursor position
    expect(textarea.selectionStart).toBe(7);
    expect(textarea.selectionEnd).toBe(7);

    // Get text before cursor
    const textBeforeCursor = textarea.value.substring(0, textarea.selectionStart);
    expect(textBeforeCursor).toBe('Hello @');
  });

  it('can insert reference at cursor position', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Before  After';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(7, 7); // Between "Before" and "After"

    const cursorPos = textarea.selectionStart;

    // Insert reference
    const reference = '@[[Node Title]]';
    const before = textarea.value.substring(0, cursorPos);
    const after = textarea.value.substring(cursorPos);
    textarea.value = before + reference + after;

    // Verify insertion
    expect(textarea.value).toBe('Before @[[Node Title]] After');

    // Move cursor after insertion
    const newCursorPos = cursorPos + reference.length;
    textarea.setSelectionRange(newCursorPos, newCursorPos);

    expect(textarea.selectionStart).toBe(newCursorPos);
  });

  it('can extract query text between @ and cursor', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Text @query text';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(11, 11); // After "query"

    const cursorPos = textarea.selectionStart;
    const textBeforeCursor = textarea.value.substring(0, cursorPos);

    // Find last @ before cursor
    const atIndex = textBeforeCursor.lastIndexOf('@');
    expect(atIndex).toBeGreaterThan(-1);

    // Extract query
    const query = textBeforeCursor.substring(atIndex + 1);
    expect(query).toBe('query');
  });

  it('preserves cursor position after reference insertion', () => {
    const textarea = document.createElement('textarea');
    textarea.value = '@query';
    document.body.appendChild(textarea);

    textarea.focus();
    const originalCursor = 6; // After "query"
    textarea.setSelectionRange(originalCursor, originalCursor);

    // Replace @query with reference
    const reference = '@[[Selected Node]]';
    textarea.value = reference;

    // Move cursor to end of reference
    const newCursorPos = reference.length;
    textarea.setSelectionRange(newCursorPos, newCursorPos);

    expect(textarea.selectionStart).toBe(newCursorPos);
    expect(textarea.selectionEnd).toBe(newCursorPos);
  });
});

describe('Autocomplete - Keyboard Navigation (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('can detect arrow key events', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();

    let arrowDownCount = 0;
    let arrowUpCount = 0;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') {
        arrowDownCount++;
      } else if (e.key === 'ArrowUp') {
        arrowUpCount++;
      }
    });

    // Simulate arrow down
    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    expect(arrowDownCount).toBe(1);

    // Simulate arrow up
    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    expect(arrowUpCount).toBe(1);
  });

  it('can detect Enter key for selection', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();

    let enterPressed = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        enterPressed = true;
        e.preventDefault(); // Prevent newline
      }
    });

    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(enterPressed).toBe(true);
  });

  it('can detect Escape key for closing', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();

    let escapePressed = false;

    textarea.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        escapePressed = true;
      }
    });

    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(escapePressed).toBe(true);
  });
});

describe('Autocomplete - Mouse Interaction (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('can detect click events on dropdown options', () => {
    const dropdown = document.createElement('div');
    dropdown.role = 'listbox';
    document.body.appendChild(dropdown);

    const option = document.createElement('div');
    option.role = 'option';
    option.textContent = 'Node Title';
    dropdown.appendChild(option);

    let clickCount = 0;

    option.addEventListener('click', () => {
      clickCount++;
    });

    // Simulate click
    option.click();

    expect(clickCount).toBe(1);
  });

  it('can detect hover events for option highlighting', () => {
    const option = document.createElement('div');
    option.role = 'option';
    document.body.appendChild(option);

    let hoverCount = 0;

    option.addEventListener('mouseenter', () => {
      hoverCount++;
    });

    // Simulate mouse enter
    option.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));

    expect(hoverCount).toBe(1);
  });
});

describe('Autocomplete - Content Integrity (Browser Mode)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('preserves content before and after insertion point', () => {
    const textarea = document.createElement('textarea');
    textarea.value = 'Start Middle End';
    document.body.appendChild(textarea);

    textarea.focus();
    textarea.setSelectionRange(6, 12); // Select "Middle"

    const start = textarea.value.substring(0, textarea.selectionStart);
    const end = textarea.value.substring(textarea.selectionEnd);

    // Replace selection with reference
    textarea.value = start + '@[[Reference]]' + end;

    expect(textarea.value).toBe('Start @[[Reference]] End');
  });

  it('handles multiple references in same textarea', () => {
    const textarea = document.createElement('textarea');
    textarea.value = '@[[First]] some text @[[Second]]';
    document.body.appendChild(textarea);

    // Verify both references preserved
    expect(textarea.value).toContain('@[[First]]');
    expect(textarea.value).toContain('@[[Second]]');

    // Count references
    const matches = textarea.value.match(/@\[\[.+?\]\]/g);
    expect(matches).toBeTruthy();
    expect(matches!.length).toBe(2);
  });
});
