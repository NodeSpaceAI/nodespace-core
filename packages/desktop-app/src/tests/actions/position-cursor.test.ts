/**
 * Tests for positionCursor Svelte action
 *
 * Verifies reactive cursor positioning behavior with different cursor position types
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { positionCursor, type CursorPosition } from '$lib/actions/position-cursor';
import { TextareaController } from '$lib/design/components/textarea-controller';

describe('positionCursor action', () => {
  let textarea: HTMLTextAreaElement;
  let controller: TextareaController;
  // Type as Vitest mock instance - vi.spyOn returns a mock that can be restored
  let rafSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    // Ensure requestAnimationFrame exists in test environment
    if (!globalThis.requestAnimationFrame) {
      globalThis.requestAnimationFrame = ((cb: (time: number) => void) => {
        cb(0);
        return 0;
      }) as typeof requestAnimationFrame;
    }

    // Create textarea element
    textarea = document.createElement('textarea');
    textarea.value = 'Line 1\nLine 2\nLine 3';
    document.body.appendChild(textarea);

    // Create controller
    controller = new TextareaController(textarea, 'test-node', 'text', 'default', {
      contentChanged: vi.fn(),
      focus: vi.fn(),
      blur: vi.fn(),
      createNewNode: vi.fn(),
      indentNode: vi.fn(),
      outdentNode: vi.fn(),
      navigateArrow: vi.fn(),
      combineWithPrevious: vi.fn(),
      deleteNode: vi.fn(),
      directSlashCommand: vi.fn(),
      triggerDetected: vi.fn(),
      triggerHidden: vi.fn(),
      nodeReferenceSelected: vi.fn(),
      slashCommandDetected: vi.fn(),
      slashCommandHidden: vi.fn(),
      slashCommandSelected: vi.fn(),
      nodeTypeConversionDetected: vi.fn()
    });

    // Spy on requestAnimationFrame
    // Type cast required: vi.spyOn returns generic Mock type that doesn't expose mockRestore until runtime
    rafSpy = vi.spyOn(globalThis, 'requestAnimationFrame').mockImplementation((cb) => {
      cb(0);
      return 0;
    }) as ReturnType<typeof vi.fn>;
  });

  afterEach(() => {
    if (rafSpy) {
      rafSpy.mockRestore();
    }
    controller.destroy();
    document.body.removeChild(textarea);
  });

  it('should apply default cursor position', () => {
    const spy = vi.spyOn(controller, 'positionCursorAtLineBeginning');

    const data: CursorPosition = { type: 'default', skipSyntax: true };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(spy).toHaveBeenCalledWith(0, true);
  });

  it('should apply absolute cursor position', () => {
    const spy = vi.spyOn(controller, 'setCursorPosition');

    const data: CursorPosition = { type: 'absolute', position: 10 };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(spy).toHaveBeenCalledWith(10);
  });

  it('should apply arrow navigation cursor position', () => {
    const spy = vi.spyOn(controller, 'enterFromArrowNavigation');

    const data: CursorPosition = { type: 'arrow-navigation', direction: 'up', pixelOffset: 50 };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(spy).toHaveBeenCalledWith('up', 50);
  });

  it('should apply line-column cursor position', () => {
    const spy = vi.spyOn(controller, 'positionCursorAtLineBeginning');

    const data: CursorPosition = { type: 'line-column', line: 2, skipSyntax: false };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(spy).toHaveBeenCalledWith(2, false);
  });

  it('should skip positioning when data is null', () => {
    const spy = vi.spyOn(controller, 'setCursorPosition');

    positionCursor(textarea, { data: null, controller });

    expect(spy).not.toHaveBeenCalled();
  });

  it('should skip positioning when controller is null', () => {
    const data: CursorPosition = { type: 'absolute', position: 10 };

    // Should not throw
    expect(() => {
      positionCursor(textarea, { data, controller: null });
    }).not.toThrow();
  });

  it('should not re-apply same position (duplicate prevention)', () => {
    const spy = vi.spyOn(controller, 'setCursorPosition');
    const data: CursorPosition = { type: 'absolute', position: 10 };

    const action = positionCursor(textarea, { data, controller });

    expect(spy).toHaveBeenCalledTimes(1);

    // Update with same data
    action.update({ data, controller });

    // Should still be called only once (no duplicate)
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('should re-apply position after data becomes null', () => {
    const spy = vi.spyOn(controller, 'setCursorPosition');
    const data: CursorPosition = { type: 'absolute', position: 10 };

    const action = positionCursor(textarea, { data, controller });

    expect(spy).toHaveBeenCalledTimes(1);

    // Clear data
    action.update({ data: null, controller });

    // Re-apply same position
    action.update({ data, controller });

    // Should be called twice (re-application allowed after null)
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it('should handle different position types in updates', () => {
    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');
    const setLineSpy = vi.spyOn(controller, 'positionCursorAtLineBeginning');
    const arrowNavSpy = vi.spyOn(controller, 'enterFromArrowNavigation');

    const data1: CursorPosition = { type: 'absolute', position: 5 };
    const action = positionCursor(textarea, { data: data1, controller });

    expect(setCursorSpy).toHaveBeenCalledWith(5);

    // Update to different type
    const data2: CursorPosition = { type: 'line-column', line: 1, skipSyntax: true };
    action.update({ data: data2, controller });

    expect(setLineSpy).toHaveBeenCalledWith(1, true);

    // Update to arrow navigation
    const data3: CursorPosition = { type: 'arrow-navigation', direction: 'down', pixelOffset: 100 };
    action.update({ data: data3, controller });

    expect(arrowNavSpy).toHaveBeenCalledWith('down', 100);
  });

  it('should apply node-type-conversion cursor position', () => {
    const focusSpy = vi.spyOn(controller, 'focus');
    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');

    const data: CursorPosition = { type: 'node-type-conversion', position: 15 };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(focusSpy).toHaveBeenCalled();
    expect(setCursorSpy).toHaveBeenCalledWith(15);
  });

  it('should retry node-type-conversion if cursor position changes', async () => {
    vi.useFakeTimers();

    // Mock setCursorPosition to NOT actually change the cursor position
    // This simulates the scenario where a component switch resets the cursor
    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition').mockImplementation(() => {
      // Do nothing - simulates cursor being reset by component
    });

    const data: CursorPosition = { type: 'node-type-conversion', position: 20 };

    // Simulate textarea being focused with wrong cursor position
    textarea.focus();
    textarea.selectionStart = 5; // Different from target position
    textarea.selectionEnd = 5;

    positionCursor(textarea, { data, controller });

    // First call happens in RAF
    expect(setCursorSpy).toHaveBeenCalledWith(20);

    // Wait for retry timeout (10ms)
    vi.advanceTimersByTime(10);

    // Should retry because selectionStart (5) !== data.position (20)
    expect(setCursorSpy).toHaveBeenCalledTimes(2);
    expect(setCursorSpy).toHaveBeenCalledWith(20);

    vi.useRealTimers();
  });

  it('should not retry node-type-conversion if cursor position is correct', async () => {
    vi.useFakeTimers();

    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');
    const data: CursorPosition = { type: 'node-type-conversion', position: 20 };

    // Simulate textarea being focused with correct cursor position
    textarea.focus();
    textarea.selectionStart = 20; // Same as target position
    textarea.selectionEnd = 20;

    positionCursor(textarea, { data, controller });

    // First call happens in RAF
    expect(setCursorSpy).toHaveBeenCalledWith(20);

    // Wait for retry timeout (10ms)
    vi.advanceTimersByTime(10);

    // Should NOT retry because position is already correct
    expect(setCursorSpy).toHaveBeenCalledTimes(1);

    vi.useRealTimers();
  });

  it('should not retry node-type-conversion if element is not a textarea', async () => {
    vi.useFakeTimers();

    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');
    const data: CursorPosition = { type: 'node-type-conversion', position: 20 };

    // Simulate a non-textarea element being focused
    const div = document.createElement('div');
    div.focus();

    positionCursor(textarea, { data, controller });

    // First call happens in RAF
    expect(setCursorSpy).toHaveBeenCalledWith(20);

    // Wait for retry timeout (10ms)
    vi.advanceTimersByTime(10);

    // Should NOT retry because activeElement is not a textarea
    expect(setCursorSpy).toHaveBeenCalledTimes(1);

    vi.useRealTimers();
  });

  it('should apply inherited-type cursor position', () => {
    const focusSpy = vi.spyOn(controller, 'focus');
    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');

    const data: CursorPosition = { type: 'inherited-type', position: 8 };
    positionCursor(textarea, { data, controller });

    expect(rafSpy).toHaveBeenCalled();
    expect(focusSpy).toHaveBeenCalled();
    expect(setCursorSpy).toHaveBeenCalledWith(8);
  });

  it('should retry inherited-type if cursor position changes', async () => {
    vi.useFakeTimers();

    // Mock setCursorPosition to NOT actually change the cursor position
    // This simulates the scenario where a component switch resets the cursor
    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition').mockImplementation(() => {
      // Do nothing - simulates cursor being reset by component
    });

    const data: CursorPosition = { type: 'inherited-type', position: 12 };

    // Simulate textarea being focused with wrong cursor position
    textarea.focus();
    textarea.selectionStart = 3; // Different from target position
    textarea.selectionEnd = 3;

    positionCursor(textarea, { data, controller });

    // First call happens in RAF
    expect(setCursorSpy).toHaveBeenCalledWith(12);

    // Wait for retry timeout (10ms)
    vi.advanceTimersByTime(10);

    // Should retry because selectionStart (3) !== data.position (12)
    expect(setCursorSpy).toHaveBeenCalledTimes(2);
    expect(setCursorSpy).toHaveBeenCalledWith(12);

    vi.useRealTimers();
  });

  it('should not retry inherited-type if cursor position is correct', async () => {
    vi.useFakeTimers();

    const setCursorSpy = vi.spyOn(controller, 'setCursorPosition');
    const data: CursorPosition = { type: 'inherited-type', position: 12 };

    // Simulate textarea being focused with correct cursor position
    textarea.focus();
    textarea.selectionStart = 12; // Same as target position
    textarea.selectionEnd = 12;

    positionCursor(textarea, { data, controller });

    // First call happens in RAF
    expect(setCursorSpy).toHaveBeenCalledWith(12);

    // Wait for retry timeout (10ms)
    vi.advanceTimersByTime(10);

    // Should NOT retry because position is already correct
    expect(setCursorSpy).toHaveBeenCalledTimes(1);

    vi.useRealTimers();
  });

  it('should use skipSyntax default value (true) for default position', () => {
    const spy = vi.spyOn(controller, 'positionCursorAtLineBeginning');

    // Omit skipSyntax to test default
    const data: CursorPosition = { type: 'default' };
    positionCursor(textarea, { data, controller });

    // Should default to true
    expect(spy).toHaveBeenCalledWith(0, true);
  });

  it('should use skipSyntax default value (true) for line-column position', () => {
    const spy = vi.spyOn(controller, 'positionCursorAtLineBeginning');

    // Omit skipSyntax to test default
    const data: CursorPosition = { type: 'line-column', line: 1 };
    positionCursor(textarea, { data, controller });

    // Should default to true
    expect(spy).toHaveBeenCalledWith(1, true);
  });

  it('should handle skipSyntax: false explicitly for default position', () => {
    const spy = vi.spyOn(controller, 'positionCursorAtLineBeginning');

    const data: CursorPosition = { type: 'default', skipSyntax: false };
    positionCursor(textarea, { data, controller });

    expect(spy).toHaveBeenCalledWith(0, false);
  });

  it('should handle arrow navigation with down direction', () => {
    const spy = vi.spyOn(controller, 'enterFromArrowNavigation');

    const data: CursorPosition = { type: 'arrow-navigation', direction: 'down', pixelOffset: 75 };
    positionCursor(textarea, { data, controller });

    expect(spy).toHaveBeenCalledWith('down', 75);
  });
});
