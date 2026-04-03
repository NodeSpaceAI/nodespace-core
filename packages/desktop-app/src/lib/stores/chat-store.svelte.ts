/**
 * Chat Store - Manages chat conversation state using Svelte 5 runes.
 *
 * Provides mock streaming for development. Real Tauri integration
 * will be wired in #1008.
 */

import { createLogger } from '$lib/utils/logger';
import type { ChatMessage, ToolExecutionRecord } from '$lib/types/agent-types';

const log = createLogger('ChatStore');

/** Extended message type for UI display with tool executions and streaming state. */
export interface DisplayMessage {
  readonly id: string;
  readonly role: ChatMessage['role'];
  content: string;
  readonly toolExecutions: ToolExecutionRecord[];
  readonly timestamp: number;
}

/** Generate a simple unique ID. */
function generateId(): string {
  return `msg-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

const MOCK_RESPONSES = [
  'I can help you with that. NodeSpace uses a graph-based knowledge model where everything is a node connected by typed edges.',
  'Based on the context in your workspace, here is what I found. The schema system supports both built-in and custom property types.',
  'Let me look into that for you. The playbook engine processes event-driven workflows using graph traversal.',
  'That is an interesting question. The architecture uses a hybrid approach combining hardcoded behaviors with schema-driven extensions.',
];

const MOCK_TOOL_CALLS: ToolExecutionRecord[] = [
  {
    tool_call_id: 'tc-1',
    name: 'search_nodes',
    args: { query: 'schema validation', limit: 5 },
    result: { matches: 3, nodes: ['node-1', 'node-2', 'node-3'] },
    is_error: false,
    duration_ms: 142,
  },
];

class ChatStore {
  messages = $state<DisplayMessage[]>([]);
  isStreaming = $state(false);
  currentSessionId = $state<string | null>(null);
  error = $state<string | null>(null);

  private streamAbortController: AbortController | null = null;

  /** Send a user message and get a mock streaming response. */
  async sendMessage(content: string): Promise<void> {
    if (!content.trim()) return;
    if (this.isStreaming) {
      log.warn('Cannot send message while streaming');
      return;
    }

    this.error = null;

    // Ensure we have a session (synchronous to avoid yielding before state updates)
    if (!this.currentSessionId) {
      this.currentSessionId = `session-${Date.now()}`;
      this.messages = [];
      this.error = null;
    }

    // Add user message
    const userMessage: DisplayMessage = {
      id: generateId(),
      role: 'user',
      content: content.trim(),
      toolExecutions: [],
      timestamp: Date.now(),
    };
    this.messages = [...this.messages, userMessage];

    // Start streaming mock response
    this.isStreaming = true;
    this.streamAbortController = new AbortController();

    const assistantMessage: DisplayMessage = {
      id: generateId(),
      role: 'assistant',
      content: '',
      toolExecutions: Math.random() > 0.6 ? MOCK_TOOL_CALLS : [],
      timestamp: Date.now(),
    };
    this.messages = [...this.messages, assistantMessage];

    try {
      const responseText = MOCK_RESPONSES[Math.floor(Math.random() * MOCK_RESPONSES.length)];
      const words = responseText.split(' ');

      for (let i = 0; i < words.length; i++) {
        if (this.streamAbortController.signal.aborted) break;

        await new Promise<void>((resolve, reject) => {
          const timeout = setTimeout(resolve, 40 + Math.random() * 30);
          this.streamAbortController!.signal.addEventListener('abort', () => {
            clearTimeout(timeout);
            reject(new Error('aborted'));
          }, { once: true });
        });

        const separator = i === 0 ? '' : ' ';
        assistantMessage.content += separator + words[i];
        // Trigger reactivity by reassigning the array
        this.messages = [...this.messages.slice(0, -1), { ...assistantMessage }];
      }

      log.debug('Mock response complete', { messageId: assistantMessage.id });
    } catch (err) {
      if (err instanceof Error && err.message === 'aborted') {
        log.info('Streaming aborted by user');
      } else {
        const errorMsg = err instanceof Error ? err.message : 'Unknown streaming error';
        log.error('Streaming error', { error: errorMsg });
        this.error = errorMsg;
      }
    } finally {
      this.isStreaming = false;
      this.streamAbortController = null;
    }
  }

  /** Cancel the current streaming response. */
  cancelStreaming(): void {
    if (this.streamAbortController) {
      this.streamAbortController.abort();
    }
  }

  /** Create a new chat session. */
  async createSession(modelId?: string): Promise<string> {
    const sessionId = `session-${Date.now()}`;
    this.currentSessionId = sessionId;
    this.messages = [];
    this.error = null;
    log.info('Created new chat session', { sessionId, modelId });
    return sessionId;
  }

  /** Clear all messages in the current session. */
  clearMessages(): void {
    this.messages = [];
    this.error = null;
    log.debug('Messages cleared');
  }

  /** Reset the entire store state. */
  reset(): void {
    this.cancelStreaming();
    this.messages = [];
    this.isStreaming = false;
    this.currentSessionId = null;
    this.error = null;
  }
}

export const chatStore = new ChatStore();
