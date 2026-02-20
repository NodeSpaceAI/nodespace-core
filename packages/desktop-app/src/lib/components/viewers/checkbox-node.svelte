<!--
  CheckboxNode - Wraps BaseNode with checkbox-specific functionality

  Responsibilities:
  - Derives checked state from content prefix ("- [x] " = checked, "- [ ] " = unchecked)
  - Hides the "- [ ] " / "- [x] " prefix in display mode via displayContent
  - Handles icon click to toggle checked state by rewriting the content prefix
  - No separate state property — checked/unchecked is encoded entirely in content
  - Forwards all other events to BaseNode

  Content format: "- [ ] Buy milk" (unchecked) / "- [x] Buy milk" (checked)
  The full markdown line is stored as-is; checked state is derived from the prefix.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

  let {
    nodeId,
    nodeType: propsNodeType = 'checkbox',
    autoFocus = false,
    content: propsContent = '',
    children: propsChildren = [],
    metadata = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
    metadata?: Record<string, unknown>;
  } = $props();

  const dispatch = createEventDispatcher();

  let sharedNode = $derived(sharedNodeStore.getNode(nodeId));
  let childIds = $derived(structureTree.getChildren(nodeId));

  let content = $derived(sharedNode?.content ?? propsContent);
  let nodeType = $derived(sharedNode?.nodeType ?? propsNodeType);
  let children = $derived(childIds ?? propsChildren);

  // Derive checked state purely from content — no separate property needed
  let isChecked = $derived(content.startsWith('- [x] ') || content.startsWith('- [X] '));

  // Pass checked state via metadata so the icon registry can render the correct icon state.
  // This is ephemeral/derived — it is never written as a node property.
  let checkboxMetadata = $derived({
    ...metadata,
    taskState: isChecked ? 'completed' : 'pending'
  });

  // Display mode: strip the "- [ ] " / "- [x] " prefix so only the text is shown
  let displayContent = $derived(content.replace(/^- \[[ xX]\] /, ''));

  /**
   * Icon click toggles the checked state by rewriting the content prefix.
   * No property writes — state lives entirely in content.
   */
  function handleIconClick() {
    let newContent: string;
    if (isChecked) {
      newContent = '- [ ] ' + content.replace(/^- \[[xX]\] /, '');
    } else {
      newContent = '- [x] ' + content.replace(/^- \[ \] /, '');
    }
    dispatch('contentChanged', { content: newContent });
  }

  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<div
  class="checkbox-node-wrapper"
  class:checked={isChecked}
  role="group"
  aria-label="Checkbox node"
>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content
    {displayContent}
    {children}
    metadata={checkboxMetadata}
    on:iconClick={handleIconClick}
    on:createNewNode={forwardEvent('createNewNode')}
    on:contentChanged={forwardEvent('contentChanged')}
    on:indentNode={forwardEvent('indentNode')}
    on:outdentNode={forwardEvent('outdentNode')}
    on:navigateArrow={forwardEvent('navigateArrow')}
    on:combineWithPrevious={forwardEvent('combineWithPrevious')}
    on:deleteNode={forwardEvent('deleteNode')}
    on:focus={forwardEvent('focus')}
    on:blur={forwardEvent('blur')}
    on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
    on:slashCommandSelected={forwardEvent('slashCommandSelected')}
    on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
  />
</div>

<style>
  .checkbox-node-wrapper {
    position: relative;
  }

  /* Strike-through text when checked, matching task-completed pattern */
  .checkbox-node-wrapper.checked :global(.node__content) {
    text-decoration: line-through;
    opacity: 0.6;
  }
</style>
