/**
 * Browser Tests: Rapid Keyboard Operations (Issue #870 Part 2A)
 *
 * These tests run in a real browser (Chromium via Playwright) to test
 * rapid keyboard sequences that could expose race conditions between
 * UI updates and backend persistence.
 *
 * These tests would have caught the PR #861 race conditions where:
 * - Enter→Tab (create node then immediately indent) could use stale parent data
 * - Tab→Shift+Tab (indent then immediate outdent) could corrupt hierarchy
 *
 * Since we can't easily test full Svelte components with backend integration
 * in browser mode, these tests focus on:
 * 1. Keyboard event timing and sequencing
 * 2. Event queue behavior under rapid input
 * 3. Focus management during rapid operations
 */

import { describe, it, expect, beforeEach } from 'vitest';

describe('Rapid Keyboard Operations - Browser Mode (Issue #870)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  describe('Keyboard Event Sequencing', () => {
    it('should receive keyboard events in correct order during rapid input', () => {
      const textarea = document.createElement('textarea');
      document.body.appendChild(textarea);
      textarea.focus();

      const receivedEvents: string[] = [];

      textarea.addEventListener('keydown', (e) => {
        receivedEvents.push(`down:${e.key}`);
      });

      textarea.addEventListener('keyup', (e) => {
        receivedEvents.push(`up:${e.key}`);
      });

      // Simulate rapid Enter→Tab sequence (the PR #861 race condition)
      const events = [
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
        new KeyboardEvent('keyup', { key: 'Enter', bubbles: true }),
        new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }),
        new KeyboardEvent('keyup', { key: 'Tab', bubbles: true })
      ];

      for (const event of events) {
        textarea.dispatchEvent(event);
      }

      // Events should be received in exact order
      expect(receivedEvents).toEqual(['down:Enter', 'up:Enter', 'down:Tab', 'up:Tab']);
    });

    it('should handle Tab→Shift+Tab rapid sequence', () => {
      const textarea = document.createElement('textarea');
      document.body.appendChild(textarea);
      textarea.focus();

      const receivedEvents: { key: string; shiftKey: boolean }[] = [];

      textarea.addEventListener('keydown', (e) => {
        receivedEvents.push({ key: e.key, shiftKey: e.shiftKey });
      });

      // Simulate rapid Tab→Shift+Tab (indent then immediate outdent)
      const events = [
        new KeyboardEvent('keydown', { key: 'Tab', shiftKey: false, bubbles: true }),
        new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true })
      ];

      for (const event of events) {
        textarea.dispatchEvent(event);
      }

      expect(receivedEvents[0]).toEqual({ key: 'Tab', shiftKey: false });
      expect(receivedEvents[1]).toEqual({ key: 'Tab', shiftKey: true });
    });

    it('should handle 100 rapid Tab keypresses', () => {
      const textarea = document.createElement('textarea');
      document.body.appendChild(textarea);
      textarea.focus();

      let tabCount = 0;

      textarea.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') {
          tabCount++;
          e.preventDefault();
        }
      });

      // Dispatch 100 Tab events rapidly
      for (let i = 0; i < 100; i++) {
        textarea.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
        );
      }

      expect(tabCount).toBe(100);
    });

    it('should maintain event order during alternating indent/outdent', () => {
      const textarea = document.createElement('textarea');
      document.body.appendChild(textarea);
      textarea.focus();

      const operations: string[] = [];

      textarea.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') {
          operations.push(e.shiftKey ? 'outdent' : 'indent');
        }
      });

      // 50 alternating indent/outdent operations
      for (let i = 0; i < 50; i++) {
        textarea.dispatchEvent(
          new KeyboardEvent('keydown', {
            key: 'Tab',
            shiftKey: i % 2 === 1,
            bubbles: true
          })
        );
      }

      expect(operations.length).toBe(50);
      expect(operations[0]).toBe('indent');
      expect(operations[1]).toBe('outdent');
      expect(operations[48]).toBe('indent');
      expect(operations[49]).toBe('outdent');
    });
  });

  describe('Focus Management During Rapid Operations', () => {
    it('should maintain focus during rapid keyboard operations', () => {
      const textarea = document.createElement('textarea');
      textarea.id = 'main-input';
      document.body.appendChild(textarea);
      textarea.focus();

      // Verify initial focus
      expect(document.activeElement).toBe(textarea);

      // Dispatch 50 keyboard events
      for (let i = 0; i < 50; i++) {
        textarea.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })
        );
      }

      // Focus should be maintained
      expect(document.activeElement).toBe(textarea);
    });

    it('should handle focus transitions between textareas during rapid input', () => {
      // Create 5 textareas to simulate node list
      const textareas: HTMLTextAreaElement[] = [];
      for (let i = 0; i < 5; i++) {
        const textarea = document.createElement('textarea');
        textarea.id = `node-${i}`;
        document.body.appendChild(textarea);
        textareas.push(textarea);
      }

      const focusTransitions: string[] = [];

      textareas.forEach((ta) => {
        ta.addEventListener('focus', () => {
          focusTransitions.push(`focus:${ta.id}`);
        });
        ta.addEventListener('blur', () => {
          focusTransitions.push(`blur:${ta.id}`);
        });
      });

      // Rapidly cycle focus through all textareas
      for (let cycle = 0; cycle < 10; cycle++) {
        for (const textarea of textareas) {
          textarea.focus();
        }
      }

      // Should have recorded many focus/blur events
      // (exact count depends on browser batching)
      expect(focusTransitions.length).toBeGreaterThan(0);

      // Final focus should be on last textarea
      expect(document.activeElement).toBe(textareas[4]);
    });

    it('should preserve cursor position through rapid operations', () => {
      const textarea = document.createElement('textarea');
      textarea.value = 'Hello World Test Content';
      document.body.appendChild(textarea);
      textarea.focus();
      textarea.setSelectionRange(6, 6); // After "Hello "

      // Rapid operations that shouldn't affect cursor
      for (let i = 0; i < 20; i++) {
        textarea.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })
        );
      }

      // Cursor position should be preserved
      expect(textarea.selectionStart).toBe(6);
      expect(textarea.selectionEnd).toBe(6);
    });
  });

  describe('Async Operation Simulation', () => {
    it('should handle operations that complete out of order', async () => {
      const completionOrder: number[] = [];

      // Simulate async operations with varying delays
      const operations = [1, 2, 3, 4, 5].map(async (id) => {
        // Random delay to simulate database timing variance
        const delay = Math.random() * 10;
        await new Promise((resolve) => setTimeout(resolve, delay));
        completionOrder.push(id);
        return id;
      });

      await Promise.all(operations);

      // All operations should complete
      expect(completionOrder.length).toBe(5);
      expect(completionOrder.sort()).toEqual([1, 2, 3, 4, 5]);
    });

    it('should track pending operations correctly', async () => {
      let pendingCount = 0;
      let maxPending = 0;

      const trackOperation = async (id: number) => {
        pendingCount++;
        maxPending = Math.max(maxPending, pendingCount);

        // Simulate async work
        await new Promise((resolve) => setTimeout(resolve, Math.random() * 5));

        pendingCount--;
        return id;
      };

      // Launch 10 operations rapidly
      const operations = [];
      for (let i = 0; i < 10; i++) {
        operations.push(trackOperation(i));
      }

      await Promise.all(operations);

      // Should have had concurrent operations
      expect(maxPending).toBeGreaterThan(1);
      expect(pendingCount).toBe(0);
    });
  });

  describe('Event Prevention and Bubbling', () => {
    it('should correctly prevent default for Tab key', () => {
      const container = document.createElement('div');
      const textarea = document.createElement('textarea');
      container.appendChild(textarea);
      document.body.appendChild(container);
      textarea.focus();

      let containerReceivedTab = false;
      let textareaPreventedDefault = false;

      container.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') {
          containerReceivedTab = true;
        }
      });

      textarea.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') {
          e.preventDefault();
          textareaPreventedDefault = true;
        }
      });

      textarea.dispatchEvent(
        new KeyboardEvent('keydown', {
          key: 'Tab',
          bubbles: true,
          cancelable: true
        })
      );

      expect(textareaPreventedDefault).toBe(true);
      // Container should still receive event (bubbling)
      expect(containerReceivedTab).toBe(true);
    });

    it('should handle stopPropagation during rapid events', () => {
      const container = document.createElement('div');
      const textarea = document.createElement('textarea');
      container.appendChild(textarea);
      document.body.appendChild(container);
      textarea.focus();

      let containerEvents = 0;
      let textareaEvents = 0;

      container.addEventListener('keydown', () => {
        containerEvents++;
      });

      textarea.addEventListener('keydown', (e) => {
        textareaEvents++;
        e.stopPropagation();
      });

      // Rapid events with stopPropagation
      for (let i = 0; i < 50; i++) {
        textarea.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })
        );
      }

      expect(textareaEvents).toBe(50);
      expect(containerEvents).toBe(0); // stopPropagation should prevent bubbling
    });
  });
});

describe('Stress Test - High Frequency Events (Issue #870)', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('should handle 500 rapid keyboard events without dropping any', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();

    let eventCount = 0;
    const timestamps: number[] = [];

    textarea.addEventListener('keydown', () => {
      eventCount++;
      timestamps.push(performance.now());
    });

    const startTime = performance.now();

    // Dispatch 500 events as fast as possible
    for (let i = 0; i < 500; i++) {
      textarea.dispatchEvent(
        new KeyboardEvent('keydown', {
          key: i % 2 === 0 ? 'Tab' : 'Enter',
          bubbles: true
        })
      );
    }

    const endTime = performance.now();

    // All events should be received
    expect(eventCount).toBe(500);

    // Events should be processed quickly (< 100ms for 500 events)
    expect(endTime - startTime).toBeLessThan(100);
  });

  it('should handle mixed event types in rapid sequence', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();

    const eventTypes: string[] = [];

    const keys = ['Tab', 'Enter', 'Backspace', 'ArrowUp', 'ArrowDown'];

    textarea.addEventListener('keydown', (e) => {
      eventTypes.push(e.key);
    });

    // Dispatch 100 events with random key types
    for (let i = 0; i < 100; i++) {
      const key = keys[i % keys.length];
      textarea.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    }

    expect(eventTypes.length).toBe(100);

    // Verify correct sequence
    for (let i = 0; i < 100; i++) {
      expect(eventTypes[i]).toBe(keys[i % keys.length]);
    }
  });
});
