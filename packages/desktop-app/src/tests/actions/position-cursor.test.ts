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
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let rafSpy: any;

  beforeEach(() => {
    // Ensure requestAnimationFrame exists in test environment
    if (!globalThis.requestAnimationFrame) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      globalThis.requestAnimationFrame = ((cb: any) => {
        cb(0);
        return 0;
      }) as typeof requestAnimationFrame;
    }

    // Create textarea element
    textarea = document.createElement('textarea');
    textarea.value = 'Line 1\nLine 2\nLine 3';
    document.body.appendChild(textarea);

    // Create controller
    controller = new TextareaController(textarea, 'test-node', 'text', {
      contentChanged: vi.fn(),
      headerLevelChanged: vi.fn(),
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
    rafSpy = vi.spyOn(globalThis, 'requestAnimationFrame').mockImplementation((cb) => {
      cb(0);
      return 0;
    });
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
});
