<!--
  AiChatAcpNodeViewer - Page-level viewer for ACP AI chat nodes

  `ai-chat-acp` is a conversation with an external agent driven over the Agent
  Client Protocol (ADR-088). The agent's turn is stored as one `ai-chat-message`
  child per part, in order: reply text, thinking, tool calls (updated in place),
  plans (Markdown, replaced whole) and system notices. This viewer derives the
  conversation from the chat's children and renders each part as its own row.

  What exists only while a session runs is not stored and arrives as an
  optional `session` source: the pending permission request, whether a turn is
  running, context usage and the controls (send, stop, end, resume). Without a
  source the viewer is a read-only view of the stored conversation.

  The AI chat Labs flag gates the viewer, as it gates the sidebar's AI Chats
  section.
-->

<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import { appendUserMessage, discardUnsentMessage } from '$lib/services/ai-chat-messages';
  import { labsFlags } from '$lib/stores/labs-flags.svelte';
  import ChatMessage from '$lib/components/chat/chat-message.svelte';
  import ChatInput from '$lib/components/chat/chat-input.svelte';
  import AcpPermissionCard from '$lib/components/chat/acp-permission-card.svelte';
  import AcpToolCallRow from '$lib/components/chat/acp-tool-call-row.svelte';
  import AcpCollapsibleRow from '$lib/components/chat/acp-collapsible-row.svelte';
  import type { DisplayMessage } from '$lib/components/chat/types';
  import type { AcpSessionSource } from '$lib/types/acp-session';
  import type { AiChatAcpNode, AiChatMessageNode } from '$lib/types/ai-chat-node';
  import { isAiChatMessageNode, nodeToAiChatMessageNode } from '$lib/types/ai-chat-node';
  import { acpRowKind } from '$lib/utils/acp-chat';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import AiChatHeader from './ai-chat-header.svelte';
  import AcpSessionBar from './acp-session-bar.svelte';

  const log = createLogger('AiChatAcpNodeViewer');
  const VIEWER_ID = 'ai-chat-viewer';

  let {
    nodeId,
    session
  }: {
    nodeId: string;
    /** The running session's live state and controls; absent for a read-only view. */
    session?: AcpSessionSource;
  } = $props();

  let messagesContainer: HTMLDivElement | undefined = $state();
  let sendError = $state<string | null>(null);
  let isSending = $state(false);

  const node = $derived(sharedNodeStore.getNode(nodeId) as AiChatAcpNode | undefined);
  const sessionStatus = $derived(node?.sessionStatus ?? 'active');
  const agent = $derived(node?.agent ?? '');

  /** The chat's message children in conversation order; other children are not the conversation. */
  const messageNodes = $derived.by<AiChatMessageNode[]>(() => {
    const messages: AiChatMessageNode[] = [];
    for (const childId of structureTree.getChildren(nodeId)) {
      const child = sharedNodeStore.getNode(childId);
      if (child && isAiChatMessageNode(child)) messages.push(nodeToAiChatMessageNode(child));
    }
    return messages;
  });

  function toDisplay(message: AiChatMessageNode): DisplayMessage {
    return {
      id: message.id,
      role: message.role,
      content: message.content,
      toolExecutions: [],
      timestamp: new Date(message.timestamp ?? message.createdAt).getTime(),
      reasoning: message.reasoning,
      options: message.options
    };
  }

  const canPrompt = $derived(sessionStatus === 'active' && !!session?.sendPrompt);
  const turnRunning = $derived(session?.turnRunning ?? false);
  const errorText = $derived(sendError ?? session?.error ?? null);

  async function handleSend(content: string): Promise<void> {
    const text = content.trim();
    if (!text || !session?.sendPrompt || turnRunning || isSending) return;
    isSending = true;
    sendError = null;
    const messageId = appendUserMessage(nodeId, text, VIEWER_ID);
    try {
      await session.sendPrompt(text);
    } catch (err) {
      log.warn('Failed to send prompt', { error: toError(err).message });
      discardUnsentMessage(messageId, VIEWER_ID);
      sendError = `Could not send the message: ${toError(err).message}`;
    } finally {
      isSending = false;
    }
  }

  async function scrollToBottom(): Promise<void> {
    await tick();
    if (messagesContainer) messagesContainer.scrollTop = messagesContainer.scrollHeight;
  }

  onMount(async () => {
    try {
      await sharedNodeStore.loadChildrenForParent(nodeId);
    } catch (err) {
      log.warn('Failed to load chat messages', { error: toError(err).message });
    }
  });

  $effect(() => {
    void messageNodes.length;
    void session?.pendingPermission;
    scrollToBottom();
  });
</script>

<div class="ai-chat-viewer">
  <AiChatHeader {nodeId} />

  {#if !labsFlags.aiChatEnabled}
    <div class="acp-off" role="status">
      <p>AI chat is turned off. Turn it on in Settings, under Labs, to use this chat.</p>
    </div>
  {:else}
    <AcpSessionBar status={sessionStatus} {agent} {session} />

    <div
      class="chat-viewer-messages"
      bind:this={messagesContainer}
      role="list"
      aria-label="Chat conversation"
    >
      {#if messageNodes.length === 0}
        <p class="empty-conversation">No messages yet.</p>
      {/if}
      {#each messageNodes as message (message.id)}
        {@const kind = acpRowKind(message)}
        {#if kind === 'user' || kind === 'text'}
          <div role="none" data-acp-row={kind} data-message-id={message.id}>
            <ChatMessage message={toDisplay(message)} />
          </div>
        {:else if kind === 'tool'}
          <AcpToolCallRow {message} />
        {:else if kind === 'plan'}
          <AcpCollapsibleRow id={message.id} row="plan" label="Plan" content={message.content} open />
        {:else if kind === 'thought'}
          <AcpCollapsibleRow
            id={message.id}
            row="thought"
            label="Thinking"
            content={message.content}
          />
        {:else}
          <div class="acp-notice" role="listitem" data-acp-row="notice" data-message-id={message.id}>
            {message.content}
          </div>
        {/if}
      {/each}

      {#if session?.pendingPermission}
        <AcpPermissionCard
          request={session.pendingPermission}
          onAnswer={session.answerPermission
            ? (requestId, outcome) => session.answerPermission?.(requestId, outcome)
            : undefined}
        />
      {/if}

      {#if turnRunning}
        <div class="acp-working" role="status">Agent is working</div>
      {/if}
    </div>

    {#if errorText}
      <div class="send-error" role="alert">{errorText}</div>
    {/if}

    {#if canPrompt}
      <ChatInput
        onSend={handleSend}
        disabled={turnRunning || isSending}
        placeholder={turnRunning ? 'The agent is responding...' : 'Type a message...'}
      />
    {:else if sessionStatus === 'ended'}
      <p class="acp-ended-note">This session has ended.</p>
    {/if}
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

  .acp-off {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem 1rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.9375rem;
  }

  .chat-viewer-messages {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0;
  }

  .empty-conversation {
    margin: 2rem 1rem;
    text-align: center;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-notice {
    margin: 0.25rem 1rem;
    padding: 0.25rem 0.75rem;
    border-left: 3px solid hsl(var(--border));
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-working {
    padding: 0.5rem 1.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-ended-note {
    margin: 0;
    padding: 0.75rem 1rem;
    border-top: 1px solid hsl(var(--border));
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
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
</style>
