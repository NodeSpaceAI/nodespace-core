<!--
  CheckboxNode - Wraps BaseNode with checkbox-specific functionality

  Responsibilities:
  - Parses content to determine checked state ("- [x] text" vs "- [ ] text")
  - Renders a native checkbox + text display
  - Toggles update the content string in place (no properties written)
  - No task management semantics — pure content node like a heading or list item

  Content format: "- [ ] Buy milk" (unchecked) / "- [x] Buy milk" (checked)
  The full markdown line is stored as-is; state is encoded in the content.
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

  let isChecked = $derived(content.startsWith('- [x] ') || content.startsWith('- [X] '));

  function toggleCheckbox() {
    let newContent: string;
    if (isChecked) {
      // Replace "- [x] " or "- [X] " prefix with "- [ ] "
      newContent = '- [ ] ' + content.replace(/^- \[[xX]\] /, '');
    } else {
      // Replace "- [ ] " prefix with "- [x] "
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
  class:has-children={children.length > 0}
  role="group"
  aria-label="Checkbox node"
>
  <button
    class="checkbox-toggle"
    type="button"
    aria-label={isChecked ? 'Uncheck' : 'Check'}
    aria-checked={isChecked}
    role="checkbox"
    onclick={toggleCheckbox}
  >
    <span class="checkbox-indicator" class:checked={isChecked}>
      {#if isChecked}
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
          <path d="M1.5 5L4 7.5L8.5 2.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      {/if}
    </span>
  </button>

  <div class="checkbox-content" class:checked={isChecked}>
    <BaseNode
      {nodeId}
      {nodeType}
      {autoFocus}
      bind:content
      {children}
      metadata={{...metadata}}
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
</div>

<style>
  .checkbox-node-wrapper {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    position: relative;
    width: 100%;
  }

  /* Ring indicator when node has children (matches TaskNode pattern) */
  .checkbox-node-wrapper.has-children .checkbox-toggle {
    outline: 2px solid hsl(var(--border));
    outline-offset: 1px;
  }

  .checkbox-toggle {
    flex-shrink: 0;
    margin-top: 0.2rem;
    width: 1rem;
    height: 1rem;
    padding: 0;
    background: transparent;
    border: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .checkbox-indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1rem;
    height: 1rem;
    border: 1.5px solid hsl(var(--muted-foreground));
    border-radius: 0.2rem;
    background: transparent;
    color: hsl(var(--foreground));
    transition: background 0.1s, border-color 0.1s;
  }

  .checkbox-indicator.checked {
    background: hsl(var(--foreground));
    border-color: hsl(var(--foreground));
    color: hsl(var(--background));
  }

  .checkbox-content {
    flex: 1;
    min-width: 0;
  }

  .checkbox-content.checked :global(.node__content) {
    text-decoration: line-through;
    opacity: 0.6;
  }
</style>
