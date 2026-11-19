<!--
  AcpCollapsibleRow - a plan or a thought of an ACP turn: Markdown under a
  native disclosure. The plan opens by default; the agent's thinking does not.
-->

<script lang="ts">
  import ChatMarkdown from './chat-markdown.svelte';

  let {
    id,
    row,
    label,
    content,
    open = false
  }: {
    id: string;
    row: 'plan' | 'thought';
    label: string;
    content: string;
    open?: boolean;
  } = $props();
</script>

<div class="acp-collapsible" role="listitem" data-acp-row={row} data-message-id={id}>
  <details {open}>
    <summary>{label}</summary>
    <div class="acp-collapsible-body">
      <ChatMarkdown {content} />
    </div>
  </details>
</div>

<style>
  .acp-collapsible {
    margin: 0.25rem 1rem;
    font-size: 0.8125rem;
  }

  summary {
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    user-select: none;
  }

  summary:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }

  .acp-collapsible-body {
    margin-top: 0.375rem;
    padding-left: 0.75rem;
    border-left: 2px solid hsl(var(--border));
    word-break: break-word;
  }
</style>
