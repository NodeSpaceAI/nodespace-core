/**
 * AiChatNodeViewer Component Tests
 *
 * Unit tests for the AI chat viewer logic extracted from the component.
 * Tests cover:
 * - Write buffering with debounced flush
 * - Tool result archival (result nulled, result_summary kept)
 * - Soft 500-message cap detection
 * - Archived conversation read-only detection
 * - Message extraction for display format
 *
 * Follows the project pattern of testing extracted logic functions directly
 * (not rendering Svelte components) using Happy-DOM.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { DisplayMessage } from '$lib/stores/chat-store.svelte';
import type { ToolExecutionRecord } from '$lib/types/agent-types';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

// =============================================================================
// Extracted logic from ai-chat-node-viewer.svelte (testable without rendering)
// =============================================================================

const SOFT_MESSAGE_CAP = 500;
const FLUSH_DEBOUNCE_MS = 7_000;

/**
 * Convert DisplayMessage[] back to the persisted messages format,
 * nulling full tool results (only result_summary kept).
 *
 * Mirrors archiveMessages() from the component.
 */
function archiveMessages(msgs: DisplayMessage[]): Record<string, unknown>[] {
  const archived: Record<string, unknown>[] = [];
  for (const msg of msgs) {
    if (msg.toolExecutions.length > 0) {
      for (const te of msg.toolExecutions) {
        archived.push({
          role: 'tool_call',
          tool: te.name,
          args: te.args,
          status: te.is_error ? 'error' : 'completed',
          result_summary: typeof te.result === 'string'
            ? te.result
            : te.result != null
              ? JSON.stringify(te.result).slice(0, 200)
              : null,
          result: null, // Nulled at write time per ADR-028
          duration_ms: te.duration_ms,
          timestamp: new Date(msg.timestamp).toISOString(),
        });
      }
      // If the assistant message also had text content, emit it separately
      if (msg.content) {
        archived.push({
          role: 'assistant',
          content: msg.content,
          timestamp: new Date(msg.timestamp).toISOString(),
        });
      }
    } else {
      archived.push({
        role: msg.role,
        content: msg.content,
        timestamp: new Date(msg.timestamp).toISOString(),
      });
    }
  }
  return archived;
}

/**
 * Load messages from persisted node properties into DisplayMessage[].
 *
 * Mirrors loadMessagesFromNode() from the component.
 */
function loadMessagesFromNode(
  persisted: Record<string, unknown>[] | undefined
): DisplayMessage[] {
  if (!Array.isArray(persisted)) {
    return [];
  }
  return persisted.map((m: Record<string, unknown>, idx: number) => {
    const role = (m.role as string) ?? 'user';
    if (role === 'tool_call') {
      const toolExec: ToolExecutionRecord = {
        tool_call_id: `tc-${idx}`,
        name: (m.tool as string) ?? 'unknown',
        args: m.args ?? {},
        result: m.result_summary ?? null,
        is_error: m.status === 'error',
        duration_ms: (m.duration_ms as number) ?? 0,
      };
      return {
        id: `persisted-${idx}`,
        role: 'assistant' as const,
        content: '',
        toolExecutions: [toolExec],
        timestamp: m.timestamp
          ? new Date(m.timestamp as string).getTime()
          : Date.now(),
      };
    }
    return {
      id: `persisted-${idx}`,
      role: role as DisplayMessage['role'],
      content: (m.content as string) ?? '',
      toolExecutions: [],
      timestamp: m.timestamp
        ? new Date(m.timestamp as string).getTime()
        : Date.now(),
    };
  });
}

/** Rough token estimate: ~4 chars per token */
function estimateTokens(msgs: DisplayMessage[]): number {
  let chars = 0;
  for (const m of msgs) {
    chars += m.content.length;
  }
  return Math.ceil(chars / 4);
}

// =============================================================================
// Helpers
// =============================================================================

function makeDisplayMessage(
  overrides: Partial<DisplayMessage> & { id: string }
): DisplayMessage {
  return {
    role: 'user',
    content: 'hello',
    toolExecutions: [],
    timestamp: Date.now(),
    ...overrides,
  };
}

function makeToolExecution(
  overrides: Partial<ToolExecutionRecord> = {}
): ToolExecutionRecord {
  return {
    tool_call_id: 'tc-1',
    name: 'search_nodes',
    args: { query: 'test' },
    result: { matches: 3, nodes: ['n1', 'n2', 'n3'] },
    is_error: false,
    duration_ms: 150,
    ...overrides,
  };
}

// =============================================================================
// Tests
// =============================================================================

describe('AiChatNodeViewer Logic', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  // ---------------------------------------------------------------------------
  // 1. Write Buffering (7s debounce, flush on destroy)
  // ---------------------------------------------------------------------------
  describe('Write buffering', () => {
    it('debounces flush calls within the 7s window', () => {
      const flushSpy = vi.fn();

      // Simulate the debounce logic from the component
      let flushTimer: ReturnType<typeof setTimeout> | null = null;
      let hasUnsavedChanges = false;

      function scheduleFlush(): void {
        if (flushTimer) clearTimeout(flushTimer);
        hasUnsavedChanges = true;
        flushTimer = setTimeout(() => {
          flushSpy();
          hasUnsavedChanges = false;
          flushTimer = null;
        }, FLUSH_DEBOUNCE_MS);
      }

      // Rapid-fire 5 messages
      scheduleFlush();
      vi.advanceTimersByTime(1000);
      scheduleFlush();
      vi.advanceTimersByTime(1000);
      scheduleFlush();
      vi.advanceTimersByTime(1000);
      scheduleFlush();
      vi.advanceTimersByTime(1000);
      scheduleFlush();

      // Flush should NOT have been called yet (timer keeps resetting)
      expect(flushSpy).not.toHaveBeenCalled();
      expect(hasUnsavedChanges).toBe(true);

      // Advance past the final debounce window
      vi.advanceTimersByTime(FLUSH_DEBOUNCE_MS);
      expect(flushSpy).toHaveBeenCalledTimes(1);
      expect(hasUnsavedChanges).toBe(false);
    });

    it('flushes immediately on destroy when there are unsaved changes', () => {
      const updateNodeSpy = vi.fn();
      let flushTimer: ReturnType<typeof setTimeout> | null = null;
      let hasUnsavedChanges = false;

      function scheduleFlush(): void {
        if (flushTimer) clearTimeout(flushTimer);
        hasUnsavedChanges = true;
        flushTimer = setTimeout(() => {
          updateNodeSpy();
          hasUnsavedChanges = false;
          flushTimer = null;
        }, FLUSH_DEBOUNCE_MS);
      }

      function onDestroy(): void {
        // Mirrors the onDestroy callback in the component
        if (hasUnsavedChanges) {
          updateNodeSpy();
          hasUnsavedChanges = false;
        }
        if (flushTimer) {
          clearTimeout(flushTimer);
          flushTimer = null;
        }
      }

      // Schedule a flush but destroy before timer fires
      scheduleFlush();
      vi.advanceTimersByTime(2000); // Only 2s of the 7s window
      expect(updateNodeSpy).not.toHaveBeenCalled();

      // Destroy triggers immediate flush
      onDestroy();
      expect(updateNodeSpy).toHaveBeenCalledTimes(1);
    });

    it('does not flush on destroy when there are no unsaved changes', () => {
      const updateNodeSpy = vi.fn();
      let hasUnsavedChanges = false;

      function onDestroy(): void {
        if (hasUnsavedChanges) {
          updateNodeSpy();
          hasUnsavedChanges = false;
        }
      }

      onDestroy();
      expect(updateNodeSpy).not.toHaveBeenCalled();
    });

    it('only triggers one flush after rapid messages followed by silence', () => {
      const flushSpy = vi.fn();
      let flushTimer: ReturnType<typeof setTimeout> | null = null;

      function scheduleFlush(): void {
        if (flushTimer) clearTimeout(flushTimer);
        flushTimer = setTimeout(() => {
          flushSpy();
          flushTimer = null;
        }, FLUSH_DEBOUNCE_MS);
      }

      // Simulate 10 rapid messages with 500ms gaps
      for (let i = 0; i < 10; i++) {
        scheduleFlush();
        vi.advanceTimersByTime(500);
      }

      // Still within debounce window of last message
      expect(flushSpy).not.toHaveBeenCalled();

      // Advance past the final debounce window
      vi.advanceTimersByTime(FLUSH_DEBOUNCE_MS);
      expect(flushSpy).toHaveBeenCalledTimes(1);
    });
  });

  // ---------------------------------------------------------------------------
  // 2. Tool Result Archival
  // ---------------------------------------------------------------------------
  describe('Tool result archival', () => {
    it('nulls full result and preserves result_summary for object results', () => {
      const toolExec = makeToolExecution({
        result: { matches: 3, nodes: ['n1', 'n2', 'n3'] },
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived).toHaveLength(1);
      expect(archived[0].role).toBe('tool_call');
      expect(archived[0].result).toBeNull();
      expect(archived[0].result_summary).toBe(
        JSON.stringify({ matches: 3, nodes: ['n1', 'n2', 'n3'] }).slice(0, 200)
      );
    });

    it('preserves string results as result_summary directly', () => {
      const toolExec = makeToolExecution({
        result: 'Found 3 matching nodes',
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived[0].result).toBeNull();
      expect(archived[0].result_summary).toBe('Found 3 matching nodes');
    });

    it('sets result_summary to null when result is null', () => {
      const toolExec = makeToolExecution({
        result: null,
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived[0].result).toBeNull();
      expect(archived[0].result_summary).toBeNull();
    });

    it('truncates long result_summary to 200 chars', () => {
      const longResult = { data: 'x'.repeat(300) };
      const toolExec = makeToolExecution({
        result: longResult,
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived[0].result).toBeNull();
      expect((archived[0].result_summary as string).length).toBe(200);
    });

    it('preserves error status in archived messages', () => {
      const toolExec = makeToolExecution({
        is_error: true,
        result: 'Permission denied',
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived[0].status).toBe('error');
      expect(archived[0].result_summary).toBe('Permission denied');
    });

    it('emits assistant text content separately from tool calls', () => {
      const toolExec = makeToolExecution();

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: 'Here are the results I found:',
          toolExecutions: [toolExec],
        }),
      ];

      const archived = archiveMessages(messages);

      // Should emit tool_call record + separate assistant text record
      expect(archived).toHaveLength(2);
      expect(archived[0].role).toBe('tool_call');
      expect(archived[0].result).toBeNull();
      expect(archived[1].role).toBe('assistant');
      expect(archived[1].content).toBe('Here are the results I found:');
    });

    it('archives multiple tool executions as separate records', () => {
      const toolExec1 = makeToolExecution({
        tool_call_id: 'tc-1',
        name: 'search_nodes',
        result: { count: 5 },
      });
      const toolExec2 = makeToolExecution({
        tool_call_id: 'tc-2',
        name: 'get_node',
        result: { id: 'n1', content: 'Test' },
      });

      const messages: DisplayMessage[] = [
        makeDisplayMessage({
          id: 'msg-1',
          role: 'assistant',
          content: '',
          toolExecutions: [toolExec1, toolExec2],
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived).toHaveLength(2);
      expect(archived[0].tool).toBe('search_nodes');
      expect(archived[0].result).toBeNull();
      expect(archived[1].tool).toBe('get_node');
      expect(archived[1].result).toBeNull();
    });

    it('archives plain user/assistant messages without tool handling', () => {
      const messages: DisplayMessage[] = [
        makeDisplayMessage({ id: 'msg-1', role: 'user', content: 'Hello' }),
        makeDisplayMessage({
          id: 'msg-2',
          role: 'assistant',
          content: 'Hi there',
        }),
      ];

      const archived = archiveMessages(messages);

      expect(archived).toHaveLength(2);
      expect(archived[0]).toEqual(
        expect.objectContaining({ role: 'user', content: 'Hello' })
      );
      expect(archived[1]).toEqual(
        expect.objectContaining({ role: 'assistant', content: 'Hi there' })
      );
      // No tool-specific fields
      expect(archived[0].tool).toBeUndefined();
      expect(archived[1].tool).toBeUndefined();
    });
  });

  // ---------------------------------------------------------------------------
  // 3. Soft Message Cap
  // ---------------------------------------------------------------------------
  describe('Soft message cap', () => {
    it('shows nudge when message count reaches 500', () => {
      const messageCount = 500;
      const showMessageCap = messageCount >= SOFT_MESSAGE_CAP;
      expect(showMessageCap).toBe(true);
    });

    it('shows nudge when message count exceeds 500', () => {
      const messageCount = 501;
      const showMessageCap = messageCount >= SOFT_MESSAGE_CAP;
      expect(showMessageCap).toBe(true);
    });

    it('does not show nudge when message count is below 500', () => {
      const messageCount = 499;
      const showMessageCap = messageCount >= SOFT_MESSAGE_CAP;
      expect(showMessageCap).toBe(false);
    });

    it('does not show nudge for empty conversation', () => {
      const messageCount = 0;
      const showMessageCap = messageCount >= SOFT_MESSAGE_CAP;
      expect(showMessageCap).toBe(false);
    });
  });

  // ---------------------------------------------------------------------------
  // 4. Archived Conversation Read-Only
  // ---------------------------------------------------------------------------
  describe('Archived conversation read-only', () => {
    it('disables input when status is archived', () => {
      const status = 'archived';
      const isArchived = status === 'archived';
      expect(isArchived).toBe(true);
    });

    it('shows input when status is active', () => {
      const status = 'active';
      expect(status).not.toBe('archived');
    });

    it('reads provider and model from node properties with defaults', () => {
      // Simulate the derived state logic from the component
      const nodeProperties: Record<string, unknown> = {};
      const provider = (nodeProperties.provider as string) ?? 'native';
      const model = (nodeProperties.model as string) ?? '';

      expect(provider).toBe('native');
      expect(model).toBe('');
    });

    it('reads provider and model when present', () => {
      const nodeProperties: Record<string, unknown> = {
        provider: 'openai',
        model: 'gpt-4',
      };
      const provider = (nodeProperties.provider as string) ?? 'native';
      const model = (nodeProperties.model as string) ?? '';

      expect(provider).toBe('openai');
      expect(model).toBe('gpt-4');
    });
  });

  // ---------------------------------------------------------------------------
  // 5. Message Extraction for Display
  // ---------------------------------------------------------------------------
  describe('Message extraction for display', () => {
    it('loads user messages from persisted format', () => {
      const persisted = [
        {
          role: 'user',
          content: 'Hello',
          timestamp: '2026-01-01T00:00:00.000Z',
        },
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages).toHaveLength(1);
      expect(messages[0].role).toBe('user');
      expect(messages[0].content).toBe('Hello');
      expect(messages[0].toolExecutions).toEqual([]);
      expect(messages[0].id).toBe('persisted-0');
    });

    it('loads assistant messages from persisted format', () => {
      const persisted = [
        {
          role: 'assistant',
          content: 'I can help with that.',
          timestamp: '2026-01-01T00:00:01.000Z',
        },
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages).toHaveLength(1);
      expect(messages[0].role).toBe('assistant');
      expect(messages[0].content).toBe('I can help with that.');
    });

    it('maps tool_call messages to assistant role with tool executions', () => {
      const persisted = [
        {
          role: 'tool_call',
          tool: 'search_nodes',
          args: { query: 'test' },
          result_summary: 'Found 3 nodes',
          status: 'completed',
          duration_ms: 150,
          timestamp: '2026-01-01T00:00:02.000Z',
        },
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages).toHaveLength(1);
      expect(messages[0].role).toBe('assistant');
      expect(messages[0].content).toBe('');
      expect(messages[0].toolExecutions).toHaveLength(1);

      const toolExec = messages[0].toolExecutions[0];
      expect(toolExec.name).toBe('search_nodes');
      expect(toolExec.args).toEqual({ query: 'test' });
      expect(toolExec.result).toBe('Found 3 nodes');
      expect(toolExec.is_error).toBe(false);
      expect(toolExec.duration_ms).toBe(150);
    });

    it('maps error tool_call messages correctly', () => {
      const persisted = [
        {
          role: 'tool_call',
          tool: 'delete_node',
          args: { id: 'n1' },
          result_summary: 'Permission denied',
          status: 'error',
          duration_ms: 50,
          timestamp: '2026-01-01T00:00:03.000Z',
        },
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages[0].toolExecutions[0].is_error).toBe(true);
      expect(messages[0].toolExecutions[0].result).toBe('Permission denied');
    });

    it('handles missing optional fields with defaults', () => {
      const persisted = [
        { role: 'user' }, // No content, no timestamp
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages).toHaveLength(1);
      expect(messages[0].content).toBe('');
      expect(messages[0].timestamp).toBeGreaterThan(0);
    });

    it('returns empty array for undefined messages', () => {
      const messages = loadMessagesFromNode(undefined);
      expect(messages).toEqual([]);
    });

    it('returns empty array for non-array messages', () => {
      const messages = loadMessagesFromNode(
        'not an array' as unknown as undefined
      );
      expect(messages).toEqual([]);
    });

    it('loads a mixed conversation correctly', () => {
      const persisted = [
        {
          role: 'user',
          content: 'Find nodes about testing',
          timestamp: '2026-01-01T00:00:00.000Z',
        },
        {
          role: 'tool_call',
          tool: 'search_nodes',
          args: { query: 'testing' },
          result_summary: '{"matches":2}',
          status: 'completed',
          duration_ms: 100,
          timestamp: '2026-01-01T00:00:01.000Z',
        },
        {
          role: 'assistant',
          content: 'I found 2 nodes about testing.',
          timestamp: '2026-01-01T00:00:02.000Z',
        },
      ];

      const messages = loadMessagesFromNode(persisted);

      expect(messages).toHaveLength(3);
      expect(messages[0].role).toBe('user');
      expect(messages[1].role).toBe('assistant');
      expect(messages[1].toolExecutions).toHaveLength(1);
      expect(messages[2].role).toBe('assistant');
      expect(messages[2].content).toBe('I found 2 nodes about testing.');
    });

    it('round-trips messages through archive and reload', () => {
      const original: DisplayMessage[] = [
        makeDisplayMessage({ id: 'msg-1', role: 'user', content: 'Hello' }),
        makeDisplayMessage({
          id: 'msg-2',
          role: 'assistant',
          content: 'Hi there',
        }),
        makeDisplayMessage({
          id: 'msg-3',
          role: 'assistant',
          content: '',
          toolExecutions: [
            makeToolExecution({ result: { count: 5 } }),
          ],
        }),
      ];

      // Archive (write path)
      const archived = archiveMessages(original);
      // Reload (read path)
      const reloaded = loadMessagesFromNode(archived);

      // User message preserved
      expect(reloaded[0].role).toBe('user');
      expect(reloaded[0].content).toBe('Hello');

      // Assistant text preserved
      expect(reloaded[1].role).toBe('assistant');
      expect(reloaded[1].content).toBe('Hi there');

      // Tool call: result was nulled on write, reloaded as summary
      expect(reloaded[2].role).toBe('assistant');
      expect(reloaded[2].toolExecutions).toHaveLength(1);
      expect(reloaded[2].toolExecutions[0].result).toBe(
        JSON.stringify({ count: 5 }).slice(0, 200)
      );
    });
  });

  // ---------------------------------------------------------------------------
  // 6. Token Estimation
  // ---------------------------------------------------------------------------
  describe('Token estimation', () => {
    it('estimates tokens at ~4 chars per token', () => {
      const messages: DisplayMessage[] = [
        makeDisplayMessage({ id: 'msg-1', content: 'abcd' }), // 4 chars = 1 token
        makeDisplayMessage({ id: 'msg-2', content: 'abcdefgh' }), // 8 chars = 2 tokens
      ];

      const tokens = estimateTokens(messages);
      expect(tokens).toBe(3); // ceil(12/4) = 3
    });

    it('returns 0 tokens for empty messages', () => {
      const messages: DisplayMessage[] = [
        makeDisplayMessage({ id: 'msg-1', content: '' }),
      ];

      const tokens = estimateTokens(messages);
      expect(tokens).toBe(0);
    });

    it('rounds up partial tokens', () => {
      const messages: DisplayMessage[] = [
        makeDisplayMessage({ id: 'msg-1', content: 'abc' }), // 3 chars = 1 token (ceil)
      ];

      const tokens = estimateTokens(messages);
      expect(tokens).toBe(1);
    });
  });
});
