<!--
  HeaderNode - Wraps BaseNode with header-specific functionality

  Responsibilities:
  - Manages header level (1-6) derived from markdown syntax
  - Provides header-specific styling based on level
  - Handles header level detection and updates
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

  // Internal reactive state - sync with content prop changes
  let internalContent = $state(content);

  // Sync internalContent when content prop changes externally
  // Apply header-specific content transformations (strip newlines for single-line headers)
  $effect(() => {
    let cleanedContent = content;

    // Headers are single-line - strip newlines when converting from multiline text nodes
    if (cleanedContent.includes('\n')) {
      cleanedContent = cleanedContent.replace(/\n+/g, ' '); // Replace newlines with spaces
      // Update parent with cleaned content
      dispatch('contentChanged', { content: cleanedContent });
    }

    internalContent = cleanedContent;
  });

  // Header level - derived from markdown syntax (#, ##, ###, etc.)
  let headerLevel = $state(parseHeaderLevel(content));

  // Update header level when content changes
  $effect(() => {
    const newLevel = parseHeaderLevel(internalContent);
    if (newLevel !== headerLevel) {
      headerLevel = newLevel;
    }
  });

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
   */
  function handleContentChange(event: CustomEvent<{ content: string }>) {
    const newContent = event.detail.content;
    internalContent = newContent;
    dispatch('contentChanged', { content: newContent });
  }

  /**
   * Handle header level changes from controller
   */
  function handleHeaderLevelChange(event: CustomEvent<{ level: number }>) {
    headerLevel = event.detail.level;
    dispatch('headerLevelChanged', { level: event.detail.level });
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
    on:headerLevelChanged={handleHeaderLevelChange}
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
