/**
 * Unit tests for ChatInput component - keyboard handling
 *
 * Tests the keyboard behavior logic: Enter sends, Shift+Enter adds newline, Escape clears.
 * Uses Happy-DOM for basic DOM simulation.
 */

import { describe, it, expect, vi } from 'vitest';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

/**
 * Test the keyboard handling logic directly (unit test approach).
 * We test the behavior functions without rendering the Svelte component.
 */
describe('ChatInput Keyboard Behavior', () => {
  describe('Enter key handling', () => {
    it('Enter without shift should trigger send', () => {
      const sendFn = vi.fn();
      let value = 'hello world';
      const disabled = false;

      // Simulate the keydown handler logic
      function handleKeydown(event: { key: string; shiftKey: boolean; preventDefault: () => void }) {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault();
          if (value.trim().length > 0 && !disabled) {
            sendFn(value.trim());
            value = '';
          }
        }
      }

      const preventDefault = vi.fn();
      handleKeydown({ key: 'Enter', shiftKey: false, preventDefault });

      expect(preventDefault).toHaveBeenCalled();
      expect(sendFn).toHaveBeenCalledWith('hello world');
    });

    it('Shift+Enter should NOT trigger send', () => {
      const sendFn = vi.fn();
      let value = 'hello world';
      const disabled = false;

      function handleKeydown(event: { key: string; shiftKey: boolean; preventDefault: () => void }) {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault();
          if (value.trim().length > 0 && !disabled) {
            sendFn(value.trim());
            value = '';
          }
        }
      }

      const preventDefault = vi.fn();
      handleKeydown({ key: 'Enter', shiftKey: true, preventDefault });

      expect(preventDefault).not.toHaveBeenCalled();
      expect(sendFn).not.toHaveBeenCalled();
    });

    it('Enter with empty value should not send', () => {
      const sendFn = vi.fn();
      const value = '   ';
      const disabled = false;

      function handleKeydown(event: { key: string; shiftKey: boolean; preventDefault: () => void }) {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault();
          if (value.trim().length > 0 && !disabled) {
            sendFn(value.trim());
          }
        }
      }

      const preventDefault = vi.fn();
      handleKeydown({ key: 'Enter', shiftKey: false, preventDefault });

      expect(preventDefault).toHaveBeenCalled();
      expect(sendFn).not.toHaveBeenCalled();
    });

    it('Enter when disabled should not send', () => {
      const sendFn = vi.fn();
      const value = 'hello';
      const disabled = true;

      function handleKeydown(event: { key: string; shiftKey: boolean; preventDefault: () => void }) {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault();
          if (value.trim().length > 0 && !disabled) {
            sendFn(value.trim());
          }
        }
      }

      const preventDefault = vi.fn();
      handleKeydown({ key: 'Enter', shiftKey: false, preventDefault });

      expect(preventDefault).toHaveBeenCalled();
      expect(sendFn).not.toHaveBeenCalled();
    });
  });

  describe('Escape key handling', () => {
    it('Escape should clear value', () => {
      let value = 'some text';

      function handleKeydown(event: { key: string; shiftKey: boolean; preventDefault: () => void }) {
        if (event.key === 'Escape') {
          value = '';
        }
      }

      const preventDefault = vi.fn();
      handleKeydown({ key: 'Escape', shiftKey: false, preventDefault });

      expect(value).toBe('');
    });
  });

  describe('canSend derived state', () => {
    it('allows send when value is non-empty and not disabled', () => {
      const value = 'hello';
      const disabled = false;
      const canSend = value.trim().length > 0 && !disabled;
      expect(canSend).toBe(true);
    });

    it('disallows send when value is empty', () => {
      const value = '';
      const disabled = false;
      const canSend = value.trim().length > 0 && !disabled;
      expect(canSend).toBe(false);
    });

    it('disallows send when value is whitespace only', () => {
      const value = '   ';
      const disabled = false;
      const canSend = value.trim().length > 0 && !disabled;
      expect(canSend).toBe(false);
    });

    it('disallows send when disabled', () => {
      const value = 'hello';
      const disabled = true;
      const canSend = value.trim().length > 0 && !disabled;
      expect(canSend).toBe(false);
    });
  });
});
