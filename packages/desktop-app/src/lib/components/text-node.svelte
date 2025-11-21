<!--
  TextNode - Wraps BaseNode for text content editing

  Individual text node component that provides smart multiline behavior
  based on header level:
  - Headers (h1-h6): Single-line only for semantic integrity
  - Regular text: Multi-line with Shift+Enter support
-->

<script lang="ts">
  import BaseNode from '$lib/design/components/base-node.svelte';
  import { nodeData } from '$lib/stores/reactive-node-data.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

  // Props
  let {
    nodeId,
    autoFocus = false,
    content: propsContent = '',
    nodeType: propsNodeType = 'text',
    children: propsChildren = []
  }: {
    nodeId: string;
    autoFocus?: boolean;
    content?: string;
    nodeType?: string;
    children?: string[];
  } = $props();

  // Use reactive stores directly instead of relying on props
  // Components query the stores for current data and re-render automatically when data changes
  let node = $derived(nodeData.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));

  // Derive props from stores with fallback to passed props for backward compatibility
  let content = $derived(node?.content ?? propsContent);
  let nodeType = $derived(node?.nodeType ?? propsNodeType);
  let children = $derived(childIds ?? propsChildren);

  // Text nodes always allow multiline editing
  const editableConfig = {
    allowMultiline: true
  };

  // REFACTOR (Issue #316): No longer need createEventDispatcher
  // Svelte automatically forwards on:* events from child components to parent
  // when no handler is specified (event forwarding is a built-in Svelte feature)
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
