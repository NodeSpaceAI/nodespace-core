/**
 * Focus Management Browser Tests
 *
 * These tests run in a real browser (Chromium via Playwright) to test focus
 * management, blur/focus events, and interactions that require actual browser behavior.
 *
 * These tests were previously deleted because Happy-DOM cannot simulate real focus/blur events.
 */

import { describe, it, expect, beforeEach } from 'vitest';

describe('Focus Management - Browser Mode', () => {
  beforeEach(() => {
    // Clear the body before each test
    document.body.innerHTML = '';
  });

  it('verifies browser test setup is working', () => {
    // Simple smoke test to verify browser mode is configured correctly
    const body = document.body;
    expect(body).toBeDefined();
    expect(document).toBeDefined();
  });

  it('can create and focus a textarea element', () => {
    // Create a textarea programmatically
    const textarea = document.createElement('textarea');
    textarea.id = 'test-textarea';
    textarea.value = 'Test content';
    document.body.appendChild(textarea);

    // Focus the textarea
    textarea.focus();

    // Verify the textarea received focus using real browser focus APIs
    const isFocused = document.activeElement === textarea;
    expect(isFocused).toBe(true);
  });

  it('can blur and refocus elements', () => {
    // Create two textareas
    const textarea1 = document.createElement('textarea');
    textarea1.id = 'textarea-1';
    textarea1.value = 'First';
    document.body.appendChild(textarea1);

    const textarea2 = document.createElement('textarea');
    textarea2.id = 'textarea-2';
    textarea2.value = 'Second';
    document.body.appendChild(textarea2);

    // Focus first textarea
    textarea1.focus();
    let activeId = (document.activeElement as HTMLElement)?.id;
    expect(activeId).toBe('textarea-1');

    // Focus second textarea (should blur first)
    textarea2.focus();
    activeId = (document.activeElement as HTMLElement)?.id;
    expect(activeId).toBe('textarea-2');
  });

  it('can detect cursor position in focused textarea', () => {
    // Create textarea with content
    const textarea = document.createElement('textarea');
    textarea.id = 'cursor-test';
    textarea.value = 'Hello World';
    document.body.appendChild(textarea);

    // Focus and set cursor position to middle of text (after "Hello ")
    textarea.focus();
    textarea.setSelectionRange(6, 6);

    // Verify cursor position
    const cursorPosition = textarea.selectionStart;
    expect(cursorPosition).toBe(6);
  });
});

describe('Focus Management - Real World Scenarios', () => {
  beforeEach(() => {
    // Clear the body before each test
    document.body.innerHTML = '';
  });

  it('maintains cursor position after focus/blur cycle', () => {
    // Create two textareas
    const textarea1 = document.createElement('textarea');
    textarea1.id = 'main-textarea';
    textarea1.value = 'Main content here';
    document.body.appendChild(textarea1);

    const textarea2 = document.createElement('textarea');
    textarea2.id = 'other-textarea';
    textarea2.value = 'Other content';
    document.body.appendChild(textarea2);

    // Focus main textarea and set cursor position
    textarea1.focus();
    textarea1.setSelectionRange(5, 5); // After "Main "

    // Blur by focusing other textarea
    textarea2.focus();

    // Refocus main textarea
    textarea1.focus();

    // Chromium restores cursor position after focus/blur cycle
    // This is documented browser behavior we rely on for NodeSpace
    const cursorPosition = textarea1.selectionStart;
    expect(cursorPosition).toBe(5);
  });

  it('focus events fire correctly', () => {
    // Test that actual focus/blur events work (Happy-DOM can't do this)
    const textarea = document.createElement('textarea');
    textarea.id = 'event-test';
    document.body.appendChild(textarea);

    let focusCount = 0;
    let blurCount = 0;

    textarea.addEventListener('focus', () => {
      focusCount++;
    });

    textarea.addEventListener('blur', () => {
      blurCount++;
    });

    // Focus the textarea
    textarea.focus();
    expect(focusCount).toBe(1);
    expect(blurCount).toBe(0);

    // Blur by creating and focusing another element
    const anotherTextarea = document.createElement('textarea');
    document.body.appendChild(anotherTextarea);
    anotherTextarea.focus();

    expect(focusCount).toBe(1);
    expect(blurCount).toBe(1);
  });
});
