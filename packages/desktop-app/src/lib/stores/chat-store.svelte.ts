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
  AcpMessage,
  AcpSessionState,
} from '$lib/types/agent-types';
import { AGENT_EVENTS, isAcpSessionFailed } from '$lib/types/agent-types';
import * as tauriCommands from '$lib/services/tauri-commands';
import { agentStore } from '$lib/stores/agent-store.svelte';

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
// ACP Response Extraction
// ---------------------------------------------------------------------------

/**
 * Best-effort extraction of text from an ACP result payload.
 * The exact shape from Claude Code / Gemini CLI is not yet known.
 * Logs the raw result so the shape can be observed and refined later.
 */
/**
 * Extract text from an ACP message.
 *
 * ACP agents stream `session/update` notifications:
 *   { method: "session/update", params: { update: { sessionUpdate: "agent_message_chunk",
 *     content: { type: "text", text: "..." } } } }
 *
 * The final `session/prompt` response:
 *   { result: { stopReason: "end_turn", usage: {...} } }
 */
function extractAcpText(msg: Record<string, unknown>): { text: string | null; done: boolean } {
  // Streaming session/update notification
  if (msg['method'] === 'session/update' && msg['params']) {
    const params = msg['params'] as Record<string, unknown>;
    const update = params['update'] as Record<string, unknown> | undefined;
    if (update) {
      const updateType = update['sessionUpdate'] as string;
      if (updateType === 'agent_message_chunk') {
        const content = update['content'] as Record<string, unknown> | undefined;
        if (content && typeof content['text'] === 'string') {
          return { text: content['text'], done: false };
        }
      }
    }
    return { text: null, done: false };
  }

  // Final session/prompt response (has result.stopReason)
  if (msg['result']) {
    const result = msg['result'] as Record<string, unknown>;
    if (result['stopReason']) {
      return { text: null, done: true };
    }
  }

  return { text: null, done: false };
}

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
  private acpSessionId: string | null = null;
  private acpAgentIdForSession: string | null = null;

  /** Determine if the currently selected agent is an ACP agent (not local). */
  private get isAcpAgent(): boolean {
    const id = agentStore.selectedAgentId;
    return id !== null && id !== 'local-agent';
  }

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
      if (this.isAcpAgent) {
        await this.sendViaTauriAcp(content.trim());
      } else {
        await this.sendViaTauriLocal(content.trim());
      }
    } else {
      await this.sendViaMock(content.trim());
    }
  }

  /** Send via ACP agent (external subprocess via Tauri). */
  private async sendViaTauriAcp(content: string): Promise<void> {
    const agentId = agentStore.selectedAgentId!;

    // Start ACP session if not already active for this agent
    if (!this.acpSessionId || this.acpAgentIdForSession !== agentId) {
      // Tear down any stale session for a different agent
      if (this.acpSessionId) {
        log.debug('Ending stale ACP session for agent switch', {
          old: this.acpAgentIdForSession,
          new: agentId,
        });
        tauriCommands.acpEndSession(this.acpSessionId).catch((err) => {
          log.warn('Failed to end stale ACP session', { error: String(err) });
        });
        this.acpSessionId = null;
        this.acpAgentIdForSession = null;
      }

      try {
        log.info('Starting ACP session', { agentId });
        this.acpSessionId = await tauriCommands.acpStartSession(agentId);
        this.acpAgentIdForSession = agentId;
        log.info('ACP session started', { sessionId: this.acpSessionId, agentId });
      } catch (err) {
        let errorMsg = 'Failed to start ACP session';
        if (err instanceof Error) {
          errorMsg = err.message;
        } else if (typeof err === 'object' && err !== null) {
          errorMsg = JSON.stringify(err);
        } else if (typeof err === 'string') {
          errorMsg = err;
        }
        log.error('ACP session start failed', { agentId, error: errorMsg, fullError: err });
        this.error = `Failed to start ${agentStore.selectedAgent?.name ?? agentId}: ${errorMsg}`;
        return;
      }
    }

    this.isStreaming = true;

    // Add placeholder for the assistant response
    const assistantMessage: DisplayMessage = {
      id: generateId(),
      role: 'assistant',
      content: '',
      toolExecutions: [],
      timestamp: Date.now(),
    };
    this.messages = [...this.messages, assistantMessage];

    try {
      const { listen } = await import('@tauri-apps/api/event');

      // Accumulate streamed text chunks from session/update notifications
      let accumulatedText = '';

      const unlistenMessage = await listen<AcpMessage>(AGENT_EVENTS.ACP_AGENT_MESSAGE, (event) => {
        const msg = event.payload as unknown as Record<string, unknown>;
        log.debug('ACP agent message received', { raw: msg });

        // Handle error responses
        const error = msg['error'] as Record<string, unknown> | undefined;
        if (error) {
          const errorString = JSON.stringify(error);
          let errText = 'Agent error';
          if (typeof error['message'] === 'string') {
            errText = `Agent error: ${error['message']}`;
            if (error['code']) errText += ` (code ${error['code']})`;
          } else {
            errText = `Agent error: ${errorString}`;
          }
          log.error('ACP agent returned error', { error, errorString, fullMessage: msg });
          this.error = errText;
          return;
        }

        const { text, done } = extractAcpText(msg);
        if (text !== null) {
          accumulatedText += text;
          assistantMessage.content = accumulatedText;
          this.messages = [...this.messages.slice(0, -1), { ...assistantMessage }];
        }
        if (done) {
          log.info('ACP turn completed');
        }
      });
      this.eventUnlisteners.push(unlistenMessage);

      // Listen for session state changes (for error reporting)
      const unlistenState = await listen<AcpSessionState>(AGENT_EVENTS.ACP_SESSION_STATE, (event) => {
        const state = event.payload;
        log.debug('ACP session state', { state });
        if (isAcpSessionFailed(state)) {
          log.error('ACP session failed', { reason: state.reason });
          this.error = `Agent session failed: ${state.reason}`;
        }
      });
      this.eventUnlisteners.push(unlistenState);

      // Fire and wait — response arrives via event above
      await tauriCommands.acpSendMessage(this.acpSessionId!, content);
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'ACP send failed';
      log.error('ACP send error', { error: errorMsg });
      this.error = errorMsg;
    } finally {
      this.cleanupEventListeners();
      this.isStreaming = false;
    }
  }

  /** Send via real Tauri invocation with event-based streaming (local agent). */
  private async sendViaTauriLocal(content: string): Promise<void> {
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
  private async sendViaMock(_content: string): Promise<void> {
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
    if (isTauri()) {
      if (this.isAcpAgent) {
        // ACP has no cancel mid-response — just clean up listeners
        log.debug('ACP cancel: cleaning up listeners');
      } else if (this.currentSessionId) {
        tauriCommands.localAgentCancel(this.currentSessionId).catch((err) => {
          log.error('Failed to cancel agent generation', { error: String(err) });
        });
      }
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
        if (this.isAcpAgent) {
          // ACP path: session is lazy-started on first sendMessage
          // Just reset conversation state here
          const sessionId = `acp-pending-${Date.now()}`;
          this.currentSessionId = sessionId;
          this.messages = [];
          this.error = null;
          log.info('Prepared ACP chat session (lazy start)', { agentId: agentStore.selectedAgentId });
          return sessionId;
        } else {
          // Local agent path (unchanged)
          const sessionId = await tauriCommands.localAgentNewSession(
            modelId ?? 'ministral-3b-q4km'
          );
          this.currentSessionId = sessionId;
          this.messages = [];
          this.error = null;
          log.info('Created new Tauri chat session', { sessionId, modelId });
          return sessionId;
        }
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

    if (isTauri()) {
      if (this.acpSessionId) {
        tauriCommands.acpEndSession(this.acpSessionId).catch((err) => {
          log.error('Failed to end ACP session during reset', { error: String(err) });
        });
        this.acpSessionId = null;
        this.acpAgentIdForSession = null;
      }
      if (!this.isAcpAgent && this.currentSessionId) {
        tauriCommands.localAgentEndSession(this.currentSessionId).catch((err) => {
          log.error('Failed to end local session during reset', { error: String(err) });
        });
      }
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
