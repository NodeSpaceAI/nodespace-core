<!--
  NodeCardInline Component

  Renders a nodespace:// URI as a rich inline node card in AI chat messages.
  Shows type icon, node title, type badge, and task status when applicable.
  Wraps in an anchor tag so the global click handler still works.
-->

<script lang="ts">
  import { createLogger } from '$lib/utils/logger';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { getNode } from '$lib/services/tauri-commands';
  import { TaskNodeHelpers, isTaskNode } from '$lib/types/task-node';

  const log = createLogger('NodeCardInline');

  let { nodeId, displayText = '' }: { nodeId: string; displayText?: string } = $props();

  const nodeTypeIcons: Record<string, string> = {
    text: '📝',
    task: '☑️',
    date: '📅',
    document: '📄',
    'ai-chat': '🤖',
    user: '👤',
    entity: '🏷️',
    query: '🔍'
  };

  let node = $derived(sharedNodeStore.getNode(nodeId));
  let fetchAttempted = $state(false);

  // Fetch from backend if not in store
  $effect(() => {
    if (!node && !fetchAttempted) {
      fetchAttempted = true;
      getNode(nodeId).then((fetched) => {
        if (fetched) {
          sharedNodeStore.setNode(fetched, { type: 'database', reason: 'node-card-fetch' }, true);
        }
      }).catch((e) => {
        log.warn(`Failed to fetch node ${nodeId}:`, e);
      });
    }
  });

  let title = $derived(node?.content?.slice(0, 80) || displayText || nodeId.slice(0, 8));
  let nodeType = $derived(node?.nodeType || 'unknown');
  let icon = $derived(nodeTypeIcons[nodeType] || '📄');
  let taskStatus = $derived(
    node && isTaskNode(node)
      ? TaskNodeHelpers.getStatusDisplayName(node.status)
      : null
  );
</script>

<a
  href="nodespace://{nodeId}"
  class="ns-node-card-inline ns-node-card-inline--{nodeType}"
  data-node-id={nodeId}
>
  <span class="node-card-icon">{icon}</span>
  <span class="node-card-title">{title}</span>
  {#if node}
    <span class="node-card-type">{nodeType}</span>
  {/if}
  {#if taskStatus}
    <span class="node-card-status">{taskStatus}</span>
  {/if}
  {#if !node && fetchAttempted}
    <span class="node-card-type node-card-type--unknown">unknown</span>
  {/if}
</a>

<style>
  .ns-node-card-inline {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.125rem 0.5rem;
    border-radius: 0.375rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--muted) / 0.3);
    text-decoration: none;
    color: hsl(var(--foreground));
    font-size: 0.85em;
    line-height: 1.4;
    cursor: pointer;
    max-width: 100%;
    vertical-align: middle;
  }

  .ns-node-card-inline:hover {
    background: hsl(var(--muted) / 0.5);
    border-color: hsl(var(--primary) / 0.4);
  }

  .node-card-icon {
    flex-shrink: 0;
    font-size: 0.9em;
  }

  .node-card-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }

  .node-card-type {
    flex-shrink: 0;
    padding: 0 0.25rem;
    border-radius: 0.25rem;
    background: hsl(var(--primary) / 0.1);
    color: hsl(var(--primary));
    font-size: 0.75em;
    font-weight: 500;
  }

  .node-card-type--unknown {
    background: hsl(var(--muted));
    color: hsl(var(--muted-foreground));
  }

  .node-card-status {
    flex-shrink: 0;
    padding: 0 0.25rem;
    border-radius: 0.25rem;
    background: hsl(var(--chart-2) / 0.15);
    color: hsl(var(--chart-2));
    font-size: 0.75em;
    font-weight: 500;
  }

  /* Type-specific border colors */
  .ns-node-card-inline--task {
    border-color: hsl(var(--chart-2) / 0.3);
  }
  .ns-node-card-inline--date {
    border-color: hsl(var(--chart-3) / 0.3);
  }
  .ns-node-card-inline--ai-chat {
    border-color: hsl(var(--chart-5) / 0.3);
  }
</style>
