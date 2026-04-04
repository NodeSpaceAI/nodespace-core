/**
 * Chat Store - Manages chat conversation state using Svelte 5 runes.
 *
 * Wired to real Tauri invocations for local agent send/cancel.
 * Falls back to mock streaming when Tauri is not available (dev mode).
 *
 * Issue #1008: replaced mock-only implementation with real Tauri integration.
 */

import { createLogger } from '$lib/utils/logger';
import type {
  ChatMessage,
  StreamingChunk,
  LocalAgentStatus,
  ToolExecutionRecord,
  AgentTurnResult,
} from '$lib/types/agent-types';
import { AGENT_EVENTS } from '$lib/types/agent-types';
import * as tauriCommands from '$lib/services/tauri-commands';

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

/** Check if running in Tauri desktop environment. */
function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

// ---------------------------------------------------------------------------
// Mock helpers (used when Tauri is unavailable)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// ChatStore
// ---------------------------------------------------------------------------

class ChatStore {
  messages = $state<DisplayMessage[]>([]);
  isStreaming = $state(false);
  currentSessionId = $state<string | null>(null);
  error = $state<string | null>(null);

  private streamAbortController: AbortController | null = null;
  private eventUnlisteners: Array<() => void> = [];

  /** Send a user message and get a response (real or mock). */
  async sendMessage(content: string): Promise<void> {
    if (!content.trim()) return;
    if (this.isStreaming) {
      log.warn('Cannot send message while streaming');
      return;
    }

    this.error = null;

    // Ensure we have a session
    if (!this.currentSessionId) {
      await this.createSession();
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

    if (isTauri()) {
      await this.sendViaTauri(content.trim());
    } else {
      await this.sendViaMock(content.trim());
    }
  }

  /** Send via real Tauri invocation with event-based streaming. */
  private async sendViaTauri(content: string): Promise<void> {
    if (!this.currentSessionId) return;

    this.isStreaming = true;

    // Prepare assistant message placeholder
    const assistantMessage: DisplayMessage = {
      id: generateId(),
      role: 'assistant',
      content: '',
      toolExecutions: [],
      timestamp: Date.now(),
    };
    this.messages = [...this.messages, assistantMessage];

    // Set up event listeners for streaming chunks
    try {
      const { listen } = await import('@tauri-apps/api/event');

      // Listen for streaming chunks
      const unlistenChunk = await listen<StreamingChunk>(AGENT_EVENTS.LOCAL_AGENT_CHUNK, (event) => {
        const chunk = event.payload;
        if (chunk.type === 'token') {
          assistantMessage.content += chunk.text;
          this.messages = [...this.messages.slice(0, -1), { ...assistantMessage }];
        }
      });
      this.eventUnlisteners.push(unlistenChunk);

      // Listen for status updates
      const unlistenStatus = await listen<LocalAgentStatus>(AGENT_EVENTS.LOCAL_AGENT_STATUS, (event) => {
        log.debug('Agent status update', { status: event.payload });
      });
      this.eventUnlisteners.push(unlistenStatus);

      // Listen for errors
      const unlistenError = await listen<string>(AGENT_EVENTS.LOCAL_AGENT_ERROR, (event) => {
        log.error('Agent error', { error: event.payload });
        this.error = event.payload;
      });
      this.eventUnlisteners.push(unlistenError);

      // Send the message and wait for the turn to complete
      const result: AgentTurnResult = await tauriCommands.localAgentSend(
        this.currentSessionId,
        content
      );

      // Update the final message with complete content and tool executions
      const finalMessage: DisplayMessage = {
        ...assistantMessage,
        content: result.response || assistantMessage.content,
        toolExecutions: result.tool_calls_made,
      };
      this.messages = [...this.messages.slice(0, -1), finalMessage];

      log.debug('Agent turn complete', {
        messageId: finalMessage.id,
        toolCalls: result.tool_calls_made.length,
        promptTokens: result.usage.prompt_tokens,
        completionTokens: result.usage.completion_tokens,
      });
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'Unknown agent error';
      log.error('Agent send error', { error: errorMsg });
      this.error = errorMsg;
    } finally {
      this.cleanupEventListeners();
      this.isStreaming = false;
    }
  }

  /** Send via mock streaming (used in dev mode without Tauri). */
  private async sendViaMock(content: string): Promise<void> {
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
          this.streamAbortController!.signal.addEventListener(
            'abort',
            () => {
              clearTimeout(timeout);
              reject(new Error('aborted'));
            },
            { once: true }
          );
        });

        const separator = i === 0 ? '' : ' ';
        assistantMessage.content += separator + words[i];
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
    if (isTauri() && this.currentSessionId) {
      tauriCommands.localAgentCancel(this.currentSessionId).catch((err) => {
        log.error('Failed to cancel agent generation', { error: String(err) });
      });
    }
    if (this.streamAbortController) {
      this.streamAbortController.abort();
    }
    this.cleanupEventListeners();
  }

  /** Create a new chat session. */
  async createSession(modelId?: string): Promise<string> {
    if (isTauri()) {
      try {
        const sessionId = await tauriCommands.localAgentNewSession(
          modelId ?? 'ministral-3b-q4km'
        );
        this.currentSessionId = sessionId;
        this.messages = [];
        this.error = null;
        log.info('Created new Tauri chat session', { sessionId, modelId });
        return sessionId;
      } catch (err) {
        log.error('Failed to create Tauri session, falling back to mock', {
          error: String(err),
        });
      }
    }

    // Mock fallback
    const sessionId = `session-${Date.now()}`;
    this.currentSessionId = sessionId;
    this.messages = [];
    this.error = null;
    log.info('Created new mock chat session', { sessionId, modelId });
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

    if (isTauri() && this.currentSessionId) {
      tauriCommands.localAgentEndSession(this.currentSessionId).catch((err) => {
        log.error('Failed to end session during reset', { error: String(err) });
      });
    }

    this.messages = [];
    this.isStreaming = false;
    this.currentSessionId = null;
    this.error = null;
  }

  /** Clean up Tauri event listeners. */
  private cleanupEventListeners(): void {
    for (const unlisten of this.eventUnlisteners) {
      unlisten();
    }
    this.eventUnlisteners = [];
  }
}

export const chatStore = new ChatStore();
