<script lang="ts">
  import type { ToolExecutionRecord } from '$lib/types/agent-types';

  let { toolExecution }: { toolExecution: ToolExecutionRecord } = $props();

  const argsPreview = $derived(() => {
    try {
      const str = typeof toolExecution.args === 'string'
        ? toolExecution.args
        : JSON.stringify(toolExecution.args, null, 2);
      return str.length > 120 ? str.slice(0, 120) + '...' : str;
    } catch {
      return String(toolExecution.args);
    }
  });

  const resultPreview = $derived(() => {
    try {
      const str = typeof toolExecution.result === 'string'
        ? toolExecution.result
        : JSON.stringify(toolExecution.result, null, 2);
      return str.length > 200 ? str.slice(0, 200) + '...' : str;
    } catch {
      return String(toolExecution.result);
    }
  });
</script>

<details class="tool-call-card" class:tool-error={toolExecution.is_error}>
  <summary class="tool-call-summary">
    <span class="tool-icon">
      {#if toolExecution.is_error}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
          <circle cx="12" cy="12" r="10" />
          <line x1="15" y1="9" x2="9" y2="15" />
          <line x1="9" y1="9" x2="15" y2="15" />
        </svg>
      {:else}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="14" height="14">
          <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
        </svg>
      {/if}
    </span>
    <span class="tool-name">{toolExecution.name}</span>
    <span class="tool-duration">{toolExecution.duration_ms}ms</span>
  </summary>
  <div class="tool-call-details">
    <div class="tool-section">
      <span class="tool-section-label">Arguments</span>
      <pre class="tool-code">{argsPreview()}</pre>
    </div>
    <div class="tool-section">
      <span class="tool-section-label">Result</span>
      <pre class="tool-code">{resultPreview()}</pre>
    </div>
  </div>
</details>

<style>
  .tool-call-card {
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    margin: 0.5rem 0;
    font-size: 0.8125rem;
    overflow: hidden;
  }

  .tool-call-card.tool-error {
    border-color: hsl(var(--destructive) / 0.5);
  }

  .tool-call-summary {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    background: hsl(var(--muted) / 0.5);
    user-select: none;
    list-style: none;
  }

  .tool-call-summary::-webkit-details-marker {
    display: none;
  }

  .tool-call-summary:hover {
    background: hsl(var(--muted));
  }

  .tool-icon {
    display: flex;
    align-items: center;
    color: hsl(var(--muted-foreground));
  }

  .tool-error .tool-icon {
    color: hsl(var(--destructive));
  }

  .tool-name {
    font-weight: 500;
    color: hsl(var(--foreground));
    font-family: monospace;
  }

  .tool-duration {
    margin-left: auto;
    color: hsl(var(--muted-foreground));
    font-size: 0.75rem;
  }

  .tool-call-details {
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    background: hsl(var(--background));
  }

  .tool-section-label {
    font-size: 0.6875rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: hsl(var(--muted-foreground));
    display: block;
    margin-bottom: 0.25rem;
  }

  .tool-code {
    margin: 0;
    padding: 0.375rem 0.5rem;
    background: hsl(var(--muted) / 0.5);
    border-radius: 0.25rem;
    font-family: monospace;
    font-size: 0.75rem;
    white-space: pre-wrap;
    word-break: break-all;
    color: hsl(var(--foreground));
    max-height: 10rem;
    overflow-y: auto;
  }
</style>
