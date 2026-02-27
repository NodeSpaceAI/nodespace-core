import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { statusBar, statusBarVisible } from '$lib/stores/status-bar';

describe('Status Bar Store', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Reset to initial state
    statusBar.clearMessage();
    statusBar.setEnabled(true);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('initial state', () => {
    it('should start enabled with empty message and info type', () => {
      const state = get(statusBar);
      expect(state.enabled).toBe(true);
      expect(state.message).toBe('');
      expect(state.type).toBe('info');
      expect(state.progress).toBeUndefined();
    });
  });

  describe('toggle', () => {
    it('should flip enabled state', () => {
      statusBar.toggle();
      expect(get(statusBar).enabled).toBe(false);

      statusBar.toggle();
      expect(get(statusBar).enabled).toBe(true);
    });
  });

  describe('setEnabled', () => {
    it('should set enabled to false', () => {
      statusBar.setEnabled(false);
      expect(get(statusBar).enabled).toBe(false);
    });

    it('should set enabled to true', () => {
      statusBar.setEnabled(false);
      statusBar.setEnabled(true);
      expect(get(statusBar).enabled).toBe(true);
    });
  });

  describe('show', () => {
    it('should set message and info type', () => {
      statusBar.show('Loading...');
      const state = get(statusBar);
      expect(state.message).toBe('Loading...');
      expect(state.type).toBe('info');
      expect(state.progress).toBeUndefined();
    });

    it('should set message with progress', () => {
      statusBar.show('Importing...', 50);
      const state = get(statusBar);
      expect(state.message).toBe('Importing...');
      expect(state.progress).toBe(50);
      expect(state.type).toBe('info');
    });
  });

  describe('success', () => {
    it('should set success type and message', () => {
      statusBar.success('Done!');
      const state = get(statusBar);
      expect(state.message).toBe('Done!');
      expect(state.type).toBe('success');
      expect(state.progress).toBeUndefined();
    });

    it('should auto-clear message after 5 seconds', () => {
      statusBar.success('Done!');
      expect(get(statusBar).message).toBe('Done!');

      vi.advanceTimersByTime(5000);

      const state = get(statusBar);
      expect(state.message).toBe('');
      expect(state.type).toBe('info');
    });

    it('should cancel previous timer when called again', () => {
      statusBar.success('First');
      vi.advanceTimersByTime(3000);

      statusBar.success('Second');
      expect(get(statusBar).message).toBe('Second');

      vi.advanceTimersByTime(3000);
      // First timer would have fired at 5s, but was cancelled
      expect(get(statusBar).message).toBe('Second');

      vi.advanceTimersByTime(2000);
      // Second timer fires at 5s after it was set
      expect(get(statusBar).message).toBe('');
    });
  });

  describe('error', () => {
    it('should set error type and persist', () => {
      statusBar.error('Something went wrong');
      const state = get(statusBar);
      expect(state.message).toBe('Something went wrong');
      expect(state.type).toBe('error');
      expect(state.progress).toBeUndefined();
    });

    it('should not auto-clear', () => {
      statusBar.error('Persistent error');
      vi.advanceTimersByTime(10000);

      expect(get(statusBar).message).toBe('Persistent error');
      expect(get(statusBar).type).toBe('error');
    });
  });

  describe('updateProgress', () => {
    it('should calculate percentage from current/total', () => {
      statusBar.show('Processing...');
      statusBar.updateProgress(25, 100);
      expect(get(statusBar).progress).toBe(25);
    });

    it('should round percentage', () => {
      statusBar.updateProgress(1, 3);
      expect(get(statusBar).progress).toBe(33);
    });

    it('should optionally update message', () => {
      statusBar.show('Old message');
      statusBar.updateProgress(5, 10, 'New message');
      const state = get(statusBar);
      expect(state.message).toBe('New message');
      expect(state.progress).toBe(50);
    });

    it('should keep existing message when no message provided', () => {
      statusBar.show('Keep me');
      statusBar.updateProgress(5, 10);
      expect(get(statusBar).message).toBe('Keep me');
    });
  });

  describe('clearMessage', () => {
    it('should reset message, progress, and type', () => {
      statusBar.show('Busy', 75);
      statusBar.clearMessage();
      const state = get(statusBar);
      expect(state.message).toBe('');
      expect(state.progress).toBeUndefined();
      expect(state.type).toBe('info');
    });

    it('should not affect enabled state', () => {
      statusBar.setEnabled(false);
      statusBar.clearMessage();
      expect(get(statusBar).enabled).toBe(false);
    });
  });

  describe('statusBarVisible derived store', () => {
    it('should reflect enabled state', () => {
      expect(get(statusBarVisible)).toBe(true);

      statusBar.setEnabled(false);
      expect(get(statusBarVisible)).toBe(false);

      statusBar.setEnabled(true);
      expect(get(statusBarVisible)).toBe(true);
    });
  });
});
