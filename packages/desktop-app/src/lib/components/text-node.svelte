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
  import { contentProcessor } from '$lib/services/content-processor.js';
  import type { NodeViewerProps } from '../types/node-viewers';

  // Props - implements NodeViewerProps interface
  let {
    nodeId,
    autoFocus = false,
    content = '',
    nodeType = 'text',
    inheritHeaderLevel = 0,
    children = []
  }: NodeViewerProps = $props();

  // Internal reactive state
  let internalContent = $state(content);
  let headerLevel = $state(contentProcessor.parseHeaderLevel(content) || inheritHeaderLevel);

  // Sync internalContent when content prop changes externally
  $effect(() => {
    internalContent = content;
  });

  // Text nodes always allow multiline editing
  const editableConfig = {
    allowMultiline: true
  };

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    createNewNode: {
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
    };
    contentChanged: { content: string };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: {
      nodeId: string;
      direction: 'up' | 'down';
      pixelOffset: number;
    };
    combineWithPrevious: {
      nodeId: string;
      currentContent: string;
    };
    slashCommandSelected: {
      command: string;
      nodeType: string;
      cursorPosition?: number;
    };
    deleteNode: { nodeId: string };
    nodeTypeChanged: { nodeType: string; cleanedContent?: string };
    focus: void;
    blur: void;
  }>();

  function handleFocus() {
    dispatch('focus');
  }

  function handleBlur() {
    dispatch('blur');
  }

  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const newContent = event.detail.content;
    internalContent = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  function handleHeaderLevelChange(event: CustomEvent<{ level: number }>) {
    const newLevel = event.detail.level !== undefined ? event.detail.level : inheritHeaderLevel;
    headerLevel = newLevel;
  }
</script>

<BaseNode
  {nodeId}
  {nodeType}
  {autoFocus}
  content={internalContent}
  {children}
  {editableConfig}
  on:createNewNode={(e) =>
    dispatch('createNewNode', { ...e.detail, inheritHeaderLevel: headerLevel })}
  on:contentChanged={handleContentChange}
  on:headerLevelChanged={handleHeaderLevelChange}
  on:indentNode={(e) => dispatch('indentNode', e.detail)}
  on:outdentNode={(e) => dispatch('outdentNode', e.detail)}
  on:navigateArrow={(e) => dispatch('navigateArrow', e.detail)}
  on:combineWithPrevious={(e) => dispatch('combineWithPrevious', e.detail)}
  on:slashCommandSelected={(e) => dispatch('slashCommandSelected', e.detail)}
  on:nodeTypeChanged={(e) => dispatch('nodeTypeChanged', e.detail)}
  on:deleteNode={(e) => dispatch('deleteNode', e.detail)}
  on:focus={handleFocus}
  on:blur={handleBlur}
/>
