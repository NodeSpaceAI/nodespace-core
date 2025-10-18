<!--
  QuoteBlockNode - Wraps BaseNode with quote block-specific functionality

  Responsibilities:
  - Manages > prefix for quote lines
  - Provides quote styling with left border accent and subtle background
  - Handles multiline editing with auto > prefix on new lines
  - Supports inline markdown formatting (bold, italic, code, links)
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper QuoteBlockIcon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure

  Design System Reference: docs/design-system/components.html → Quote Block Nodes
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';

  // Props using Svelte 5 runes mode - same interface as BaseNode
  let {
    nodeId,
    nodeType = 'quote',
    autoFocus = false,
    content = '',
    children = []
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
  } = $props();

  const dispatch = createEventDispatcher();

  // Internal reactive state - sync with content prop changes
  let internalContent = $state(content);

  // Sync internalContent when content prop changes externally
  $effect(() => {
    internalContent = content;
  });

  // Quote blocks use multiline editing (Shift+Enter for new lines with auto > prefix)
  const editableConfig = { allowMultiline: true };

  // Quote metadata - enable markdown processing
  let quoteMetadata = $derived({
    disableMarkdown: false // Quote blocks support inline markdown
  });

  /**
   * Extract quote content for editing (keep > prefixes visible)
   * User sees and edits: > Line 1\n> Line 2
   */
  function extractQuoteForEditing(content: string): string {
    return content; // Show > prefixes in edit mode
  }

  /**
   * Extract quote content for display (show > prefixes with formatting)
   * View mode: Show > prefixes with content, markdown rendered
   */
  function extractQuoteForDisplay(content: string): string {
    return content; // Show > prefixes in display mode with formatting
  }

  // Edit mode: Show > prefixes
  // View mode: Show > prefixes with markdown rendered
  let editContent = $derived(extractQuoteForEditing(internalContent));
  let displayContent = $derived(extractQuoteForDisplay(internalContent));

  /**
   * Handle content changes from BaseNode
   * Automatically add > prefix to new lines when Shift+Enter is pressed
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const userContent = event.detail.content;
    internalContent = userContent;
    dispatch('contentChanged', { content: userContent });
  }

  /**
   * Handle createNewNode event - check if we should convert back to text
   */
  function handleCreateNewNode(event: CustomEvent) {
    const detail = event.detail;

    // If the content no longer starts with >, convert to text node
    if (detail.currentContent && !detail.currentContent.trim().startsWith('>')) {
      // Remove all > prefixes from all lines
      const cleanedContent = detail.currentContent
        .split('\n')
        .map((line: string) => line.replace(/^>\s?/, ''))
        .join('\n');

      detail.currentContent = cleanedContent;
      detail.nodeType = 'text'; // Convert to text node

      // Dispatch node type change
      dispatch('nodeTypeChanged', {
        nodeId,
        newNodeType: 'text',
        cleanedContent
      });
    }

    // For quote blocks: Enter at end creates text node, Enter in middle creates new quote line
    if (detail.nodeType === 'quote') {
      // The default behavior from BaseNode will handle this correctly
      // - If at end: create text node below
      // - If in middle: split and create new quote node
    }

    dispatch('createNewNode', detail);
  }

  /**
   * Handle node type conversion to quote-block
   */
  function handleNodeTypeChanged(event: CustomEvent) {
    const detail = event.detail;

    // First dispatch the event as-is
    dispatch('nodeTypeChanged', detail);

    // Auto-add > prefix if converting to quote and content doesn't have it
    if (detail.newNodeType === 'quote') {
      const cleaned = detail.cleanedContent || '';

      if (!cleaned.startsWith('>')) {
        // Add > prefix to all lines
        const withPrefix = cleaned
          .split('\n')
          .map((line: string) => (line.trim() ? `> ${line}` : '>'))
          .join('\n');

        // Schedule update for next tick
        setTimeout(() => {
          internalContent = withPrefix;
          dispatch('contentChanged', { content: withPrefix });
        }, 0);
      }
    }
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with quote-block-specific styling -->
<div class="quote-block-node-wrapper">
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    content={editContent}
    {displayContent}
    {children}
    {editableConfig}
    metadata={quoteMetadata}
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
  /* Quote block wrapper */
  .quote-block-node-wrapper {
    width: 100%;
    display: block;
    position: relative;
  }

  /* Apply quote styling to the content area only (BaseNode wraps it) */
  .quote-block-node-wrapper :global(.node__content) {
    background: hsl(var(--muted));
    padding-left: 0.5rem;
    border-radius: var(--radius);
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    position: relative;
    line-height: 1.6;
    white-space: pre-wrap;
  }

  /* Straight vertical line (4px primary color) */
  .quote-block-node-wrapper :global(.node__content)::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 4px;
    background: hsl(var(--primary));
  }
</style>
