<!--
  AiChatNodeViewer - Page-level viewer for AI chat conversation nodes

  Renders a conversation from node properties (messages[]) using the existing
  ChatMessage and ToolCallDisplay components from #1003. Implements write
  buffering with debounced flush and tool result archival per ADR-028.

  Follows the *NodeViewer pattern but does NOT wrap BaseNodeViewer because
  it renders a chat conversation rather than a hierarchical node collection.
-->

<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import ChatMessage from '$lib/components/chat/chat-message.svelte';
  import ChatInput from '$lib/components/chat/chat-input.svelte';
  import { chatStore } from '$lib/stores/chat-store.svelte';
  import type { DisplayMessage } from '$lib/stores/chat-store.svelte';
  import type { ToolExecutionRecord } from '$lib/types/agent-types';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('AiChatNodeViewer');

  let {
    nodeId,
    onTitleChange,
  }: {
    nodeId: string;
    onTitleChange?: (_title: string) => void;
  } = $props();

  // --- State ---
  let messagesContainer: HTMLDivElement | undefined = $state();
  let inMemoryMessages = $state<DisplayMessage[]>([]);
  let isStreaming = $state(false);
  let flushTimer: ReturnType<typeof setTimeout> | null = null;
  let hasUnsavedChanges = $state(false);

  /** Soft message cap -- show a nudge when conversation gets long */
  const SOFT_MESSAGE_CAP = 500;
  const FLUSH_DEBOUNCE_MS = 7_000; // 7 seconds (within 5-10s range per spec)

  // --- Reactive node lookup ---
  const node = $derived(sharedNodeStore.getNode(nodeId));
  const provider = $derived(
    (node?.properties?.provider as string) ?? 'native'
  );
  const model = $derived((node?.properties?.model as string) ?? '');
  const status = $derived(
    (node?.properties?.status as string) ?? 'active'
  );
  const showMessageCap = $derived(inMemoryMessages.length >= SOFT_MESSAGE_CAP);

  // --- Load messages from persisted node properties ---
  function loadMessagesFromNode(): void {
    if (!node) return;
    const persisted = node.properties?.messages;
    if (!Array.isArray(persisted)) {
      inMemoryMessages = [];
      return;
    }
    inMemoryMessages = persisted.map((m: Record<string, unknown>, idx: number) => {
      const role = (m.role as string) ?? 'user';
      // For tool_call messages, map to assistant role with tool executions
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
          timestamp: m.timestamp ? new Date(m.timestamp as string).getTime() : Date.now(),
        };
      }
      return {
        id: `persisted-${idx}`,
        role: role as DisplayMessage['role'],
        content: (m.content as string) ?? '',
        toolExecutions: [],
        timestamp: m.timestamp ? new Date(m.timestamp as string).getTime() : Date.now(),
      };
    });
  }

  // --- Write buffering: archive tool results and debounced flush ---

  /** Convert DisplayMessage[] back to the persisted messages format,
   *  nulling full tool results (only result_summary kept). */
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

  function scheduleFlush(): void {
    if (flushTimer) clearTimeout(flushTimer);
    hasUnsavedChanges = true;
    flushTimer = setTimeout(() => flushToStore(), FLUSH_DEBOUNCE_MS);
  }

  function flushToStore(): void {
    if (!node || !hasUnsavedChanges) return;
    if (flushTimer) {
      clearTimeout(flushTimer);
      flushTimer = null;
    }
    try {
      const archivedMessages = archiveMessages(inMemoryMessages);
      sharedNodeStore.updateNode(nodeId, {
        properties: {
          ...node.properties,
          messages: archivedMessages,
          last_active: new Date().toISOString(),
          context_tokens: estimateTokens(inMemoryMessages),
        },
      }, { type: 'viewer', viewerId: 'ai-chat-viewer' });
      hasUnsavedChanges = false;
      log.debug('Flushed messages to store', { messageCount: archivedMessages.length });
    } catch (err) {
      log.error('Failed to flush messages', err);
    }
  }

  /** Rough token estimate: ~4 chars per token */
  function estimateTokens(msgs: DisplayMessage[]): number {
    let chars = 0;
    for (const m of msgs) {
      chars += m.content.length;
    }
    return Math.ceil(chars / 4);
  }

  // --- Sending messages (delegates to chatStore for streaming) ---

  async function handleSend(content: string): Promise<void> {
    if (!content.trim() || isStreaming) return;

    // Add user message to in-memory state
    const userMsg: DisplayMessage = {
      id: `msg-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`,
      role: 'user',
      content: content.trim(),
      toolExecutions: [],
      timestamp: Date.now(),
    };
    inMemoryMessages = [...inMemoryMessages, userMsg];
    scheduleFlush();

    // Delegate to chatStore for streaming response
    isStreaming = true;
    try {
      await chatStore.sendMessage(content);

      // Capture the assistant response from chatStore
      const storeMessages = chatStore.messages;
      if (storeMessages.length > 0) {
        const lastMsg = storeMessages[storeMessages.length - 1];
        if (lastMsg.role === 'assistant') {
          inMemoryMessages = [...inMemoryMessages, { ...lastMsg }];
          scheduleFlush();
        }
      }
    } finally {
      isStreaming = false;
    }

    await scrollToBottom();
  }

  async function scrollToBottom(): Promise<void> {
    await tick();
    if (messagesContainer) {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
  }

  // --- Lifecycle ---

  onMount(() => {
    log.debug('AiChatNodeViewer mounted', { nodeId });
    loadMessagesFromNode();
    if (node?.content) {
      onTitleChange?.(node.content);
    }
  });

  onDestroy(() => {
    // Immediate flush on close/navigate-away
    if (hasUnsavedChanges) {
      flushToStore();
    }
    if (flushTimer) clearTimeout(flushTimer);
  });

  // Auto-scroll when messages change
  $effect(() => {
    // Access length to create dependency tracking
    if (inMemoryMessages.length >= 0) {
      scrollToBottom();
    }
  });

  // Update title when node content changes
  $effect(() => {
    if (node?.content) {
      onTitleChange?.(node.content);
    }
  });
</script>

<div class="ai-chat-viewer">
  <!-- Header -->
  <div class="chat-viewer-header">
    <div class="chat-viewer-header-left">
      <h2 class="chat-viewer-title">{node?.content ?? 'AI Chat'}</h2>
      <div class="chat-viewer-meta">
        <span class="meta-badge">{provider}</span>
        {#if model}
          <span class="meta-badge meta-model">{model}</span>
        {/if}
        <span class="meta-badge" class:meta-archived={status === 'archived'}>
          {status}
        </span>
      </div>
    </div>
    {#if hasUnsavedChanges}
      <span class="save-indicator" title="Unsaved changes (auto-saving...)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
          <circle cx="12" cy="12" r="3" />
        </svg>
      </span>
    {/if}
  </div>

  <!-- Message list -->
  <div
    class="chat-viewer-messages"
    bind:this={messagesContainer}
    role="list"
    aria-label="Chat conversation"
  >
    {#if inMemoryMessages.length === 0}
      <div class="empty-conversation">
        <div class="empty-conversation-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="48" height="48">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
          </svg>
        </div>
        <p class="empty-conversation-text">Start a conversation</p>
        <p class="empty-conversation-hint">Type a message below to begin</p>
      </div>
    {:else}
      {#each inMemoryMessages as message (message.id)}
        <ChatMessage {message} />
      {/each}
    {/if}

    {#if isStreaming}
      <div class="typing-indicator" aria-label="AI is thinking">
        <span class="typing-dot"></span>
        <span class="typing-dot"></span>
        <span class="typing-dot"></span>
      </div>
    {/if}

    {#if showMessageCap}
      <div class="message-cap-nudge" role="alert">
        <p>This conversation has {inMemoryMessages.length} messages. Consider starting a new chat for better performance.</p>
      </div>
    {/if}
  </div>

  <!-- Input -->
  {#if status !== 'archived'}
    <ChatInput
      onSend={handleSend}
      disabled={isStreaming}
      placeholder={isStreaming ? 'AI is responding...' : 'Type a message...'}
    />
  {:else}
    <div class="archived-notice">
      This conversation is archived and read-only.
    </div>
  {/if}
</div>

<style>
  .ai-chat-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .chat-viewer-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
  }

  .chat-viewer-header-left {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .chat-viewer-title {
    font-size: 1rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
  }

  .chat-viewer-meta {
    display: flex;
    gap: 0.375rem;
    flex-wrap: wrap;
  }

  .meta-badge {
    font-size: 0.6875rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    padding: 0.0625rem 0.375rem;
    border-radius: 9999px;
    text-transform: lowercase;
  }

  .meta-model {
    font-family: monospace;
    font-size: 0.625rem;
  }

  .meta-archived {
    color: hsl(var(--destructive));
    background: hsl(var(--destructive) / 0.1);
  }

  .save-indicator {
    display: flex;
    align-items: center;
    color: hsl(var(--muted-foreground));
    animation: pulse-save 1.5s ease-in-out infinite;
  }

  @keyframes pulse-save {
    0%, 100% { opacity: 0.4; }
    50% { opacity: 1; }
  }

  .chat-viewer-messages {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0;
  }

  .empty-conversation {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: 2rem;
    text-align: center;
  }

  .empty-conversation-icon {
    color: hsl(var(--muted-foreground) / 0.5);
    margin-bottom: 1rem;
  }

  .empty-conversation-text {
    font-size: 1rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    margin: 0 0 0.5rem;
  }

  .empty-conversation-hint {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  .typing-indicator {
    display: flex;
    gap: 0.25rem;
    padding: 0.75rem 1.5rem;
    align-items: center;
  }

  .typing-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: hsl(var(--muted-foreground));
    animation: typing-bounce 1.2s ease-in-out infinite;
  }

  .typing-dot:nth-child(2) {
    animation-delay: 0.15s;
  }

  .typing-dot:nth-child(3) {
    animation-delay: 0.3s;
  }

  @keyframes typing-bounce {
    0%, 60%, 100% {
      transform: translateY(0);
      opacity: 0.4;
    }
    30% {
      transform: translateY(-4px);
      opacity: 1;
    }
  }

  .message-cap-nudge {
    margin: 0.5rem 1rem;
    padding: 0.75rem 1rem;
    background: hsl(var(--accent) / 0.1);
    border: 1px solid hsl(var(--accent) / 0.3);
    border-radius: 0.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .message-cap-nudge p {
    margin: 0;
  }

  .archived-notice {
    padding: 0.75rem 1rem;
    text-align: center;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted) / 0.5);
    border-top: 1px solid hsl(var(--border));
  }
</style>
