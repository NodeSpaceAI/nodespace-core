<!--
  AiChatNodeViewer - Page-level viewer for AI chat conversation nodes

  Per ADR-034, `ai-chat` is one node type with multiple provider modes. This is THE
  single dispatcher for the type. It renders a header (title + unified model selector)
  and routes on `properties.provider` (+ `properties.model`):
    - pty                           → embedded terminal session (AiChatPtySession).
    - native | openai-compat → message UI (chat input + streamed messages[]).
    - model not yet set             → prompt to select a model via the header selector.

  The header selector (AiChatModelSelector) replaces the two-step provider → model
  picker flow. It is locked (disabled) after the first user message is sent.

  Node-as-message-queue architecture: the node is the single source of truth.
  - Frontend writes `updateNode` to append the user message and set
    `turn_status: 'processing'` — that write is the trigger the daemon watches
    for. WRITES use the canonical snake_case schema key (`turn_status`), not
    the camelCase `turnStatus` the confirmed node reads back as: ai-chat has
    no dedicated typed write command like `task` does, so whatever property
    key this component uses reaches storage verbatim, and a wrong-cased write
    key silently never reaches the daemon's inference trigger at all (see
    `AiChatNode::from_node`'s doc comment on the Rust side for the full
    story). The daemon owns every subsequent turn_status write for the turn.
  - LocalAgentService in the daemon reacts to node changes and drives inference.
  - Streaming tokens arrive via Tauri events (local-agent://chunk) and accumulate
    in a local `streamingContent` buffer. The buffer is cleared when WatchNodes
    delivers the completed assistant message.
  - Typing indicator driven by the node's top-level `turnStatus === 'processing'`
    — a READ, so this uses the promoted camelCase field the daemon always
    returns, regardless of which case the write used. The daemon flattens the
    `ai-chat` namespace before it reaches the frontend, so this reads
    `node.turnStatus`, never `node.properties['ai-chat'].turn_status`.
  - `turnStatus`/`turn_status` and `sessionStatus`/`session_status` are
    independent axes (turn/inference state vs. PTY session lifecycle) — see
    `ai-chat-node.ts`'s doc comments. This viewer only reads/writes the turn
    axis; the session axis belongs to `AiChatPtySession`.
-->

<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import ChatMessage from '$lib/components/chat/chat-message.svelte';
  import ChatInput from '$lib/components/chat/chat-input.svelte';
  import AiChatPtySession from './ai-chat-pty-session.svelte';
  import AiChatModelSelector from './ai-chat-model-selector.svelte';
  import type { ModelSelection } from './ai-chat-model-selector.svelte';
  import type { DisplayMessage } from '$lib/components/chat/types';
  import type { StreamingChunk } from '$lib/types/agent-types';
  import { AGENT_EVENTS } from '$lib/types/agent-types';
  import type { AiChatNode } from '$lib/types/ai-chat-node';
  import {
    localAgentCancelTurn,
    ensureModelReady,
  } from '$lib/services/tauri-commands';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { browserSyncService } from '$lib/services/browser-sync-service';
  import { statusBar } from '$lib/stores/status-bar.svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { aiChatsData } from '$lib/stores/ai-chats.svelte';
  import {
    aiChatDisplayTitle,
    resolveChatTitleCommit,
    UNTITLED_CHAT_LABEL
  } from '$lib/utils/ai-chat-title';

  const log = createLogger('AiChatNodeViewer');

  /** Provider modes that render the message UI. */
  const MESSAGE_PROVIDERS = ['native', 'openai', 'openai-compat'] as const;

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
  /**
   * Current phase reported by the daemon while ensureModelReady is running.
   * `null` until the first `model://status` event of a given run arrives, so
   * the overlay shows a generic label rather than falsely claiming a phase
   * before it is confirmed. Distinguishing "verifying" from "loading" matters
   * because a cache-miss integrity check can take minutes on a multi-GB
   * model — a flat "preparing model" spinner over that reads as a hang.
   */
  let ensuringModelPhase = $state<'verifying' | 'loading' | null>(null);

  // --- Title editing ---------------------------------------------------
  /** True while the header title is an editable input rather than static text. */
  let editingTitle = $state(false);
  /** The in-progress edit. Only meaningful while editingTitle is true. */
  let titleDraft = $state('');
  let titleInputEl: HTMLInputElement | undefined = $state();

  const SOFT_MESSAGE_CAP = 500;

  // --- Reactive node lookup ---
  const node = $derived(sharedNodeStore.getNode(nodeId) as AiChatNode | undefined);

  const provider = $derived(node?.provider);
  const model = $derived(node?.model ?? '');
  const isMessageProvider = $derived(
    provider !== undefined && (MESSAGE_PROVIDERS as readonly string[]).includes(provider)
  );
  const lifecycleStatus = $derived(node?.lifecycleStatus ?? 'active');

  /** True while the daemon is processing an inference turn for this node. */
  const isProcessing = $derived(node?.turnStatus === 'processing');

  /** True once the first user message has been sent — locks model selector. */
  const hasMessages = $derived(
    (node?.messages ?? []).filter((m) => m.role === 'user').length > 0
  );

  /**
   * Value string for the AiChatModelSelector <select>.
   * Mirrors the encoding used inside the component (provider:modelId).
   */
  const selectorCurrentValue = $derived(
    provider && model
      ? provider === 'openai-compat'
        ? model                      // model = full daemon ID "openai-compat:<config>[:<model>]"
        : provider === 'pty'
          ? `pty:${model}`           // model = agent id, e.g. "claude-code"
          : `native:${model}`
      : ''
  );

  /** Messages from the persisted node, mapped to DisplayMessage for rendering. */
  const persistedMessages: DisplayMessage[] = $derived.by(() => {
    const msgs = node?.messages;
    if (!Array.isArray(msgs)) return [];
    return msgs
      .filter((m) => m.role === 'user' || m.role === 'assistant')
      .map((m, idx) => ({
        id: `persisted-${idx}-${m.timestamp ?? idx}`,
        role: m.role as DisplayMessage['role'],
        content: m.content ?? '',
        toolExecutions: [],
        timestamp: m.timestamp ? new Date(m.timestamp).getTime() : Date.now(),
        reasoning: m.reasoning,
        question: m.question,
        options: m.options,
      }));
  });

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

  const showMessageCap = $derived(persistedMessages.length >= SOFT_MESSAGE_CAP);

  /**
   * Handle a model selection from the AiChatModelSelector dropdown.
   *
   * For native models: if the model is not yet downloaded (no status in the
   * catalog list) this shows the download modal. The download modal listens for
   * MODEL_DOWNLOAD_PROGRESS events and clears itself on MODEL_DOWNLOAD_READY.
   * For openai-compat: write provider + model to the node immediately.
   */
  function handleModelSelect(selection: ModelSelection): void {
    if (selection.provider === 'native') {
      // Persist the selection regardless of download status so the node
      // remembers what model was chosen. The send path (handleSend) calls
      // ensureModelReady which also triggers download if needed.
      const current = sharedNodeStore.getNode(nodeId) as unknown as AiChatNode | undefined;
      sharedNodeStore.updateNode(
        nodeId,
        {
          properties: {
            messages: current?.messages ?? [],
            // Canonical snake_case keys, matching the schema's declared field
            // names — ai-chat has no dedicated typed write command like task
            // does, so whatever key this object uses reaches storage
            // verbatim. See AiChatNode::from_node's doc comment for why a
            // camelCase key here would silently break inference triggering.
            turn_status: current?.turnStatus ?? 'idle',
            session_status: current?.sessionStatus ?? 'active',
            provider: 'native',
            model: selection.modelId,
          },
        },
        { type: 'viewer', viewerId: 'ai-chat-viewer' }
      );
      return;
    }

    if (selection.provider === 'pty') {
      // PTY sessions store no messages — the conversation lives in the
      // external harness. `model` holds the chosen agent id (e.g.
      // "claude-code") so AiChatPtySession can pre-select it in the launch
      // config; capture:* properties are filled in separately once launched.
      const current = sharedNodeStore.getNode(nodeId) as unknown as AiChatNode | undefined;
      sharedNodeStore.updateNode(
        nodeId,
        {
          properties: {
            messages: current?.messages ?? [],
            turn_status: current?.turnStatus ?? 'idle',
            session_status: current?.sessionStatus ?? 'active',
            provider: 'pty',
            model: selection.modelId || null,
          },
        },
        { type: 'viewer', viewerId: 'ai-chat-viewer' }
      );
      return;
    }

    // openai-compat: write directly.
    const current = sharedNodeStore.getNode(nodeId) as unknown as AiChatNode | undefined;
    sharedNodeStore.updateNode(
      nodeId,
      {
        properties: {
          messages: current?.messages ?? [],
          turn_status: current?.turnStatus ?? 'idle',
          session_status: current?.sessionStatus ?? 'active',
          provider: selection.provider,
          model: selection.modelId,
        },
      },
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
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
   * Send a user message: append it to the node's ai-chat messages via updateNode.
   * The daemon reacts to the NodeUpdated event and drives inference.
   * Model must be loaded first via ensureModelReady.
   */
  async function handleSend(content: string): Promise<void> {
    const trimmed = content.trim();
    if (!trimmed || isProcessing || !model) return;

    sendError = null;

    // Append the user message immediately (synchronous, before any await) so the
    // optimistic store update fires while still in a Svelte reactive context.
    // The daemon handles model loading internally before starting inference.
    const current = sharedNodeStore.getNode(nodeId);
    if (!current) {
      sendError = 'Node not found';
      return;
    }

    const existingMessages = Array.isArray((current as unknown as AiChatNode).messages) ? (current as unknown as AiChatNode).messages : [];
    const newMessage = {
      role: 'user' as const,
      content: trimmed,
      timestamp: new Date().toISOString(),
    };

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

    // Set turn_status:'processing' (the canonical, schema-declared key — see
    // the note in handleModelSelect) so the typing indicator appears and the
    // daemon picks up the turn via NodeUpdated. Model is guaranteed loaded
    // above. session_status is untouched — this write only owns the turn axis.
    sharedNodeStore.updateNode(
      nodeId,
      {
        properties: {
          messages: [...existingMessages, newMessage],
          turn_status: 'processing',
        },
      },
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
    );

    await scrollToBottom();
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

  // --- Title editing --------------------------------------------------------

  /** Enter edit mode, seeded with the node's raw (untrimmed, possibly empty)
   *  content — never the "Untitled chat" placeholder, which is a display
   *  fallback, not a value to edit. */
  function startEditingTitle(): void {
    if (editingTitle) return;
    titleDraft = node?.content ?? '';
    editingTitle = true;
  }

  /** Persist the edit if it actually changed anything, then leave edit mode. */
  function commitTitle(): void {
    if (!editingTitle) return;
    editingTitle = false;
    const toPersist = resolveChatTitleCommit(node?.content ?? '', titleDraft);
    if (toPersist === null) return;
    sharedNodeStore.updateNode(
      nodeId,
      { content: toPersist },
      { type: 'viewer', viewerId: 'ai-chat-viewer' }
    );
    // Keep the sidebar's chat list in sync without a full reload — mirrors
    // how `aiChatsData.createChat` already prepends optimistically.
    aiChatsData.updateChatContent(nodeId, toPersist);
  }

  /** Leave edit mode without persisting — the draft is simply discarded. */
  function cancelEditingTitle(): void {
    editingTitle = false;
  }

  function onTitleKeydown(event: KeyboardEvent): void {
    if (event.key === 'Enter') {
      event.preventDefault();
      commitTitle();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      cancelEditingTitle();
    }
  }

  // Focus (and select) the input the moment it mounts, so entering edit mode
  // drops the user straight into typing rather than requiring a second click.
  $effect(() => {
    if (editingTitle && titleInputEl) {
      titleInputEl.focus();
      titleInputEl.select();
    }
  });

  // --- Lifecycle ---

  let destroyed = false;

  onMount(async () => {
    log.debug('AiChatNodeViewer mounted', { nodeId });

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
              // Streaming complete. Clear the buffer — WatchNodes will deliver
              // the persisted assistant message reactively via the broadcast event.
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

        // Phase updates for the in-flight ensureModelReady call (see
        // isEnsuringModel below). Filtered by model_id so a stale or
        // cross-talk event for a different model (e.g. another view's
        // ensureModelReady call) can't flip this viewer's phase label.
        const unlistenModelStatus = await listen<{
          model_id: string;
          status: string;
          message?: string;
        }>(AGENT_EVENTS.MODEL_STATUS, (event) => {
          if (destroyed || !isEnsuringModel) return;
          if (event.payload.model_id !== model) return;
          const { status } = event.payload;
          if (status === 'verifying' || status === 'loading') {
            ensuringModelPhase = status;
          }
        });
        eventUnlisteners.push(unlistenModelStatus);
      }
    } finally {
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
  <!-- Header (shown in every mode): title + unified model selector. -->
  <div class="chat-viewer-header">
    <div class="chat-viewer-header-left">
      {#if editingTitle}
        <input
          bind:this={titleInputEl}
          class="chat-viewer-title-input"
          type="text"
          value={titleDraft}
          oninput={(e) => (titleDraft = e.currentTarget.value)}
          onblur={commitTitle}
          onkeydown={onTitleKeydown}
          aria-label="Chat title"
          placeholder={UNTITLED_CHAT_LABEL}
        />
      {:else}
        <h2 class="chat-viewer-title">
          <button
            type="button"
            class="chat-viewer-title-button"
            onclick={startEditingTitle}
            aria-label="Rename chat"
          >
            {aiChatDisplayTitle(node?.content)}
          </button>
        </h2>
      {/if}
    </div>
    <div class="chat-viewer-header-right">
      {#if provider !== 'pty'}
        <AiChatModelSelector
          {nodeId}
          disabled={hasMessages}
          currentValue={selectorCurrentValue}
          onSelect={handleModelSelect}
        />
      {/if}
    </div>
  </div>

  {#if !nodeReady}
    <div class="provider-prompt">
      <p class="provider-prompt-text">Loading…</p>
    </div>
  {:else if provider === undefined}
    <div class="provider-prompt">
      <p class="provider-prompt-text">Choose a model to get started</p>
      <p class="provider-prompt-hint">
        Select a model from the dropdown above to begin the conversation.
      </p>
    </div>
  {:else if provider === 'pty'}
    <AiChatPtySession {nodeId} />
  {:else if isMessageProvider && !model}
    <div class="provider-prompt">
      <p class="provider-prompt-text">Choose a model to get started</p>
      <p class="provider-prompt-hint">
        Select a model from the dropdown above to begin the conversation.
      </p>
    </div>
  {:else if isMessageProvider}
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
            onSelectOption={isProcessing ? undefined : handleSend}
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

      {#if showMessageCap}
        <div class="message-cap-nudge" role="alert">
          <p>
            This conversation has {persistedMessages.length} messages. Consider starting a new chat for
            better performance.
          </p>
        </div>
      {/if}
    </div>

    {#if sendError}
      <div class="send-error" role="alert">{sendError}</div>
    {/if}

    {#if lifecycleStatus !== 'archived'}
      <ChatInput
        onSend={handleSend}
        disabled={isProcessing}
        placeholder={isProcessing ? 'AI is responding...' : 'Type a message...'}
      />
    {:else}
      <div class="archived-notice">This conversation is archived and read-only.</div>
    {/if}
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

  .chat-viewer-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
    gap: 0.75rem;
  }

  .chat-viewer-header-left {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }

  .chat-viewer-header-right {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }

  .chat-viewer-title {
    font-size: 1rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .chat-viewer-title-button {
    display: block;
    width: 100%;
    max-width: 100%;
    padding: 0.125rem 0.25rem;
    margin: -0.125rem -0.25rem;
    border: none;
    border-radius: 0.25rem;
    background: transparent;
    font: inherit;
    color: inherit;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: text;
  }

  .chat-viewer-title-button:hover {
    background: hsl(var(--muted) / 0.6);
  }

  .chat-viewer-title-input {
    width: 100%;
    max-width: 100%;
    padding: 0.125rem 0.25rem;
    margin: -0.125rem -0.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    background: hsl(var(--background));
    font-size: 1rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .chat-viewer-title-input:focus {
    outline: none;
    border-color: hsl(var(--primary));
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
    animation: typing-bounce 1.2s ease-in-out infinite;
  }

  .typing-dot:nth-child(2) {
    animation-delay: 0.15s;
  }

  .typing-dot:nth-child(3) {
    animation-delay: 0.3s;
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

  @keyframes typing-bounce {
    0%,
    60%,
    100% {
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

  .send-error {
    margin: 0.5rem 1rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    background: hsl(0 72% 51% / 0.1);
    border: 1px solid hsl(0 72% 51% / 0.3);
    color: hsl(0 72% 51%);
    font-size: 0.8125rem;
  }

  .archived-notice {
    padding: 0.75rem 1rem;
    text-align: center;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted) / 0.5);
    border-top: 1px solid hsl(var(--border));
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
    box-shadow: 0 8px 32px hsl(0 0% 0% / 0.12);
  }

  .ensure-model-spinner {
    display: inline-block;
    width: 18px;
    height: 18px;
    border: 2.5px solid hsl(var(--muted-foreground) / 0.3);
    border-top-color: hsl(var(--primary));
    border-radius: 50%;
    animation: spin 0.75s linear infinite;
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

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

</style>
