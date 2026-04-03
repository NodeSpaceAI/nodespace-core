<script lang="ts">
  let {
    onSend,
    disabled = false,
    placeholder = 'Type a message...',
  }: {
    onSend: (content: string) => void;
    disabled?: boolean;
    placeholder?: string;
  } = $props();

  let value = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  const canSend = $derived(value.trim().length > 0 && !disabled);

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      send();
    }
    if (event.key === 'Escape') {
      value = '';
      textareaEl?.blur();
    }
  }

  function send() {
    if (!canSend) return;
    onSend(value.trim());
    value = '';
    // Reset textarea height
    if (textareaEl) {
      textareaEl.style.height = 'auto';
    }
  }

  function autoResize() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = `${Math.min(textareaEl.scrollHeight, 160)}px`;
  }
</script>

<div class="chat-input-container">
  <textarea
    bind:this={textareaEl}
    bind:value
    oninput={autoResize}
    onkeydown={handleKeydown}
    {placeholder}
    {disabled}
    rows="1"
    class="chat-textarea"
    aria-label="Chat message input"
  ></textarea>
  <button
    class="send-button"
    onclick={send}
    disabled={!canSend}
    aria-label="Send message"
  >
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="18" height="18">
      <line x1="22" y1="2" x2="11" y2="13" />
      <polygon points="22 2 15 22 11 13 2 9 22 2" />
    </svg>
  </button>
</div>

<style>
  .chat-input-container {
    display: flex;
    align-items: flex-end;
    gap: 0.5rem;
    padding: 0.75rem 1rem;
    border-top: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  .chat-textarea {
    flex: 1;
    resize: none;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 0.625rem 0.75rem;
    font-size: 0.875rem;
    line-height: 1.5;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-family: inherit;
    min-height: 2.5rem;
    max-height: 10rem;
    overflow-y: auto;
  }

  .chat-textarea:focus {
    outline: none;
    border-color: hsl(var(--ring));
    box-shadow: 0 0 0 2px hsl(var(--ring) / 0.2);
  }

  .chat-textarea:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .chat-textarea::placeholder {
    color: hsl(var(--muted-foreground));
  }

  .send-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2.25rem;
    height: 2.25rem;
    border-radius: 0.5rem;
    border: none;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    cursor: pointer;
    flex-shrink: 0;
    transition: opacity 0.15s;
  }

  .send-button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .send-button:not(:disabled):hover {
    opacity: 0.9;
  }
</style>
