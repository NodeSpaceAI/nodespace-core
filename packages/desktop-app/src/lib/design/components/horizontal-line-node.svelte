<!--
  HorizontalLineNode - Wraps BaseNode with horizontal rule display

  Responsibilities:
  - Displays a styled horizontal rule in view mode
  - Shows raw --- content in edit mode
  - Leaf node - cannot have children, does not accept content merges
  - Forwards all events to BaseNode
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';

  let {
    nodeId,
    nodeType = 'horizontal-line',
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

  // HR nodes are single-line, don't accept merges
  const editableConfig = { allowMultiline: false, allowMergeInto: false };

  // Check if this node is being edited
  let isEditing = $derived(focusManager.editingNodeId === nodeId);

  // Metadata - disable markdown processing
  let hrMetadata = $derived({
    disableMarkdown: true
  });

  // Display content: empty in view mode (we render the <hr> via CSS)
  // In edit mode BaseNode shows the raw content
  let displayContent = $derived(isEditing ? content : '');

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

<div class="horizontal-line-node-wrapper" class:viewing={!isEditing}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content
    {displayContent}
    {children}
    {editableConfig}
    metadata={hrMetadata}
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
  .horizontal-line-node-wrapper {
    position: relative;
  }

  /* In view mode, show a horizontal rule via CSS on the content area */
  .horizontal-line-node-wrapper.viewing :global(.node__content) {
    color: transparent;
    position: relative;
    min-height: 1.5rem;
    user-select: none;
  }

  .horizontal-line-node-wrapper.viewing :global(.node__content)::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 1px;
    background: hsl(var(--border));
    pointer-events: none;
  }
</style>
