<!--
  NodeRow - renders a single node row for BaseNodeViewer.

  Renders the chevron (for parent nodes) plus either the node's lazy-loaded plugin
  component or a plain BaseNode fallback (used while the plugin component loads, and
  for entity types that have no dedicated node component registered).

  The underlying node components dispatch legacy `on:` events; NodeRow consumes those
  here and re-exposes them to the viewer as callback props (see node-row-types.ts).
  This keeps the ~11 event handlers defined once in the viewer instead of duplicated
  across the plugin and fallback branches.
-->

<script lang="ts">
  import BaseNode from '$lib/design/components/base-node.svelte';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { extractNodeMetadata } from '$lib/design/components/schema-field-update';
  import { rendersAsEntityRow } from '$lib/design/components/node-type-predicates';
  import { HAS_RESOLVED_CHARACTER_RE } from '$lib/utils/node-display-title';
  import {
    extractFallbackDisplayContent,
    extractFallbackMetadata
  } from '$lib/design/components/fallback-node-render';
  import type {
    ViewerRenderNode,
    NodeRowCallbacks,
    ContentChangedDetail,
    NodeTypeChangedDetail,
    SlashCommandSelectedDetail,
    CreateNewNodeDetail,
    NavigateArrowDetail,
    IconClickDetail,
    TaskStateChangedDetail,
    CombineWithPreviousDetail,
    DeleteNodeDetail
  } from '$lib/design/components/node-row-types';

  let {
    node,
    loadedNodeComponent,
    paneId,
    onCreateNewNode,
    onIndentNode,
    onOutdentNode,
    onNavigateArrow,
    onContentChanged,
    onNodeTypeChanged,
    onSlashCommandSelected,
    onIconClick,
    onTaskStateChanged,
    onCombineWithPrevious,
    onDeleteNode,
    onToggleExpanded
  }: {
    node: ViewerRenderNode;
    loadedNodeComponent: unknown;
    paneId: string;
  } & NodeRowCallbacks = $props();

  /** Open a custom schema entity node in the other pane (or a new tab with modifier). */
  async function openEntity(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    const { getNavigationService } = await import('$lib/services/navigation-service');
    if (event.metaKey || event.ctrlKey) {
      getNavigationService().navigateToNode(node.id, true, paneId);
    } else {
      getNavigationService().navigateToNodeInOtherPane(node.id, paneId);
    }
  }
</script>

<div class="node-content-wrapper">
  <!-- Chevron for parent nodes using design system approach -->
  {#if node.children && node.children.length > 0}
    <button
      class="chevron-icon"
      class:expanded={node.expanded}
      onclick={() => onToggleExpanded(node.id)}
      onkeydown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onToggleExpanded(node.id);
        }
      }}
      aria-label={node.expanded ? 'Collapse node' : 'Expand node'}
      aria-expanded={node.expanded}
    >
      <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
        <path d="M6 3l5 5-5 5-1-1 4-4-4-4 1-1z" />
      </svg>
    </button>
  {/if}

  <!-- Node viewer with stable component references - all nodes use plugin registry -->
  {#if loadedNodeComponent}
    {#key `${node.id}-${node.nodeType}`}
      {@const NodeComponent = loadedNodeComponent as typeof BaseNode}
      {@const nodeMetadata = extractNodeMetadata(node)}
      <NodeComponent
        nodeId={node.id}
        nodeType={node.nodeType}
        autoFocus={node.autoFocus}
        content={node.content}
        children={node.children}
        metadata={nodeMetadata}
        editableConfig={{ allowMultiline: true }}
        on:createNewNode={(e: CustomEvent<CreateNewNodeDetail>) => onCreateNewNode(e.detail)}
        on:indentNode={(e: CustomEvent<{ nodeId: string }>) => onIndentNode(e.detail)}
        on:outdentNode={(e: CustomEvent<{ nodeId: string }>) => onOutdentNode(e.detail)}
        on:navigateArrow={(e: CustomEvent<NavigateArrowDetail>) => onNavigateArrow(e.detail)}
        on:contentChanged={(e: CustomEvent<ContentChangedDetail>) => onContentChanged(node, e.detail)}
        on:nodeTypeChanged={(e: CustomEvent<NodeTypeChangedDetail>) => onNodeTypeChanged(node, e.detail)}
        on:slashCommandSelected={(e: CustomEvent<SlashCommandSelectedDetail>) =>
          onSlashCommandSelected(node, e.detail)}
        on:iconClick={(e: CustomEvent<IconClickDetail>) => onIconClick(e.detail)}
        on:taskStateChanged={(e: CustomEvent<TaskStateChangedDetail>) => onTaskStateChanged(node, e.detail)}
        on:combineWithPrevious={(e: CustomEvent<CombineWithPreviousDetail>) =>
          onCombineWithPrevious(e.detail)}
        on:deleteNode={(e: CustomEvent<DeleteNodeDetail>) => onDeleteNode(e.detail)}
      />
    {/key}
  {:else}
    <!-- Final fallback to BaseNode with key for re-rendering -->
    <!-- Fallback applies syntax stripping for known types (code-block, header, quote-block) -->
    <!-- Custom schema entities also render here (no lazy-loaded node component) -->
    <!-- Note: on:taskStateChanged below is inert on plain BaseNode (only task-node dispatches
         it); it fires only via the plugin-component branch above. Kept for branch symmetry. -->
    {@const nodeHasTitleTemplate = pluginRegistry.hasTitleTemplate(node.nodeType)}
    {@const nodeTitleDisplay = nodeHasTitleTemplate
      ? node.title && HAS_RESOLVED_CHARACTER_RE.test(node.title)
        ? node.title
        : (pluginRegistry.getTitleTemplate(node.nodeType) ?? '')
      : undefined}
    {#key `${node.id}-${node.nodeType}`}
      <BaseNode
        nodeId={node.id}
        nodeType={node.nodeType}
        autoFocus={node.autoFocus}
        content={node.content}
        readonly={nodeHasTitleTemplate}
        displayContentIsPlaceholder={nodeHasTitleTemplate && !(node.title && HAS_RESOLVED_CHARACTER_RE.test(node.title))}
        displayContent={nodeTitleDisplay !== undefined
          ? nodeTitleDisplay
          : extractFallbackDisplayContent(node.content, node.nodeType)}
        children={node.children}
        metadata={extractFallbackMetadata(node.nodeType, node.properties)}
        editableConfig={{ allowMultiline: true }}
        on:createNewNode={(e: CustomEvent<CreateNewNodeDetail>) => onCreateNewNode(e.detail)}
        on:indentNode={(e: CustomEvent<{ nodeId: string }>) => onIndentNode(e.detail)}
        on:outdentNode={(e: CustomEvent<{ nodeId: string }>) => onOutdentNode(e.detail)}
        on:navigateArrow={(e: CustomEvent<NavigateArrowDetail>) => onNavigateArrow(e.detail)}
        on:contentChanged={(e: CustomEvent<ContentChangedDetail>) => onContentChanged(node, e.detail)}
        on:nodeTypeChanged={(e: CustomEvent<NodeTypeChangedDetail>) => onNodeTypeChanged(node, e.detail)}
        on:slashCommandSelected={(e: CustomEvent<SlashCommandSelectedDetail>) =>
          onSlashCommandSelected(node, e.detail)}
        on:iconClick={(e: CustomEvent<IconClickDetail>) => onIconClick(e.detail)}
        on:taskStateChanged={(e: CustomEvent<TaskStateChangedDetail>) => onTaskStateChanged(node, e.detail)}
        on:combineWithPrevious={(e: CustomEvent<CombineWithPreviousDetail>) =>
          onCombineWithPrevious(e.detail)}
        on:deleteNode={(e: CustomEvent<DeleteNodeDetail>) => onDeleteNode(e.detail)}
      />
    {/key}
    {#if rendersAsEntityRow(node.nodeType)}
      <button
        class="custom-entity-open-button"
        onclick={openEntity}
        type="button"
        aria-label="Open entity in dedicated viewer pane (Cmd+Click for new tab in same pane)"
        title="Open in viewer"
      >
        open
      </button>
    {/if}
  {/if}
</div>

<style>
  .node-content-wrapper {
    /* Wrapper for chevron + content */
    display: flex;
    align-items: flex-start;
    gap: 0.25rem; /* 4px gap between chevron/spacer and text content */
    position: relative; /* Enable absolute positioning for chevrons */
    width: 100%; /* Ensure wrapper fills parent so flex children can inherit */

    /* CSS-first positioning to match base-node.svelte implementation */
    /* Default values for normal text - adjusted for better circle alignment */
    --line-height: 1.875;
    --font-size: 1rem;
  }

  /* Open button for custom entity nodes (appears on hover like task open button) */
  .custom-entity-open-button {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    color: hsl(var(--foreground));
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    font-size: 0.75rem;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.2s ease;
    text-transform: lowercase;
    z-index: 5;
  }

  .node-content-wrapper:hover .custom-entity-open-button {
    opacity: 1;
  }

  .custom-entity-open-button:hover {
    background: hsl(var(--muted));
  }

  /* Flex children (node wrappers) should fill available space */
  /* Use :global() to apply across component boundaries (TaskNode, CodeBlock, etc. have different scopes) */
  .node-content-wrapper > :global(:not(.chevron-icon):not(.chevron-spacer)) {
    flex: 1;
    min-width: 0; /* Allow flex item to shrink below content size if needed */
  }

  /*
    Chevron positioning system - matches circle positioning exactly

    POSITIONING FORMULA:
    The chevron must be vertically centered with the circles, which use:
    top: calc(0.25rem + (var(--font-size) * var(--line-height) / 2))

    Where:
    - 0.25rem = container top padding (.node has padding: 0.25rem)
    - line-height-px = font-size × line-height multiplier
    - This formula positions at the visual center of the first line of text

    HORIZONTAL POSITION:
    - Exactly halfway between parent and child circles
    - Parent is at current depth, child is at depth + 2.5rem (--node-spacing-indent)
    - Chevron positioned at -1.25rem (half of 2.5rem) from current node

    INHERITANCE:
    The --line-height-px variable is inherited from .node-content-wrapper
    which detects the header level of nested content using :has() selector
  */
  .chevron-icon {
    opacity: 0; /* Hidden by default - shows on hover */
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    transition: opacity 0.15s ease-in-out; /* Smooth fade in/out */
    pointer-events: auto; /* Ensure chevron always receives pointer events */
    flex-shrink: 0;
    width: 1.25rem; /* Fixed 20px to match circle size */
    height: 1.25rem; /* Fixed 20px to match circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* Position chevron exactly halfway between parent and child circles */
    position: absolute;
    left: calc(
      -1 * var(--node-spacing-indent) / 2 + var(--circle-offset)
    ); /* Halfway back to parent + parent circle offset */
    /* Use shared CSS variable from .node - single source of truth for vertical positioning */
    top: var(--icon-vertical-position);
    transform: translate(-50%, -50%); /* Center icon on coordinates, same as circles */
    z-index: 999; /* Very high z-index to ensure clickability over all other elements */
  }

  .chevron-icon svg {
    width: 16px;
    height: 16px;
    fill: hsl(var(--node-text) / 0.5);
    transition: fill 0.15s ease;
  }

  .chevron-icon:hover svg {
    fill: hsl(var(--node-text) / 0.5);
  }

  /* Show chevron only when hovering directly over this node's content wrapper (not child nodes) */
  .node-content-wrapper:hover > .chevron-icon {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .chevron-icon.expanded {
    transform: translate(-50%, -50%) rotate(90deg);
  }

  /* Inherit font-size, line-height, and icon positioning from HeaderNode wrapper classes */
  .node-content-wrapper:has(:global(.header-h1)) {
    --font-size: 2rem;
    --line-height: 1.2;
    --icon-vertical-position: calc(0.25rem + (2rem * 1.2 / 2));
  }

  .node-content-wrapper:has(:global(.header-h2)) {
    --font-size: 1.5rem;
    --line-height: 1.3;
    --icon-vertical-position: calc(0.25rem + (1.5rem * 1.3 / 2));
  }

  .node-content-wrapper:has(:global(.header-h3)) {
    --font-size: 1.25rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.25rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h4)) {
    --font-size: 1.125rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.125rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h5)) {
    --font-size: 1rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h6)) {
    --font-size: 0.875rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (0.875rem * 1.4 / 2));
  }
</style>
