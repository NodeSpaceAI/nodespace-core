/**
 * Unit tests for ChatStore - message management, streaming state, session lifecycle
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { chatStore } from '$lib/stores/chat-store.svelte';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

describe('ChatStore', () => {
  beforeEach(() => {
    chatStore.reset();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('Initial State', () => {
    it('starts with empty messages', () => {
      expect(chatStore.messages).toEqual([]);
    });

    it('starts not streaming', () => {
      expect(chatStore.isStreaming).toBe(false);
    });

    it('starts with no session', () => {
      expect(chatStore.currentSessionId).toBeNull();
    });

    it('starts with no error', () => {
      expect(chatStore.error).toBeNull();
    });
  });

  describe('createSession', () => {
    it('creates a new session with an ID', async () => {
      const sessionId = await chatStore.createSession();
      expect(sessionId).toBeTruthy();
      expect(sessionId).toMatch(/^session-/);
      expect(chatStore.currentSessionId).toBe(sessionId);
    });

    it('clears messages on new session', async () => {
      // Add a fake message first
      chatStore.messages = [
        {
          id: 'test',
          role: 'user',
          content: 'hello',
          toolExecutions: [],
          timestamp: Date.now(),
        },
      ];
      expect(chatStore.messages).toHaveLength(1);

      await chatStore.createSession();
      expect(chatStore.messages).toHaveLength(0);
    });

    it('clears error on new session', async () => {
      chatStore.error = 'some error';
      await chatStore.createSession();
      expect(chatStore.error).toBeNull();
    });
  });

  describe('sendMessage', () => {
    it('ignores empty messages', async () => {
      const promise = chatStore.sendMessage('   ');
      await promise;
      expect(chatStore.messages).toHaveLength(0);
    });

    it('auto-creates a session if none exists', async () => {
      expect(chatStore.currentSessionId).toBeNull();

      // Start sendMessage but don't await — it will be waiting on timers
      const sendPromise = chatStore.sendMessage('hello');

      // Advance all timers to let mock streaming complete
      await vi.runAllTimersAsync();
      await sendPromise;

      expect(chatStore.currentSessionId).toBeTruthy();
    });

    it('adds user message immediately', async () => {
      const sendPromise = chatStore.sendMessage('hello world');

      // After the message is added but before streaming completes
      // At minimum the user message should be there
      expect(chatStore.messages.length).toBeGreaterThanOrEqual(1);
      expect(chatStore.messages[0].role).toBe('user');
      expect(chatStore.messages[0].content).toBe('hello world');

      await vi.runAllTimersAsync();
      await sendPromise;
    });

    it('adds assistant message during streaming', async () => {
      const sendPromise = chatStore.sendMessage('hello');

      // Advance just enough for the streaming to start
      await vi.advanceTimersByTimeAsync(100);

      // Should have user + assistant messages
      expect(chatStore.messages.length).toBeGreaterThanOrEqual(2);
      expect(chatStore.messages[1].role).toBe('assistant');

      await vi.runAllTimersAsync();
      await sendPromise;
    });

    it('sets streaming state during response', async () => {
      const sendPromise = chatStore.sendMessage('hello');

      // Should be streaming after message send starts
      expect(chatStore.isStreaming).toBe(true);

      await vi.runAllTimersAsync();
      await sendPromise;

      expect(chatStore.isStreaming).toBe(false);
    });

    it('produces a non-empty assistant response', async () => {
      const sendPromise = chatStore.sendMessage('hello');
      await vi.runAllTimersAsync();
      await sendPromise;

      const assistantMessages = chatStore.messages.filter((m) => m.role === 'assistant');
      expect(assistantMessages).toHaveLength(1);
      expect(assistantMessages[0].content.length).toBeGreaterThan(0);
    });

    it('prevents sending while streaming', async () => {
      const sendPromise = chatStore.sendMessage('first');
      expect(chatStore.isStreaming).toBe(true);

      // Try to send another message while streaming
      await chatStore.sendMessage('second');

      // Should still only have the first set of messages
      const userMessages = chatStore.messages.filter((m) => m.role === 'user');
      expect(userMessages).toHaveLength(1);
      expect(userMessages[0].content).toBe('first');

      await vi.runAllTimersAsync();
      await sendPromise;
    });
  });

  describe('cancelStreaming', () => {
    it('stops streaming when cancelled', async () => {
      const sendPromise = chatStore.sendMessage('hello');
      expect(chatStore.isStreaming).toBe(true);

      chatStore.cancelStreaming();
      await vi.runAllTimersAsync();
      await sendPromise;

      expect(chatStore.isStreaming).toBe(false);
    });
  });

  describe('clearMessages', () => {
    it('removes all messages', async () => {
      const sendPromise = chatStore.sendMessage('hello');
      await vi.runAllTimersAsync();
      await sendPromise;

      expect(chatStore.messages.length).toBeGreaterThan(0);

      chatStore.clearMessages();
      expect(chatStore.messages).toHaveLength(0);
    });

    it('clears error state', () => {
      chatStore.error = 'test error';
      chatStore.clearMessages();
      expect(chatStore.error).toBeNull();
    });
  });

  describe('reset', () => {
    it('resets all state', async () => {
      await chatStore.createSession();
      const sendPromise = chatStore.sendMessage('hello');
      await vi.runAllTimersAsync();
      await sendPromise;

      chatStore.reset();

      expect(chatStore.messages).toHaveLength(0);
      expect(chatStore.isStreaming).toBe(false);
      expect(chatStore.currentSessionId).toBeNull();
      expect(chatStore.error).toBeNull();
    });
  });

  describe('Message IDs', () => {
    it('assigns unique IDs to messages', async () => {
      const sendPromise = chatStore.sendMessage('hello');
      await vi.runAllTimersAsync();
      await sendPromise;

      const ids = chatStore.messages.map((m) => m.id);
      const uniqueIds = new Set(ids);
      expect(uniqueIds.size).toBe(ids.length);
    });
  });

  describe('Timestamps', () => {
    it('includes timestamps on messages', async () => {
      const sendPromise = chatStore.sendMessage('hello');
      await vi.runAllTimersAsync();
      await sendPromise;

      for (const msg of chatStore.messages) {
        expect(msg.timestamp).toBeGreaterThan(0);
        expect(typeof msg.timestamp).toBe('number');
      }
    });
  });
});
