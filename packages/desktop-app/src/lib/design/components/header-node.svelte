<!--
  HeaderNode - Wraps BaseNode with header-specific functionality

  ARCHITECTURE NOTE:
  - Header level detection moved FROM TextareaController TO HeaderNode $effect
  - CSS styling moved FROM BaseNode TO HeaderNode wrapper classes (.header-h1 through .header-h6)
  - This ensures proper separation of concerns: BaseNode is node-type agnostic

  Why This Architecture?
  - Single Responsibility Principle: BaseNode handles core editing, HeaderNode handles header specifics
  - TextareaController only detects pattern → header conversion, NOT level changes within headers
  - HeaderNode's $effect watches content changes and updates header level reactively
  - CSS variables set by HeaderNode wrapper are inherited by nested BaseNode for icon positioning

  Responsibilities:
  - Manages header level (1-6) derived from markdown syntax via $effect
  - Provides header-specific styling based on level through wrapper classes
  - Handles header level detection and updates independently
  - Emits headerLevelChanged events for parent components that need them
  - Forwards all other events to BaseNode

  Integration:
  - Uses icon registry for proper header icon rendering
  - Maintains compatibility with BaseNode API
  - Works seamlessly in node tree structure
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import BaseNode from './base-node.svelte';

  // Props using Svelte 5 runes mode - same interface as BaseNode
  // Using $bindable() for content enables two-way binding with parent
  // This is the proper Svelte 5 pattern - no internal state copy needed
  let {
    nodeId,
    nodeType = 'header',
    autoFocus = false,
    content = $bindable(''),
    children = []
    // metadata = {} // Not yet used but reserved for future header-specific metadata
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
    // metadata?: Record<string, unknown>;
  } = $props();

  const dispatch = createEventDispatcher();

  // Header level - derived from markdown syntax (#, ##, ###, etc.)
  // Replaced $effect with $derived for pure reactive computation
  let headerLevel = $derived(parseHeaderLevel(content));

  // Headers use default single-line editing
  const editableConfig = {};

  // Create reactive metadata object with header level
  let headerMetadata = $derived({ headerLevel });

  // Compute wrapper classes with header level
  const wrapperClasses = $derived(`header-node-wrapper header-h${headerLevel}`);

  // Compute display content for blur mode (strip hashtags)
  let displayContent = $derived(content.replace(/^#{1,6}\s+/, ''));

  /**
   * Parse header level from markdown syntax
   * Returns 1-6 for valid headers, counting hashtags even without space
   */
  function parseHeaderLevel(content: string): number {
    const trimmed = content.trim();
    // First try to match with space (complete pattern)
    const matchWithSpace = trimmed.match(/^(#{1,6})\s/);
    if (matchWithSpace) {
      return matchWithSpace[1].length;
    }

    // Fallback: count hashtags at start (for incomplete pattern like "###")
    const matchHashtags = trimmed.match(/^(#{1,6})/);
    if (matchHashtags) {
      return matchHashtags[1].length;
    }

    // Default to h1 if no hashtags found
    return 1;
  }

  /**
   * Handle content changes and sync with parent
   * Moved newline stripping from $effect to event handler
   * This makes side effects explicit and event-driven instead of reactive
   * REFACTOR: Using $bindable() prop - update content directly via two-way binding
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    let newContent = event.detail.content;

    // Headers are single-line - strip newlines when they're entered
    // This happens when converting from multiline text nodes or pasting multiline content
    if (newContent.includes('\n')) {
      newContent = newContent.replace(/\n+/g, ' '); // Replace newlines with spaces
    }

    // Update via $bindable() prop - this updates the parent's state directly
    content = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Forward all other events to parent components
   */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<!-- Wrap BaseNode with header-specific styling -->
<div class={wrapperClasses}>
  <BaseNode
    {nodeId}
    {nodeType}
    {autoFocus}
    bind:content
    {displayContent}
    {children}
    {editableConfig}
    metadata={headerMetadata}
    on:createNewNode={forwardEvent('createNewNode')}
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
    on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
    on:iconClick={forwardEvent('iconClick')}
  />
</div>

<style>
  /* Header wrapper - width handled by parent .node-content-wrapper flex rule */
  /* No additional styles needed - flex: 1 applied by parent */

  /* Header-specific typography and icon positioning, from the heading type-scale
     tokens in app.css. H4–H6 share heading-4. */
  .header-h1 {
    --font-size: var(--heading-1-size);
    --line-height: var(--heading-1-lh);
    --icon-vertical-position: calc(0.25rem + (var(--font-size) * var(--line-height) / 2));
  }

  .header-h1 :global(.node__content) {
    font-size: var(--heading-1-size);
    font-weight: 600;
    line-height: var(--heading-1-lh);
  }

  .header-h2 {
    --font-size: var(--heading-2-size);
    --line-height: var(--heading-2-lh);
    --icon-vertical-position: calc(0.25rem + (var(--font-size) * var(--line-height) / 2));
  }

  .header-h2 :global(.node__content) {
    font-size: var(--heading-2-size);
    font-weight: 600;
    line-height: var(--heading-2-lh);
  }

  .header-h3 {
    --font-size: var(--heading-3-size);
    --line-height: var(--heading-3-lh);
    --icon-vertical-position: calc(0.25rem + (var(--font-size) * var(--line-height) / 2));
  }

  .header-h3 :global(.node__content) {
    font-size: var(--heading-3-size);
    font-weight: 600;
    line-height: var(--heading-3-lh);
  }

  .header-h4,
  .header-h5,
  .header-h6 {
    --font-size: var(--heading-4-size);
    --line-height: var(--heading-4-lh);
    --icon-vertical-position: calc(0.25rem + (var(--font-size) * var(--line-height) / 2));
  }

  .header-h4 :global(.node__content),
  .header-h5 :global(.node__content),
  .header-h6 :global(.node__content) {
    font-size: var(--heading-4-size);
    font-weight: 600;
    line-height: var(--heading-4-lh);
  }

  /* Ensure empty headers hold their level's full line height. `1lh` resolves against
     .node__content's own line-height (set per level above); --font-size/--line-height
     would not, because base-node's .node resets them to body values in between.
     The editing textarea always matches :empty (its value is a property, not child
     text), so this is also every heading's edit-mode floor — keep the selector as is. */
  .header-h1 :global(.node__content:empty),
  .header-h2 :global(.node__content:empty),
  .header-h3 :global(.node__content:empty),
  .header-h4 :global(.node__content:empty),
  .header-h5 :global(.node__content:empty),
  .header-h6 :global(.node__content:empty) {
    min-height: 1lh;
  }
</style>
