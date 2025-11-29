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
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

  const dispatch = createEventDispatcher();

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

  // Use sharedNodeStore as single source of truth for cross-pane reactivity
  // This ensures content changes from other panes are immediately reflected
  // Issue #679: Migrated from nodeData (which was never receiving updates)
  let node = $derived(sharedNodeStore.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));

  // Derive props from stores with fallback to passed props for backward compatibility
  let content = $derived(node?.content ?? propsContent);
  let nodeType = $derived(node?.nodeType ?? propsNodeType);
  let children = $derived(childIds ?? propsChildren);

  // Text nodes always allow multiline editing
  const editableConfig = {
    allowMultiline: true
  };

  // Handle nodeTypeChanged event and forward to parent
  // Explicit dispatch is needed (vs automatic on: forwarding) because nodeTypeChanged
  // carries detail data that must be preserved when bubbling up through TextNode
  function handleNodeTypeChanged(e: CustomEvent) {
    dispatch('nodeTypeChanged', e.detail);
  }
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
  on:nodeTypeChanged={handleNodeTypeChanged}
  on:deleteNode
  on:focus
  on:blur
/>
