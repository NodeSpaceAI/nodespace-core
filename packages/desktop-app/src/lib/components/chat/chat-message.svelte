<script lang="ts">
  /* global navigator */
  import type { DisplayMessage } from './types';
  import ChatMarkdown from './chat-markdown.svelte';

  let {
    message,
    isLatest = false,
    onSelectOption,
  }: {
    message: DisplayMessage;
    /** Whether this is the most recent message — options are only clickable here. */
    isLatest?: boolean;
    onSelectOption?: (_option: string) => void;
  } = $props();

  let showCopyButton = $state(false);
  let copied = $state(false);

  const isUser = $derived(message.role === 'user');
  const isAssistant = $derived(message.role === 'assistant');
  /**
   * The text to show above the option chips: `content` minus the bullet list
   * (rendered as chips instead), keeping the backend's opener + question
   * framing ("I can take that a couple of ways...") intact rather than
   * dropping it in favor of the bare `question` field.
   */
  const clarifyHeaderText = $derived(
    message.options && message.options.length > 0
      ? message.content.split('\n\n')[0]
      : message.content
  );

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
      <div class="message-content">
        {#if isAssistant}
          <ChatMarkdown content={clarifyHeaderText} />
        {:else}
          {message.content}
        {/if}
      </div>
    {/if}

    {#if isAssistant && message.options && message.options.length > 0}
      <div class="clarify-options" role="group" aria-label="Choose one">
        {#each message.options as option, optionIndex (optionIndex)}
          <button
            class="clarify-option"
            type="button"
            disabled={!isLatest || !onSelectOption}
            onclick={() => onSelectOption?.(option)}
          >
            {option}
          </button>
        {/each}
      </div>
    {/if}

    {#if isAssistant && message.reasoning}
      <details class="reasoning-block">
        <summary class="reasoning-summary">Reasoning</summary>
        <div class="reasoning-content">
          <ChatMarkdown content={message.reasoning} />
        </div>
      </details>
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
    word-break: break-word;
  }

  .user-message .message-content {
    white-space: pre-wrap;
  }

  .clarify-options {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    margin-top: 0.625rem;
  }

  .clarify-option {
    text-align: left;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.8125rem;
    font-family: inherit;
    cursor: pointer;
  }

  .clarify-option:not(:disabled):hover {
    background: hsl(var(--accent));
    border-color: hsl(var(--ring));
  }

  .clarify-option:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .reasoning-block {
    margin-top: 0.5rem;
    padding-top: 0.5rem;
    border-top: 1px solid hsl(var(--border));
    font-size: 0.8125rem;
  }

  .reasoning-summary {
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    user-select: none;
    list-style: none;
  }

  .reasoning-summary::-webkit-details-marker {
    display: none;
  }

  .reasoning-summary::before {
    content: '▸';
    display: inline-block;
    margin-right: 0.375rem;
  }

  .reasoning-block[open] .reasoning-summary::before {
    transform: rotate(90deg);
  }

  .reasoning-content {
    margin-top: 0.375rem;
    color: hsl(var(--muted-foreground));
    word-break: break-word;
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
  }

  .copy-button:hover {
    color: hsl(var(--foreground));
  }
</style>
