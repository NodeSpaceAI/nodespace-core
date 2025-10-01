<!--
  DateNode - Individual date node component that wraps BaseNode

  This wrapper provides date-specific formatting and validation for date-type nodes.
  Date nodes cannot be created via slash commands - they exist implicitly for all dates.

  Follows the *Node pattern (like TaskNode) for individual node components.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import type { NodeViewerProps } from '$lib/types/nodeViewers.js';

  // Props following the NodeViewer interface
  let {
    nodeId,
    content = '',
    autoFocus = false,
    nodeType = 'date',
    inheritHeaderLevel = 0,
    children = []
  }: NodeViewerProps = $props();

  const dispatch = createEventDispatcher();

  // Parse date from content - expects YYYY-MM-DD format or similar
  function parseDate(content: string): Date | null {
    if (!content.trim()) return new Date(); // Default to today if empty

    const dateMatch = content.match(/^\d{4}-\d{2}-\d{2}/);
    if (dateMatch) {
      return new Date(dateMatch[0]);
    }

    // Try to parse as a general date
    const parsed = Date.parse(content.trim());
    return isNaN(parsed) ? null : new Date(parsed);
  }

  // Format date for display
  const parsedDate = $derived(parseDate(content));
  const formattedDate = $derived(
    parsedDate
      ? parsedDate.toLocaleDateString('en-US', {
          year: 'numeric',
          month: 'long',
          day: 'numeric'
        })
      : content
  );

  // Enhanced content with date formatting
  const displayContent = $derived(parsedDate ? `📅 ${formattedDate}` : content);
</script>

<div class="date-node-viewer">
  <div class="date-icon">
    <Icon name="calendar" size={16} color="hsl(var(--muted-foreground))" />
  </div>
  <BaseNode
    {nodeId}
    content={displayContent}
    {autoFocus}
    {nodeType}
    headerLevel={inheritHeaderLevel}
    {children}
    on:createNewNode
    on:contentChanged={(e) => {
      // Strip formatting when content changes
      const rawContent = e.detail.content.replace(/^📅\s*/, '');
      dispatch('contentChanged', { content: rawContent });
    }}
    on:indentNode
    on:outdentNode
    on:navigateArrow
    on:combineWithPrevious
    on:deleteNode
    on:focus
    on:blur
  />
</div>

<style>
  .date-node-viewer {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
  }

  .date-icon {
    display: flex;
    align-items: center;
    margin-top: 0.125rem; /* Align with text baseline */
    flex-shrink: 0;
  }
</style>
