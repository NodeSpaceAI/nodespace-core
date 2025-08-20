<!--
  TextNode Component - Enhanced with ContentProcessor for dual-representation
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/BaseNode.svelte';
  import { contentProcessor } from '$lib/services/contentProcessor.js';

  // Props
  export let nodeId: string;
  export let autoFocus: boolean = false;
  export let content: string = '';
  export let inheritHeaderLevel: number = 0; // Header level inherited from parent node
  export let children: unknown[] = []; // Passthrough for BaseNode

  // Internal reactive state
  let internalContent: string = content;
  let headerLevel: number = contentProcessor.parseHeaderLevel(content) || inheritHeaderLevel;
  let baseNodeRef: unknown;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    createNewNode: {
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      inheritHeaderLevel?: number;
    };
    contentChanged: { content: string };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: {
      nodeId: string;
      direction: 'up' | 'down';
      columnHint: number;
    };
    combineWithPrevious: {
      nodeId: string;
      currentContent: string;
    };
    deleteNode: {
      nodeId: string;
    };
    focus: void;
    blur: void;
  }>();

  // Handle focus/blur for TextNode-specific behavior
  function handleFocus() {
    dispatch('focus');
  }

  function handleBlur() {
    dispatch('blur');
  }

  // Handle content changes with ContentProcessor
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const newContent = event.detail.content;
    internalContent = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  // Handle header level changes from controller
  function handleHeaderLevelChange(event: CustomEvent<{ level: number }>) {
    const newLevel = event.detail.level !== undefined ? event.detail.level : inheritHeaderLevel;
    headerLevel = newLevel;
  }
</script>

<BaseNode
  bind:this={baseNodeRef}
  {nodeId}
  nodeType="text"
  {autoFocus}
  content={internalContent}
  {headerLevel}
  {children}
  on:createNewNode={(e) =>
    dispatch('createNewNode', { ...e.detail, inheritHeaderLevel: headerLevel })}
  on:contentChanged={handleContentChange}
  on:headerLevelChanged={handleHeaderLevelChange}
  on:indentNode={(e) => dispatch('indentNode', e.detail)}
  on:outdentNode={(e) => dispatch('outdentNode', e.detail)}
  on:navigateArrow={(e) => dispatch('navigateArrow', e.detail)}
  on:combineWithPrevious={(e) => dispatch('combineWithPrevious', e.detail)}
  on:deleteNode={(e) => dispatch('deleteNode', e.detail)}
  on:focus={handleFocus}
  on:blur={handleBlur}
/>
