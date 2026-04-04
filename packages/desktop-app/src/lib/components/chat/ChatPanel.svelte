<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { chatStore } from '$lib/stores/chat-store.svelte';
  import { agentStore } from '$lib/stores/agent-store.svelte';
  import ChatMessage from './ChatMessage.svelte';
  import ChatInput from './ChatInput.svelte';
  import AgentSelector from '$lib/components/agents/AgentSelector.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ChatPanel');

  let messagesContainer: HTMLDivElement | undefined = $state();

  const messages = $derived(chatStore.messages);
  const isStreaming = $derived(chatStore.isStreaming);
  const selectedAgent = $derived(agentStore.selectedAgent);

  onMount(() => {
    log.debug('ChatPanel mounted');
    if (agentStore.agents.length === 0) {
      agentStore.refreshAgents();
    }
  });

  async function handleSend(content: string) {
    await chatStore.sendMessage(content);
    await scrollToBottom();
  }

  async function scrollToBottom() {
    await tick();
    if (messagesContainer) {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
  }

  // Auto-scroll when new messages arrive
  $effect(() => {
    // Access messages to create the dependency
    const _len = messages.length;
    scrollToBottom();
  });

  // Reset chat when agent changes
  let previousAgentId = $state<string | null>(agentStore.selectedAgentId ?? null);

  $effect(() => {
    const currentAgentId = agentStore.selectedAgentId;
    if (previousAgentId !== null && currentAgentId !== previousAgentId) {
      log.info('Agent changed, resetting chat session', { from: previousAgentId, to: currentAgentId });
      chatStore.reset();
    }
    previousAgentId = currentAgentId ?? null;
  });
</script>

<div class="chat-panel">
  <!-- Header -->
  <div class="chat-header">
    <div class="chat-header-left">
      <h2 class="chat-title">AI Chat</h2>
      {#if selectedAgent}
        <span class="agent-label">{selectedAgent.name}</span>
      {/if}
    </div>
    <div class="chat-header-right">
      <AgentSelector />
    </div>
  </div>

  <!-- Message list -->
  <div class="messages-container" bind:this={messagesContainer} role="list" aria-label="Chat messages">
    {#if messages.length === 0}
      <div class="empty-chat">
        <div class="empty-chat-icon">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="48" height="48">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
          </svg>
        </div>
        <p class="empty-chat-text">Start a conversation with your AI assistant</p>
        <p class="empty-chat-hint">Type a message below or select a different agent above</p>
      </div>
    {:else}
      {#each messages as message (message.id)}
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
  </div>

  <!-- Input -->
  <ChatInput
    onSend={handleSend}
    disabled={isStreaming}
    placeholder={isStreaming ? 'AI is responding...' : 'Type a message...'}
  />
</div>

<style>
  .chat-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .chat-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
  }

  .chat-header-left {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .chat-title {
    font-size: 1rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
  }

  .agent-label {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
  }

  .chat-header-right {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .messages-container {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0;
  }

  .empty-chat {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: 2rem;
    text-align: center;
  }

  .empty-chat-icon {
    color: hsl(var(--muted-foreground) / 0.5);
    margin-bottom: 1rem;
  }

  .empty-chat-text {
    font-size: 1rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    margin: 0 0 0.5rem;
  }

  .empty-chat-hint {
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
</style>
