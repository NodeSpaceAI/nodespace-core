<!--
  TextNode - Wraps BaseNode for text content editing

  Individual text node component that provides smart multiline behavior
  based on header level:
  - Headers (h1-h6): Single-line only for semantic integrity
  - Regular text: Multi-line with Shift+Enter support
-->

<script lang="ts">
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

  // Text nodes always allow multiline editing
  const editableConfig = {
    allowMultiline: true
  };

  // REFACTOR (Issue #316): No longer need createEventDispatcher - events are forwarded directly
</script>

<!-- REFACTOR (Issue #316): Removed $effect and internalContent state, using bind:content instead -->
<BaseNode
  {nodeId}
  {nodeType}
  {autoFocus}
  bind:content
  {children}
  {editableConfig}
  on:createNewNode
  on:contentChanged
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
