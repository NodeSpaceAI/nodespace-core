<script lang="ts">
  /* global navigator */
  import type { DisplayMessage } from '$lib/stores/chat-store.svelte';
  import ToolCallDisplay from './tool-call-display.svelte';

  let { message }: { message: DisplayMessage } = $props();

  let showCopyButton = $state(false);
  let copied = $state(false);

  const isUser = $derived(message.role === 'user');
  const isAssistant = $derived(message.role === 'assistant');

  async function copyContent() {
    try {
      await navigator.clipboard.writeText(message.content);
      copied = true;
      setTimeout(() => { copied = false; }, 1500);
    } catch {
      // Clipboard API may not be available in all contexts
    }
  }
</script>

<div
  class="chat-message"
  class:user-message={isUser}
  class:assistant-message={isAssistant}
  role="listitem"
  onmouseenter={() => { if (isAssistant) showCopyButton = true; }}
  onmouseleave={() => { showCopyButton = false; copied = false; }}
>
  <div class="message-bubble">
    {#if message.content}
      <div class="message-content">{message.content}</div>
    {/if}

    {#if message.toolExecutions.length > 0}
      <div class="tool-calls">
        {#each message.toolExecutions as toolExec (toolExec.tool_call_id)}
          <ToolCallDisplay toolExecution={toolExec} />
        {/each}
      </div>
    {/if}

    {#if isAssistant && showCopyButton}
      <button
        class="copy-button"
        onclick={copyContent}
        aria-label="Copy message"
      >
        {#if copied}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
            <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
          </svg>
        {/if}
      </button>
    {/if}
  </div>
</div>

<style>
  .chat-message {
    display: flex;
    padding: 0.5rem 1rem;
  }

  .user-message {
    justify-content: flex-end;
  }

  .assistant-message {
    justify-content: flex-start;
  }

  .message-bubble {
    max-width: 70%;
    padding: 0.75rem 1rem;
    border-radius: 0.75rem;
    position: relative;
    line-height: 1.5;
    font-size: 0.875rem;
  }

  .user-message .message-bubble {
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border-bottom-right-radius: 0.25rem;
  }

  .assistant-message .message-bubble {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
    border-bottom-left-radius: 0.25rem;
  }

  .message-content {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .tool-calls {
    margin-top: 0.5rem;
  }

  .copy-button {
    position: absolute;
    top: 0.375rem;
    right: 0.375rem;
    background: hsl(var(--background) / 0.8);
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    padding: 0.25rem;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.15s;
  }

  .copy-button:hover {
    color: hsl(var(--foreground));
  }
</style>
