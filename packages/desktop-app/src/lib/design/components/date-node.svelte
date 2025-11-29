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
  import type { NodeComponentProps } from '$lib/types/node-viewers.js';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

  // Props following the NodeComponentProps interface (for individual node components)
  let {
    nodeId,
    content: propsContent = '',
    autoFocus = false,
    nodeType: propsNodeType = 'date',
    children: propsChildren = []
  }: NodeComponentProps = $props();

  const dispatch = createEventDispatcher();

  // Use sharedNodeStore as single source of truth for cross-pane reactivity
  // This ensures content changes from other panes are immediately reflected
  // Issue #679: Migrated from nodeData (which was never receiving updates)
  let node = $derived(sharedNodeStore.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));

  // Derive props from stores with fallback to passed props for backward compatibility
  let content = $derived(node?.content ?? propsContent);
  let nodeType = $derived(node?.nodeType ?? propsNodeType);
  let children = $derived(childIds ?? propsChildren);

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
