<!--
  AiChatNativeNodeViewer - Page-level viewer for native AI chat conversation nodes

  `ai-chat-native` is a chat NodeSpace's own agent loop answers, with a local
  model or an OpenAI-compatible endpoint (ADR-088). Terminal chats are a
  separate subtype with their own viewer (AiChatPtyNodeViewer). This viewer
  renders the shared header (title + unified model selector) and:
    - no model chosen yet → a prompt to select one via the header selector.
    - model chosen        → message UI (chat input + the chat's message nodes).

  The header selector (AiChatModelSelector) replaces the two-step provider → model
  picker flow. It is locked (disabled) after the first user message is sent.
  Picking a terminal harness in it retypes the node to `ai-chat-pty`; the pane
  then swaps this viewer for the terminal one.

  Messages are nodes (ADR-088 §3): each message is an `ai-chat-message` child of
  the chat, in conversation order. The viewer loads the chat's children into the
  shared store and derives the conversation from the structure tree, so a reply
  the daemon creates appears through the normal node-created and relationship
  events. The chat itself is the single source of truth for the turn:
  - Frontend creates the user message child, waits for it to be stored, then
    writes `updateNode` to set `turn_status: 'processing'` — the daemon starts
    the turn when that write arrives and the chat's last message is the
    user's, so the message has to be there first. WRITES use the canonical
    snake_case schema key (`turn_status`), not
    the camelCase `turnStatus` the confirmed node reads back as: the chat family
    has no dedicated typed write command like `task` does, so whatever property
    key this component uses reaches storage verbatim, and a wrong-cased write
    key silently never reaches the daemon's inference trigger at all. The
    daemon owns every subsequent turn_status write for the turn.
  - LocalAgentService in the daemon reacts to node changes and drives inference.
  - Streaming tokens arrive via Tauri events (local-agent://chunk) and accumulate
    in a local `streamingContent` buffer. The buffer is cleared when the
    completed assistant message node arrives.
  - Typing indicator driven by the node's top-level `turnStatus === 'processing'`
    — a READ, so this uses the promoted camelCase field the daemon always
    returns, regardless of which case the write used. The backend promotes the
    node's declared fields to the top level, so this reads `node.turnStatus`,
    never `node.properties.turn_status`.
-->

<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import { appendUserMessage, discardUnsentMessage } from '$lib/services/ai-chat-messages';
  import ChatMessage from '$lib/components/chat/chat-message.svelte';
  import ChatInput from '$lib/components/chat/chat-input.svelte';
  import AiChatHeader from './ai-chat-header.svelte';
  import AiChatModelSelector from './ai-chat-model-selector.svelte';
  import type { ModelSelection } from './ai-chat-model-selector.svelte';
  import type { DisplayMessage } from '$lib/components/chat/types';
  import type { StreamingChunk } from '$lib/types/agent-types';
  import { AGENT_EVENTS } from '$lib/types/agent-types';
  import type { AiChatMessageNode, AiChatNativeNode } from '$lib/types/ai-chat-node';
  import { isAiChatMessageNode, nodeToAiChatMessageNode } from '$lib/types/ai-chat-node';
  import {
    localAgentCancelTurn,
    ensureModelReady,
  } from '$lib/services/tauri-commands';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { browserSyncService } from '$lib/services/browser-sync-service';
  import { statusBar } from '$lib/stores/status-bar.svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';

  const log = createLogger('AiChatNativeNodeViewer');
  const VIEWER_ID = 'ai-chat-viewer';

  let {
    nodeId,
  }: {
    nodeId: string;
  } = $props();

  // --- State ---
  let messagesContainer: HTMLDivElement | undefined = $state();
  /** In-flight token buffer. Cleared when WatchNodes delivers the completed message. */
  let streamingContent = $state('');
  let sendError = $state<string | null>(null);
  let nodeReady = $state(false);
  let eventUnlisteners: Array<() => void> = [];
  /** True while ensureModelReady is running (may include download time for local models). */
  let isEnsuringModel = $state(false);
  /** True from Send until the turn is requested: the message is being stored. */
  let isSending = $state(false);
  /**
   * Current phase reported by the daemon while ensureModelReady is running.
   * `null` until the first `model://status` event of a given run arrives, so
   * the overlay shows a generic label rather than falsely claiming a phase
   * before it is confirmed. Distinguishing "verifying" from "loading" matters
   * because a cache-miss integrity check can take minutes on a multi-GB
   * model — a flat "preparing model" spinner over that reads as a hang.
   */
  let ensuringModelPhase = $state<'verifying' | 'loading' | null>(null);

  // --- Reactive node lookup ---
  const node = $derived(sharedNodeStore.getNode(nodeId) as AiChatNativeNode | undefined);

  const provider = $derived(node?.provider);
  const model = $derived(node?.model ?? '');

  /** True while the daemon is processing an inference turn for this node. */
  const isProcessing = $derived(node?.turnStatus === 'processing');

  /**
   * The chat's message children in conversation order. Chats may also hold
   * non-message children; those are not part of the conversation.
   */
  const messageNodes = $derived.by<AiChatMessageNode[]>(() => {
    const messages: AiChatMessageNode[] = [];
    for (const childId of structureTree.getChildren(nodeId)) {
      const child = sharedNodeStore.getNode(childId);
      if (child && isAiChatMessageNode(child)) messages.push(nodeToAiChatMessageNode(child));
    }
    return messages;
  });

  /** True once the first user message has been sent — locks model selector. */
  const hasMessages = $derived(messageNodes.some((m) => m.role === 'user'));

  /**
   * Value string for the AiChatModelSelector <select>.
   * Mirrors the encoding used inside the component (provider:modelId).
   */
  const selectorCurrentValue = $derived(
    provider && model
      ? provider === 'openai-compat'
        ? model                      // model = full daemon ID "openai-compat:<config>[:<model>]"
        : `native:${model}`
      : ''
  );

  /** The chat's messages, mapped to DisplayMessage for rendering. */
  const persistedMessages: DisplayMessage[] = $derived(
    messageNodes
      .filter((m) => m.role === 'user' || m.role === 'assistant')
      .map((m) => ({
        id: m.id,
        role: m.role as DisplayMessage['role'],
        content: m.content,
        toolExecutions: [],
        timestamp: new Date(m.timestamp ?? m.createdAt).getTime(),
        reasoning: m.reasoning,
        options: m.options,
      }))
  );

  /** All messages to display: persisted + optional streaming overlay. */
  const displayMessages: DisplayMessage[] = $derived.by(() => {
    if (!streamingContent) return persistedMessages;
    // Append a live assistant message for the in-flight tokens.
    return [
      ...persistedMessages,
      {
        id: 'streaming',
        role: 'assistant' as const,
        content: streamingContent,
        toolExecutions: [],
        timestamp: Date.now(),
      },
    ];
  });

  /**
   * Handle a model selection from the AiChatModelSelector dropdown.
   *
   * For native models: if the model is not yet downloaded (no status in the
   * catalog list) this shows the download modal. The download modal listens for
   * MODEL_DOWNLOAD_PROGRESS events and clears itself on MODEL_DOWNLOAD_READY.
   * For openai-compat: write provider + model to the node immediately.
   * For a terminal harness: retype the node to `ai-chat-pty`.
   */
  function handleModelSelect(selection: ModelSelection): void {
    if (selection.provider === 'pty') {
      // The conversation lives in the external harness, so the chat becomes a
      // PTY chat: `agent` names the harness (AiChatPtySession pre-selects it in
      // the launch config), `model` is cleared because it only ever holds a
      // model identifier. Writes use canonical snake_case keys — the chat
      // family has no typed write command, so a key here reaches storage verbatim.
      sharedNodeStore.updateNode(
        nodeId,
        {
          nodeType: 'ai-chat-pty',
          properties: {
            agent: selection.modelId,
            model: null,
          },
        },
        { type: 'viewer', viewerId: VIEWER_ID }
      );
      return;
    }

    // Native selections persist regardless of download status so the node
    // remembers what model was chosen. The send path (handleSend) calls
    // ensureModelReady which also triggers download if needed.
    sharedNodeStore.updateNode(
      nodeId,
      {
        properties: {
          provider: selection.provider,
          model: selection.modelId,
        },
      },
      { type: 'viewer', viewerId: VIEWER_ID }
    );
  }

  function isTauri(): boolean {
    return (
      typeof window !== 'undefined' &&
      ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
    );
  }

  function cleanupListeners(): void {
    for (const unlisten of eventUnlisteners) unlisten();
    eventUnlisteners = [];
  }

  /**
   * Load the chat's children into the store (and the structure tree), so the
   * conversation derives from them. Failures are non-fatal: the events the
   * daemon broadcasts keep the conversation current once it is reachable.
   */
  async function loadMessages(): Promise<void> {
    try {
      await sharedNodeStore.loadChildrenForParent(nodeId);
    } catch (err) {
      log.warn('Failed to load chat messages', { error: toError(err).message });
    }
  }

  /**
   * Send a user message: create it as a message child of the chat, then set the
   * chat's `turn_status` to processing once the message is stored. The daemon
   * reacts to the status write and drives inference. Model must be loaded
   * first via ensureModelReady.
   */
  async function handleSend(content: string): Promise<void> {
    const trimmed = content.trim();
    if (!trimmed || isProcessing || isSending || !model) return;

    sendError = null;

    if (!sharedNodeStore.getNode(nodeId)) {
      sendError = 'Node not found';
      return;
    }

    isSending = true;
    try {
      await sendMessage(trimmed);
    } finally {
      isSending = false;
    }
  }

  async function sendMessage(trimmed: string): Promise<void> {

    // Ensure the model is loaded before writing turn_status:processing to the node.
    // For local models this may trigger a download — isEnsuringModel shows an
    // overlay so the user sees progress rather than a frozen UI.
    isEnsuringModel = true;
    ensuringModelPhase = null;
    try {
      await ensureModelReady(model);
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : ((err as Record<string, unknown>)?.message as string) ?? String(err);
      sendError = msg;
      statusBar.error(`Model error: ${msg}`);
      return;
    } finally {
      isEnsuringModel = false;
      ensuringModelPhase = null;
    }

    // The message first. It shows at once; the turn is requested only once it
    // is stored, because the daemon answers the chat's latest message when the
    // status write arrives. Asking before the message exists would have it
    // answer an older one, or find nothing to answer.
    const messageId = appendUserMessage(nodeId, trimmed, VIEWER_ID);
    await scrollToBottom();
    const failed = await sharedNodeStore.flushNodeSaves([messageId]);
    if (failed.has(messageId)) {
      discardUnsentMessage(messageId, VIEWER_ID);
      sendError = 'The message could not be saved, so it was not sent.';
      return;
    }

    // Then turn_status:'processing' (the canonical, schema-declared key — see
    // the note in the header comment) so the typing indicator appears and the
    // daemon picks up the turn. Model is guaranteed loaded above.
    sharedNodeStore.updateNode(
      nodeId,
      { properties: { turn_status: 'processing' } },
      { type: 'viewer', viewerId: VIEWER_ID }
    );
  }

  async function handleCancel(): Promise<void> {
    if (!isProcessing) return;
    try {
      await localAgentCancelTurn(nodeId);
    } catch (err) {
      log.warn('Failed to cancel turn', { error: toError(err).message });
    }
  }

  async function scrollToBottom(): Promise<void> {
    await tick();
    if (messagesContainer) {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
  }

  // --- Lifecycle ---

  let destroyed = false;

  /**
   * Phase update for the in-flight ensureModelReady call (see isEnsuringModel).
   * Filtered by model id so a stale or cross-talk event for a different model
   * (e.g. another view's ensureModelReady call) can't flip this viewer's label.
   */
  function applyModelPhase(eventModelId: string, status: string): void {
    if (destroyed || !isEnsuringModel) return;
    if (eventModelId !== model) return;
    if (status === 'verifying' || status === 'loading') {
      ensuringModelPhase = status;
    }
  }

  onMount(async () => {
    log.debug('AiChatNativeNodeViewer mounted', { nodeId });

    const messagesLoaded = loadMessages();

    try {
      if (isTauri()) {
        if (destroyed) return;

        // Subscribe to streaming token events for this node.
        const unlistenChunk = await listen<StreamingChunk & { node_id?: string }>(
          AGENT_EVENTS.LOCAL_AGENT_CHUNK,
          (event) => {
            if (destroyed) return;
            const chunk = event.payload;
            // Filter to only this node's events.
            if (chunk.node_id && chunk.node_id !== nodeId) return;

            if (chunk.type === 'token') {
              streamingContent += chunk.text ?? '';
              scrollToBottom();
            } else if (chunk.type === 'done') {
              // Streaming complete. Clear the buffer — the persisted assistant
              // message node arrives reactively via the broadcast events.
              streamingContent = '';
            } else if (chunk.type === 'cancelled') {
              streamingContent = '';
            } else if (chunk.type === 'error') {
              sendError = (chunk as unknown as { error_message?: string }).error_message ?? 'Inference error';
              streamingContent = '';
            }
          }
        );
        eventUnlisteners.push(unlistenChunk);

        const unlistenError = await listen<string>(AGENT_EVENTS.LOCAL_AGENT_ERROR, (event) => {
          if (destroyed) return;
          log.error('Agent error', { error: event.payload });
          sendError = event.payload;
          streamingContent = '';
        });
        eventUnlisteners.push(unlistenError);

        const unlistenModelStatus = await listen<{
          model_id: string;
          status: string;
          message?: string;
        }>(AGENT_EVENTS.MODEL_STATUS, (event) => {
          applyModelPhase(event.payload.model_id, event.payload.status);
        });
        eventUnlisteners.push(unlistenModelStatus);
      } else {
        // Browser mode: the dev-proxy relays the same daemon progress stream
        // over SSE while its /ensure-model-ready request is in flight.
        eventUnlisteners.push(
          browserSyncService.onModelLoadProgress((event) => {
            applyModelPhase(event.modelId, event.status);
          })
        );
      }
    } finally {
      await messagesLoaded;
      nodeReady = true;
    }
  });

  onDestroy(() => {
    destroyed = true;
    cleanupListeners();
  });

  // Auto-scroll as messages stream/append.
  $effect(() => {
    void displayMessages.length;
    scrollToBottom();
  });

  // In browser mode (no Tauri streaming events), poll the backend while processing
  // so the UI updates even if the SSE connection is temporarily unavailable.
  // Capped at 15 attempts (30 s) to avoid flooding the proxy when the daemon
  // is stuck or SSE never recovers.
  $effect(() => {
    if (isTauri() || !isProcessing) return;

    const MAX_ATTEMPTS = 15;
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout>;
    let cancelled = false;

    async function poll(): Promise<void> {
      if (cancelled || attempts >= MAX_ATTEMPTS) return;
      attempts++;
      try {
        // ADR-053: drop this poll's write if the active database switches while
        // the fetch is in flight, so the previous database's node isn't written
        // into the now-active store.
        const epoch = sharedNodeStore.currentEpoch();
        const fetched = await backendAdapter.getNode(nodeId);
        if (fetched && !cancelled && sharedNodeStore.currentEpoch() === epoch) {
          sharedNodeStore.setNode(fetched, { type: 'database', reason: 'poll' }, true);
          await loadMessages();
          // If SSE is down, nudge it to reconnect.
          if (!browserSyncService.isConnected()) {
            browserSyncService.initialize().catch(() => {/* ignore */});
          }
        }
      } catch {
        // Polling failures are non-fatal — SSE will deliver when reconnected.
      }
      if (!cancelled && attempts < MAX_ATTEMPTS) {
        timer = setTimeout(poll, 2000);
      }
    }

    // Start first poll after 2 s to give SSE a chance to deliver first.
    timer = setTimeout(poll, 2000);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  });
</script>

<div class="ai-chat-viewer">
  <AiChatHeader {nodeId}>
    {#snippet actions()}
      <AiChatModelSelector
        {nodeId}
        disabled={hasMessages}
        currentValue={selectorCurrentValue}
        onSelect={handleModelSelect}
      />
    {/snippet}
  </AiChatHeader>

  {#if !nodeReady}
    <div class="provider-prompt">
      <p class="provider-prompt-text">Loading…</p>
    </div>
  {:else if !model}
    <div class="provider-prompt">
      <p class="provider-prompt-text">Choose a model to get started</p>
      <p class="provider-prompt-hint">
        Select a model from the dropdown above to begin the conversation.
      </p>
    </div>
  {:else}
    <div
      class="chat-viewer-messages"
      bind:this={messagesContainer}
      role="list"
      aria-label="Chat conversation"
    >
      {#if displayMessages.length === 0}
        <div class="empty-conversation">
          <div class="empty-conversation-icon">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              width="48"
              height="48"
            >
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
            </svg>
          </div>
          <p class="empty-conversation-text">Start a conversation</p>
          <p class="empty-conversation-hint">Type a message below to begin</p>
        </div>
      {:else}
        {#each displayMessages as message, index (message.id)}
          <ChatMessage
            {message}
            isLatest={index === displayMessages.length - 1}
            onSelectOption={isProcessing || isSending ? undefined : handleSend}
          />
        {/each}
      {/if}

      {#if isProcessing}
        <div class="typing-indicator" aria-label="AI is thinking">
          <span class="typing-dot"></span>
          <span class="typing-dot"></span>
          <span class="typing-dot"></span>
          <button class="cancel-turn-btn" onclick={handleCancel} aria-label="Cancel response">
            Stop
          </button>
        </div>
      {/if}

    </div>

    {#if sendError}
      <div class="send-error" role="alert">{sendError}</div>
    {/if}

    <ChatInput
      onSend={handleSend}
      disabled={isProcessing || isSending}
      placeholder={isProcessing ? 'AI is responding...' : 'Type a message...'}
    />
  {/if}

  <!-- Model-load overlay: shown while ensureModelReady is running (covers downloads too). -->
  {#if isEnsuringModel}
    {@const label =
      ensuringModelPhase === 'verifying'
        ? 'Verifying model integrity…'
        : ensuringModelPhase === 'loading'
          ? 'Loading model…'
          : 'Preparing model…'}
    <div class="ensure-model-overlay" role="status" aria-label={label}>
      <div class="ensure-model-box">
        <span class="ensure-model-spinner" aria-hidden="true"></span>
        <div class="ensure-model-text">
          <span class="ensure-model-label">{label}</span>
          {#if ensuringModelPhase === 'verifying'}
            <span class="ensure-model-sublabel">This can take a few minutes on a large model's first check.</span>
          {/if}
        </div>
      </div>
    </div>
  {/if}

</div>

<style>
  .ai-chat-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
    position: relative;
  }

  .provider-prompt {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 2rem 1rem;
    text-align: center;
  }

  .provider-prompt-text {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .provider-prompt-hint {
    margin: 0;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
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
  }

  .cancel-turn-btn {
    margin-left: 0.5rem;
    padding: 0.125rem 0.5rem;
    font-size: 0.75rem;
    background: transparent;
    border: 1px solid hsl(var(--muted-foreground) / 0.4);
    border-radius: 0.25rem;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
  }

  .cancel-turn-btn:hover {
    border-color: hsl(var(--destructive));
    color: hsl(var(--destructive));
  }

  .send-error {
    margin: 0.5rem 1rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    background: hsl(var(--destructive) / 0.1);
    border: 1px solid hsl(var(--destructive) / 0.3);
    color: hsl(var(--destructive));
    font-size: 0.8125rem;
  }

  /* Model-load overlay */
  .ensure-model-overlay {
    position: absolute;
    inset: 0;
    background: hsl(var(--background) / 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 20;
  }

  .ensure-model-box {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    background: hsl(var(--card));
    border: 1px solid hsl(var(--border));
    border-radius: 0.75rem;
    padding: 1.25rem 1.75rem;
  }

  .ensure-model-spinner {
    display: inline-block;
    width: 18px;
    height: 18px;
    border: 2.5px solid hsl(var(--muted-foreground) / 0.3);
    border-top-color: hsl(var(--primary));
    border-radius: 50%;
  }

  .ensure-model-text {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .ensure-model-label {
    font-size: 0.9375rem;
    font-weight: 500;
    color: hsl(var(--foreground));
  }

  .ensure-model-sublabel {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }
</style>
