<!--
  TextNode - Wraps BaseNode for text content editing

  Individual text node component that provides smart multiline behavior
  based on header level:
  - Headers (h1-h6): Single-line only for semantic integrity
  - Regular text: Multi-line with Shift+Enter support
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';

  // Props
  let {
    nodeId,
    autoFocus = false,
    content = '',
    nodeType = 'text',
    children = []
  }: {
    nodeId: string;
    autoFocus?: boolean;
    content?: string;
    nodeType?: string;
    children?: string[];
  } = $props();

  // Internal reactive state
  let internalContent = $state(content);

  // Sync internalContent when content prop changes externally
  $effect(() => {
    internalContent = content;
  });

  // Text nodes always allow multiline editing
  const editableConfig = {
    allowMultiline: true
  };

  // Event dispatcher - just forward all BaseNode events
  const dispatch = createEventDispatcher();
</script>

<BaseNode
  {nodeId}
  {nodeType}
  {autoFocus}
  content={internalContent}
  {children}
  {editableConfig}
  on:createNewNode
  on:contentChanged={(e) => {
    internalContent = e.detail.content;
    dispatch('contentChanged', e.detail);
  }}
  on:indentNode
  on:outdentNode
  on:navigateArrow
  on:combineWithPrevious
  on:slashCommandSelected
  on:nodeTypeChanged
  on:deleteNode
  on:focus
  on:blur
/>
