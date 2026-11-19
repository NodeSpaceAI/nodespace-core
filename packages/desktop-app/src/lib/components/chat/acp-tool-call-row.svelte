<!--
  AcpToolCallRow - one tool call of an ACP turn: its kind, title and status.
  The status is always written out; the glyph beside it only repeats it.
-->

<script lang="ts">
  import type { AiChatMessageNode } from '$lib/types/ai-chat-node';
  import { isToolActive, toolKindLabel, toolStatusLabel } from '$lib/utils/acp-chat';

  let { message }: { message: AiChatMessageNode } = $props();

  const status = $derived(message.toolStatus);
  const GLYPHS = { completed: '✓', failed: '✕', cancelled: '–' } as const;
  const glyph = $derived(
    status === 'completed' || status === 'failed' || status === 'cancelled'
      ? GLYPHS[status]
      : '…'
  );
</script>

<div
  class="acp-tool"
  class:failed={status === 'failed'}
  role="listitem"
  data-acp-row="tool"
  data-message-id={message.id}
  data-tool-call-id={message.toolCallId}
  data-status={status ?? 'pending'}
>
  <span class="acp-tool-kind">{toolKindLabel(message.toolKind)}</span>
  <span class="acp-tool-title">{message.toolTitle ?? 'Tool call'}</span>
  <span class="acp-tool-status" aria-busy={isToolActive(status)}>
    <span class="acp-tool-glyph" aria-hidden="true">{glyph}</span>
    {toolStatusLabel(status)}
  </span>
</div>

<style>
  .acp-tool {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: 0.25rem 1rem;
    padding: 0.375rem 0.75rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--muted) / 0.5);
    font-size: 0.8125rem;
  }

  .acp-tool.failed {
    border-color: hsl(var(--destructive));
  }

  .acp-tool-kind {
    flex-shrink: 0;
    padding: 0 0.375rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-tool-title {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .acp-tool-status {
    flex-shrink: 0;
    color: hsl(var(--muted-foreground));
  }

  .acp-tool.failed .acp-tool-status {
    color: hsl(var(--destructive));
    font-weight: 600;
  }

  .acp-tool-glyph {
    display: inline-block;
    margin-right: 0.25rem;
  }
</style>
