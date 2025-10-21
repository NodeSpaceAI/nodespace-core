<!--
  HeaderNode - Wraps BaseNode with header-specific functionality

  ARCHITECTURE NOTE (Issue #311 Refactor):
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
  let {
    nodeId,
    nodeType = 'header',
    autoFocus = false,
    content = '',
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

  // REFACTOR (Issue #316 Phase 2): Removed $effect for prop sync
  // Newline stripping now happens in event handler for clearer event flow
  let internalContent = $state(content);

  // Header level - derived from markdown syntax (#, ##, ###, etc.)
  // REFACTOR (Issue #316 Phase 1): Replaced $effect with $derived for pure reactive computation
  let headerLevel = $derived(parseHeaderLevel(internalContent));

  // Headers use default single-line editing
  const editableConfig = {};

  // Create reactive metadata object with header level
  let headerMetadata = $derived({ headerLevel });

  // Compute wrapper classes with header level
  const wrapperClasses = $derived(`header-node-wrapper header-h${headerLevel}`);

  // Compute display content for blur mode (strip hashtags)
  let displayContent = $derived(internalContent.replace(/^#{1,6}\s+/, ''));

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
   * REFACTOR (Issue #316 Phase 2): Moved newline stripping from $effect to event handler
   * This makes side effects explicit and event-driven instead of reactive
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    let newContent = event.detail.content;

    // Headers are single-line - strip newlines when they're entered
    // This happens when converting from multiline text nodes or pasting multiline content
    if (newContent.includes('\n')) {
      newContent = newContent.replace(/\n+/g, ' '); // Replace newlines with spaces
    }

    internalContent = newContent;
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
    bind:content={internalContent}
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
  /* Header wrapper must take full available width to prevent content cutoff */
  .header-node-wrapper {
    width: 100%;
    display: block;
  }

  /* Header-specific typography and icon positioning */
  .header-h1 {
    --font-size: 2rem;
    --line-height: 1.2;
    --icon-vertical-position: calc(0.25rem + (2rem * 1.2 / 2));
  }

  .header-h1 :global(.node__content) {
    font-size: 2rem;
    font-weight: bold;
    line-height: 1.2;
  }

  .header-h2 {
    --font-size: 1.5rem;
    --line-height: 1.3;
    --icon-vertical-position: calc(0.25rem + (1.5rem * 1.3 / 2));
  }

  .header-h2 :global(.node__content) {
    font-size: 1.5rem;
    font-weight: bold;
    line-height: 1.3;
  }

  .header-h3 {
    --font-size: 1.25rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.25rem * 1.4 / 2));
  }

  .header-h3 :global(.node__content) {
    font-size: 1.25rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .header-h4 {
    --font-size: 1.125rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.125rem * 1.4 / 2));
  }

  .header-h4 :global(.node__content) {
    font-size: 1.125rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .header-h5 {
    --font-size: 1rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1rem * 1.4 / 2));
  }

  .header-h5 :global(.node__content) {
    font-size: 1rem;
    font-weight: bold;
    line-height: 1.4;
  }

  .header-h6 {
    --font-size: 0.875rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (0.875rem * 1.4 / 2));
  }

  .header-h6 :global(.node__content) {
    font-size: 0.875rem;
    font-weight: bold;
    line-height: 1.4;
  }

  /* Ensure empty headers maintain proper height */
  .header-h1 :global(.node__content:empty),
  .header-h2 :global(.node__content:empty),
  .header-h3 :global(.node__content:empty),
  .header-h4 :global(.node__content:empty),
  .header-h5 :global(.node__content:empty),
  .header-h6 :global(.node__content:empty) {
    min-height: 1.5rem;
  }
</style>
