<!--
  QuoteBlockNode - Wraps BaseNode with quote block-specific functionality

  Responsibilities:
  - Displays quote lines without their > prefix (edit mode keeps it)
  - Provides quote styling with left border accent and subtle background
  - Handles multiline editing (Shift+Enter inserts a newline plus the `> ` prefix)
  - Supports inline markdown formatting (bold, italic, code, links)
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper QuoteBlockIcon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure
-->

<script lang="ts">
import { isA } from '$lib/types/core-node-types';
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';
  import { insertQuoteLineBreak } from '$lib/types/quote-block-node';

  // Props using Svelte 5 runes mode - same interface as BaseNode
  // Using $bindable() for content enables two-way binding with parent
  // This is the proper Svelte 5 pattern - no internal state copy needed
  let {
    nodeId,
    nodeType = 'quote-block',
    autoFocus = false,
    content = $bindable(''),
    children = []
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
  } = $props();

  const dispatch = createEventDispatcher();

  // Quote blocks use multiline editing (Shift+Enter inserts a newline and the "> " prefix)
  // Merging into quote-blocks is refused by the plugin's acceptsContentMerge: false
  const editableConfig = { allowMultiline: true };

  // Quote metadata - enable markdown processing and block-level elements
  let quoteMetadata = $derived({
    disableMarkdown: false, // Quote blocks support inline markdown
    enableBlockElements: true // Quote blocks render headings, lists, etc. as proper HTML elements
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
  let displayContent = $derived(extractQuoteForDisplay(content));

  /**
   * Handle content changes from BaseNode
   *
   * IMPORTANT: We pass through content as-is to allow TextareaController to detect
   * pattern changes for reversion (e.g., "> Hello" → ">Hello" should revert to text).
   *
   * This component never adds the "> " prefix to content. The prefix comes from
   * the "/quote" template, from typing "> ", and from the Enter seed in
   * handleCreateNewNode and from Shift+Enter (handleKeyDown).
   *
   * REFACTOR: Using $bindable() prop - update content directly via two-way binding
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const userContent = event.detail.content;

    // Pass through content as-is - don't transform!
    // This allows TextareaController to detect when the pattern no longer matches
    // (e.g., ">Hello" doesn't match /^>\s/) and trigger reversion to text type.
    //
    // The quote "> " prefix comes from:
    // 1. Initial conversion: User types "> " which triggers pattern detection
    // 2. The "/quote" slash command's content template
    // 3. Enter key: Creates new quote-block node with "> " content (via handleCreateNewNode)
    content = userContent;
    dispatch('contentChanged', { content: userContent });
  }

  /**
   * Handle Shift+Enter: insert a newline followed by the "> " prefix with the
   * caret after it, then notify the controller through an input event.
   * The browser default is cancelled so the bare newline is not inserted too.
   */
  function handleKeyDown(event: KeyboardEvent) {
    if (event.key !== 'Enter' || !event.shiftKey) return;
    if (event.isComposing || event.defaultPrevented) return;
    const textarea = event.target as HTMLTextAreaElement;
    if (!(textarea instanceof HTMLTextAreaElement)) return;
    event.preventDefault();
    const result = insertQuoteLineBreak(
      textarea.value,
      textarea.selectionStart,
      textarea.selectionEnd
    );
    textarea.value = result.value;
    textarea.setSelectionRange(result.caret, result.caret);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
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
    if (isA(detail.nodeType, 'quote-block')) {
      detail.currentContent = content; // Keep current node unchanged
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
    bind:content
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
    position: relative;
    /* width: 100% handled by parent .node-content-wrapper flex child rule */
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
