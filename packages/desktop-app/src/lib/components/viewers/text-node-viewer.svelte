<!--
  TextNodeViewer - Smart wrapper around BaseNode with header-aware multiline logic
  
  This component implements the core text editing functionality with intelligent
  multiline behavior based on header level:
  - Headers (h1-h6): Single-line only for semantic integrity
  - Regular text: Multi-line with Shift+Enter support
  
  This is now the canonical TextNode implementation, wrapping BaseNode internally
  while maintaining all existing functionality including keyboard shortcuts,
  markdown formatting, and navigation.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import { contentProcessor } from '$lib/services/contentProcessor.js';
  import type { NodeViewerProps } from '$lib/types/nodeViewers.js';

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
  // (e.g., from node combination operations)
  $effect(() => {
    internalContent = content;
  });

  // Smart multiline logic - headers are single-line only, regular text allows multiline
  const editableConfig = $derived({
    allowMultiline: headerLevel === 0 // Only allow multiline for regular text (headerLevel 0)
  });

  // Event dispatcher - explicitly typed with all events this component can dispatch
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

  // Handle focus/blur for TextNodeViewer-specific behavior
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
  {nodeId}
  {nodeType}
  {autoFocus}
  content={internalContent}
  {headerLevel}
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
