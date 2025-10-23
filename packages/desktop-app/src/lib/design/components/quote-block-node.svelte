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
    nodeType = 'quote-block',
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

  // Cursor positioning constants for Shift+Enter behavior
  const NEWLINE_LENGTH = 1; // \n character
  const QUOTE_PREFIX_LENGTH = 2; // "> " prefix

  // Internal reactive state - sync with content prop changes
  let internalContent = $state(content);

  // Track if we just added prefixes (for cursor adjustment)
  let pendingCursorAdjustment = $state<number | null>(null);

  // Sync internalContent when content prop changes externally
  $effect(() => {
    internalContent = content;
  });

  // Quote blocks use multiline editing (Shift+Enter for new lines with auto > prefix)
  // Prevent merging into quote-blocks (structured content can't accept arbitrary merges)
  const editableConfig = { allowMultiline: true, allowMergeInto: false };

  // Quote metadata - enable markdown processing
  let quoteMetadata = $derived({
    disableMarkdown: false // Quote blocks support inline markdown
  });

  /**
   * Extract quote content for display (strip > prefixes from each line)
   * Edit mode: > Hello world
   * Display mode: Hello world (> prefix hidden, but accent line visible)
   */
  function extractQuoteForDisplay(content: string): string {
    // Strip "> " or ">" from the beginning of each line
    // Preserve empty lines (trailing "> " becomes "" but newline preserved)
    const lines = content.split('\n');
    const strippedLines = lines.map((line) => line.replace(/^>\s?/, ''));

    // Join with newlines, preserving structure
    return strippedLines.join('\n');
  }

  // Display content: Strip > prefixes for blur mode
  let displayContent = $derived(extractQuoteForDisplay(internalContent));

  /**
   * Handle content changes from BaseNode
   * Add "> " prefix to all lines before saving (users only type it on first line)
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const userContent = event.detail.content;

    // Add "> " prefix to every line that doesn't have it
    const lines = userContent.split('\n');
    const prefixedLines = lines.map((line) => {
      const trimmed = line.trim();

      if (trimmed === '') {
        return '> '; // Empty line becomes "> " (with space for cursor)
      }
      if (trimmed.startsWith('> ')) {
        return line; // Already has "> " (with space)
      }
      if (trimmed.startsWith('>')) {
        // Has ">" but no space - add space
        return line.replace(/^>/, '> ');
      }
      // Add "> " prefix
      return `> ${line}`;
    });
    const prefixedContent = prefixedLines.join('\n');

    internalContent = prefixedContent;
    dispatch('contentChanged', { content: prefixedContent });

    // If we have a pending cursor adjustment (from Shift+Enter), apply it
    if (pendingCursorAdjustment !== null) {
      import('$lib/services/focus-manager.svelte').then(({ focusManager }) => {
        focusManager.focusNodeAtPosition(nodeId, pendingCursorAdjustment!);
        pendingCursorAdjustment = null; // Clear after use
      });
    }
  }

  /**
   * Handle Shift+Enter to position cursor after auto-added "> " prefix
   */
  function handleKeyDown(event: KeyboardEvent) {
    if (event.key === 'Enter' && event.shiftKey) {
      // Let the default happen (inserts \n), but track cursor position
      // After handleContentChange adds "> ", we'll reposition cursor
      const textarea = event.target as HTMLTextAreaElement;
      const cursorPos = textarea.selectionStart;

      // After the \n is inserted and "> " is added, cursor should be at:
      // current position + NEWLINE_LENGTH + QUOTE_PREFIX_LENGTH
      pendingCursorAdjustment = cursorPos + NEWLINE_LENGTH + QUOTE_PREFIX_LENGTH;
    }
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

    // For quote blocks: Enter creates new quote-block below with "> " prefix
    if (detail.nodeType === 'quote-block') {
      detail.currentContent = internalContent; // Keep current node unchanged
      detail.newContent = '> '; // New quote-block with "> " prefix ready
      detail.newNodeCursorPosition = 2; // Cursor after "> "
    }

    dispatch('createNewNode', detail);
  }

  /**
   * Handle node type conversion to quote-block
   */
  function handleNodeTypeChanged(event: CustomEvent) {
    const detail = event.detail;

    // Just dispatch the event as-is
    // Pattern detection already handles keeping the "> " prefix (cleanContent: false)
    // So we don't need to add it here - it's already in the content
    dispatch('nodeTypeChanged', detail);
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!--
  Wrapper div for quote-block-specific styling and keyboard event delegation.
  role="presentation": This div is purely for styling and event forwarding.
  All semantic meaning and interaction is handled by the BaseNode component below.
  Screen readers should skip this wrapper and focus on the content within BaseNode.
-->
<div class="quote-block-node-wrapper" onkeydown={handleKeyDown} role="presentation">
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    content={internalContent}
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
    border-left: 4px solid hsl(var(--primary)); /* Straight vertical line */
    position: relative;
    line-height: 1.6;
    white-space: pre-wrap;
  }
</style>
