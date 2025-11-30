<!--
  OrderedListNode - Wraps BaseNode with ordered list-specific functionality

  Responsibilities:
  - Manages "1. " prefix for ordered list items
  - Provides auto-numbering (sequential 1, 2, 3...) in display mode
  - Multiline editing with Shift+Enter to add new list items
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper OrderedListIcon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure

  Design System Reference: docs/design-system/components.html → Ordered List Nodes
-->

<script lang="ts">
  import { createEventDispatcher, getContext } from 'svelte';
  import BaseNode from './base-node.svelte';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation';

  // Get paneId from context (set by PaneContent)
  const paneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  // Props using Svelte 5 runes mode - same interface as BaseNode
  let {
    nodeId,
    nodeType = 'ordered-list',
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
  const LIST_PREFIX_LENGTH = 3; // "1. " prefix length

  // Internal reactive state - initialized from content prop
  // Note: Content updates flow through handleContentChange event, not prop sync
  // External changes are rare (only from parent re-renders) and handled by component remount
  let internalContent = $state(content);

  // Track if we just added prefixes (for cursor adjustment)
  let pendingCursorAdjustment = $state<number | null>(null);

  // Ordered lists use multiline editing (Shift+Enter for new lines with auto "1. " prefix)
  // Prevent merging into ordered-lists (structured content can't accept arbitrary merges)
  const editableConfig = { allowMultiline: true, allowMergeInto: false };

  // Ordered list metadata - enable markdown for inline formatting (bold, italic, code)
  // Note: List structure (1. 2. 3.) is preserved as plain text by marked-config.ts
  let listMetadata = $derived({
    disableMarkdown: false // Allow inline formatting like **bold**, *italic*, `code`
  });

  /**
   * Extract list content for display and add auto-numbering
   * Edit mode: 1. First item\n1. Second item\n1. Third item
   * Display mode: 1. First item\n2. Second item\n3. Third item
   */
  function extractListForDisplay(content: string): string {
    // Strip "1. " or "1." from the beginning of each line
    // Then add sequential numbering (1, 2, 3...)
    const lines = content.split('\n');
    const numberedLines = lines.map((line, index) => {
      const strippedLine = line.replace(/^1\.\s?/, '');
      return `${index + 1}. ${strippedLine}`;
    });

    // Join with newlines, preserving structure
    return numberedLines.join('\n');
  }

  // Display content: Add sequential numbering for view mode
  let displayContent = $derived(extractListForDisplay(internalContent));

  /**
   * Handle content changes from BaseNode
   * Add "1. " prefix to all lines before saving (users only type it on first line)
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const userContent = event.detail.content;

    // Add "1. " prefix to every line that doesn't have it
    const lines = userContent.split('\n');
    const prefixedLines = lines.map((line) => {
      const trimmed = line.trim();

      // Empty line - preserve placeholder with proper spacing
      if (trimmed === '') {
        return '1. ';
      }

      // Already properly formatted with "1. " prefix
      if (trimmed.startsWith('1. ')) {
        return line;
      }

      // Missing space after period: "1." → "1. "
      if (trimmed.startsWith('1.')) {
        return line.replace(/^1\./, '1. ');
      }

      // Malformed prefix (missing dot and/or space): "1Item", "1 Item", "1" → "1. Item"
      // This handles edge cases from backspace operations where user deletes characters
      // between "1" and the content, creating intermediate states like "1Item"
      if (trimmed.startsWith('1')) {
        return line.replace(/^1\.?\s?/, '1. ');
      }

      // Missing prefix entirely - add "1. " before content
      return `1. ${line}`;
    });
    const prefixedContent = prefixedLines.join('\n');

    internalContent = prefixedContent;
    dispatch('contentChanged', { content: prefixedContent });

    // If we have a pending cursor adjustment (from Shift+Enter), apply it
    if (pendingCursorAdjustment !== null) {
      import('$lib/services/focus-manager.svelte').then(({ focusManager }) => {
        focusManager.focusNodeAtPosition(nodeId, pendingCursorAdjustment!, paneId);
        pendingCursorAdjustment = null; // Clear after use
      });
    }
  }

  /**
   * Handle Shift+Enter to position cursor after auto-added "1. " prefix
   */
  function handleKeyDown(event: KeyboardEvent) {
    if (event.key === 'Enter' && event.shiftKey) {
      // Let the default happen (inserts \n), but track cursor position
      // After handleContentChange adds "1. ", we'll reposition cursor
      const textarea = event.target as HTMLTextAreaElement;
      const cursorPos = textarea.selectionStart;

      // After the \n is inserted and "1. " is added, cursor should be at:
      // current position + NEWLINE_LENGTH + LIST_PREFIX_LENGTH
      pendingCursorAdjustment = cursorPos + NEWLINE_LENGTH + LIST_PREFIX_LENGTH;
    }
  }

  /**
   * Handle createNewNode event - check if we should convert back to text
   */
  function handleCreateNewNode(event: CustomEvent) {
    const detail = event.detail;

    // If the content no longer starts with "1.", convert to text node
    if (detail.currentContent && !detail.currentContent.trim().startsWith('1.')) {
      // Remove all "1. " prefixes from all lines
      const cleanedContent = detail.currentContent
        .split('\n')
        .map((line: string) => line.replace(/^1\.\s?/, ''))
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

    dispatch('createNewNode', detail);
  }

  /**
   * Handle node type conversion to ordered-list
   */
  function handleNodeTypeChanged(event: CustomEvent) {
    // Just dispatch the event as-is
    // Pattern detection already handles keeping the "1. " prefix (cleanContent: false)
    // So we don't need to add it here - it's already in the content
    dispatch('nodeTypeChanged', event.detail);
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!--
  Wrapper div for ordered-list-specific styling and keyboard event delegation.
  role="presentation": This div is purely for styling and event forwarding.
  All semantic meaning and interaction is handled by the BaseNode component below.
  Screen readers should skip this wrapper and focus on the content within BaseNode.
-->
<div
  class="ordered-list-node-wrapper"
  data-node-type="ordered-list"
  onkeydown={handleKeyDown}
  role="presentation"
>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    content={internalContent}
    {displayContent}
    {children}
    {editableConfig}
    metadata={listMetadata}
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
  /* Ordered list wrapper */
  .ordered-list-node-wrapper {
    position: relative;
    /* width: 100% handled by parent .node-content-wrapper flex child rule */
  }

  /* Apply ordered list styling to the content area only (BaseNode wraps it) */
  .ordered-list-node-wrapper :global(.node__content) {
    display: block;
    position: relative;
    white-space: pre-wrap;
    font-variant-numeric: tabular-nums; /* Use monospaced numbers for consistent alignment */
  }
</style>
