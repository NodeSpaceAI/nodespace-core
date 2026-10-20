<!--
  TableNode - Wraps BaseNode with GFM table rendering

  Responsibilities:
  - Parses GFM markdown table syntax into rendered HTML table
  - Supports column alignment (left, center, right) from delimiter row
  - Shows rendered table in view mode, raw markdown in edit mode
  - Leaf node - cannot have children, does not accept content merges
  - Forwards all events to BaseNode
-->

<script lang="ts">
  import { createEventDispatcher, tick } from 'svelte';
  import BaseNode from './base-node.svelte';
  import ViewModeRenderer from './view-mode-renderer.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { parseTable } from './table-markdown';

  let {
    nodeId,
    nodeType = 'table',
    autoFocus = false,
    content = $bindable(''),
    children = []
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
  } = $props();

  const dispatch = createEventDispatcher();

  // Table nodes are multiline, don't accept merges
  const editableConfig = { allowMultiline: true, allowMergeInto: false };

  // Metadata - disable markdown processing (we render our own HTML)
  let tableMetadata = $derived({
    disableMarkdown: true
  });

  // Check if this node is being edited
  let isEditing = $derived(focusManager.editingNodeId === nodeId);

  // Parse the table reactively
  let parsedTable = $derived(parseTable(content));

  // Pass real content so adjustHeight() has multiline text to measure on edit entry.
  // customViewContent overrides view rendering entirely — raw markdown is never shown.
  let displayContent = $derived(content);

  // Measured height of rendered table — updated reactively via bind:clientHeight
  let tableViewHeight = $state(0);

  // Ref to wrapper div for scoping DOM queries in the $effect
  let wrapperElement = $state<HTMLDivElement | undefined>(undefined);

  // When entering edit mode, apply the last-known table height to the textarea
  // so there is no layout shift. After this, adjustHeight() takes over on each keystroke.
  $effect(() => {
    if (isEditing && tableViewHeight > 0) {
      tick().then(() => {
        const textarea = wrapperElement?.querySelector<HTMLTextAreaElement>('textarea');
        if (textarea) {
          textarea.style.height = `${tableViewHeight}px`;
        }
      });
    }
  });

  function handleContentChange(event: CustomEvent<{ content: string }>) {
    content = event.detail.content;
    dispatch('contentChanged', event.detail);
  }

  function handleCreateNewNode(event: CustomEvent) {
    dispatch('createNewNode', event.detail);
  }

  function handleNodeTypeChanged(event: CustomEvent) {
    dispatch('nodeTypeChanged', event.detail);
  }

  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

{#snippet tableViewContent()}
  {#if parsedTable}
    <table bind:clientHeight={tableViewHeight}>
        <thead>
          <tr>
            {#each parsedTable.headers as header, i}
              <th style="text-align: {parsedTable.alignments[i] || 'left'}">
                <ViewModeRenderer content={header} />
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each parsedTable.rows as row}
            <tr>
              {#each row as cell, i}
                <td style="text-align: {parsedTable.alignments[i] || 'left'}">
                  <ViewModeRenderer content={cell} />
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
    </table>
  {:else}
    <!-- Not (yet) a complete GFM table: show the source as plain text rather than nothing -->
    <ViewModeRenderer {content} disableMarkdown={true} />
  {/if}
{/snippet}

<div class="table-node-wrapper" bind:this={wrapperElement}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content
    {displayContent}
    {children}
    {editableConfig}
    metadata={tableMetadata}
    customViewContent={tableViewContent}
    on:createNewNode={handleCreateNewNode}
    on:contentChanged={handleContentChange}
    on:indentNode={forwardEvent('indentNode')}
    on:outdentNode={forwardEvent('outdentNode')}
    on:navigateArrow={forwardEvent('navigateArrow')}
    on:combineWithPrevious={forwardEvent('combineWithPrevious')}
    on:deleteNode={forwardEvent('deleteNode')}
    on:focus={forwardEvent('focus')}
    on:blur={forwardEvent('blur')}
    on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    on:slashCommandSelected={forwardEvent('slashCommandSelected')}
    on:nodeTypeChanged={handleNodeTypeChanged}
    on:iconClick={forwardEvent('iconClick')}
  />
</div>

<style>
  .table-node-wrapper {
    width: 100%;
  }

  /* Padding inside view content area to preserve visual spacing */
  .table-node-wrapper :global(.node__content--view) {
    padding-block: 0.25rem;
  }

  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.875rem;
    line-height: 1.5;
  }

  th {
    background: hsl(var(--muted));
    font-weight: 600;
    padding: 0.375rem 0.75rem;
    border-bottom: 2px solid hsl(var(--border));
    text-align: left;
  }

  td {
    padding: 0.375rem 0.75rem;
    border-bottom: 1px solid hsl(var(--border));
  }

  /* No outer borders, no vertical lines - clean minimal design */
  tr:last-child td {
    border-bottom: none;
  }
</style>
