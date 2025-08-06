<!--
  Base Node Component
  
  Provides the foundational visual pattern for all node types in NodeSpace.
  Includes interaction states, accessibility features, and theme awareness.
-->

<script context="module" lang="ts">
  // Node type for styling variants
  export type NodeType = 'text' | 'task' | 'ai-chat' | 'entity' | 'query';
</script>

<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  // Props
  export let nodeType: NodeType = 'text';
  export let nodeId: string = '';
  export let title: string = '';
  export let subtitle: string = '';
  export let content: string = '';
  export let selected: boolean = false;
  export let disabled: boolean = false;
  export let loading: boolean = false;
  export let draggable: boolean = false;
  export let clickable: boolean = true;
  // focusable is automatically handled by clickable buttons
  // export let focusable: boolean = true;
  export let showActions: boolean = true;
  export let compact: boolean = false;
  export let hasChildren: boolean = false;
  export let className: string = '';

  // Interactive state
  let isDragging = false;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    click: { nodeId: string; event: MouseEvent };
    select: { nodeId: string; selected: boolean };
    focus: { nodeId: string; event: FocusEvent };
    blur: { nodeId: string; event: FocusEvent };
    keydown: { nodeId: string; event: KeyboardEvent };
    dragstart: { nodeId: string; event: DragEvent };
    dragend: { nodeId: string; event: DragEvent };
    action: { nodeId: string; action: string };
  }>();

  // Get node type icon
  function getNodeIcon(type: NodeType): string {
    switch (type) {
      case 'text':
        return '📝';
      case 'task':
        return '✅';
      case 'ai-chat':
        return '🤖';
      case 'entity':
        return '📊';
      case 'query':
        return '🔍';
      default:
        return '📄';
    }
  }

  // Get node type label
  function getNodeTypeLabel(type: NodeType): string {
    switch (type) {
      case 'text':
        return 'Text';
      case 'task':
        return 'Task';
      case 'ai-chat':
        return 'AI Chat';
      case 'entity':
        return 'Entity';
      case 'query':
        return 'Query';
      default:
        return 'Node';
    }
  }

  // Event handlers
  function handleClick(event: MouseEvent) {
    if (!clickable || disabled || loading) return;
    dispatch('click', { nodeId, event });
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (disabled) return;

    dispatch('keydown', { nodeId, event });

    // Handle selection toggle with Space or Enter
    if (event.code === 'Space' || event.code === 'Enter') {
      event.preventDefault();
      if (clickable) {
        dispatch('click', { nodeId, event: new MouseEvent('click') });
      }
    }

    // Handle selection with keyboard
    if (event.code === 'Space' && !event.shiftKey) {
      dispatch('select', { nodeId, selected: !selected });
    }
  }

  function handleFocus(event: FocusEvent) {
    if (disabled) return;
    dispatch('focus', { nodeId, event });
  }

  function handleBlur(event: FocusEvent) {
    dispatch('blur', { nodeId, event });
  }

  function handleDragStart(event: DragEvent) {
    if (!draggable || disabled) {
      event.preventDefault();
      return;
    }
    isDragging = true;
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = 'move';
      event.dataTransfer.setData('text/plain', nodeId);
      event.dataTransfer.setData(
        'application/x-nodespace-node',
        JSON.stringify({
          id: nodeId,
          type: nodeType,
          title,
          content
        })
      );
    }
    dispatch('dragstart', { nodeId, event });
  }

  function handleDragEnd(event: DragEvent) {
    isDragging = false;
    dispatch('dragend', { nodeId, event });
  }

  function handleAction(action: string) {
    if (disabled) return;
    dispatch('action', { nodeId, action });
  }

  // CSS classes
  $: nodeClasses = [
    'ns-node',
    `ns-node--${nodeType}`,
    selected && 'ns-node--selected',
    disabled && 'ns-node--disabled',
    loading && 'ns-node--loading',
    isDragging && 'ns-node--dragging',
    compact && 'ns-node--compact',
    hasChildren && 'ns-node--has-children',
    className
  ]
    .filter(Boolean)
    .join(' ');
</script>

{#if clickable}
  <!-- Interactive node container -->
  <button
    type="button"
    class={nodeClasses}
    aria-label="{getNodeTypeLabel(nodeType)}: {title || 'Untitled'}"
    aria-pressed={selected}
    {disabled}
    data-node-id={nodeId}
    data-node-type={nodeType}
    on:click={handleClick}
    on:keydown={handleKeyDown}
    on:focus={handleFocus}
    on:blur={handleBlur}
    on:mouseenter
    on:mouseleave
    on:dragstart={draggable && !disabled ? handleDragStart : undefined}
    on:dragend={draggable && !disabled ? handleDragEnd : undefined}
    draggable={draggable && !disabled}
  >
    <!-- Loading overlay -->
    {#if loading}
      <div class="ns-node__loading-overlay">
        <div class="ns-node__spinner" aria-label="Loading..."></div>
      </div>
    {/if}

    <!-- Node header -->
    <header class="ns-node__header">
      <!-- Node type indicator -->
      <div class="ns-node__type-indicator">
        <span class="ns-node__icon" role="img" aria-label={getNodeTypeLabel(nodeType)}>
          {getNodeIcon(nodeType)}
        </span>
      </div>

      <!-- Node title and subtitle -->
      <div class="ns-node__title-section">
        {#if title}
          <h3 class="ns-node__title">{title}</h3>
        {/if}
        {#if subtitle}
          <p class="ns-node__subtitle">{subtitle}</p>
        {/if}
      </div>

      <!-- Node actions -->
      {#if showActions && !loading && !disabled}
        <div class="ns-node__actions">
          <slot name="actions">
            <span
              class="ns-node__action-btn"
              role="button"
              tabindex="0"
              title="More actions"
              on:click|stopPropagation={() => handleAction('menu')}
              on:keydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleAction('menu');
                }
              }}
            >
              ⋯
            </span>
          </slot>
        </div>
      {/if}
    </header>

    <!-- Node content -->
    <div class="ns-node__content">
      <slot>
        {#if content}
          <p class="ns-node__default-content">{content}</p>
        {:else}
          <p class="ns-node__empty-content">No content</p>
        {/if}
      </slot>
    </div>

    <!-- Node footer -->
    <footer class="ns-node__footer">
      <slot name="footer" />
    </footer>

    <!-- Selection indicator -->
    {#if selected}
      <div class="ns-node__selection-indicator" aria-hidden="true"></div>
    {/if}

    <!-- Focus ring -->
    <div class="ns-node__focus-ring" aria-hidden="true"></div>
  </button>
{:else}
  <!-- Non-interactive node container -->
  <div
    class={nodeClasses}
    role={draggable && !disabled ? 'listitem' : 'article'}
    aria-label="{getNodeTypeLabel(nodeType)}: {title || 'Untitled'}"
    data-node-id={nodeId}
    data-node-type={nodeType}
    draggable={draggable && !disabled}
    on:dragstart={draggable && !disabled ? handleDragStart : undefined}
    on:dragend={draggable && !disabled ? handleDragEnd : undefined}
  >
    <!-- Loading overlay -->
    {#if loading}
      <div class="ns-node__loading-overlay">
        <div class="ns-node__spinner" aria-label="Loading..."></div>
      </div>
    {/if}

    <!-- Node header -->
    <header class="ns-node__header">
      <!-- Node type indicator -->
      <div class="ns-node__type-indicator">
        <span class="ns-node__icon" role="img" aria-label={getNodeTypeLabel(nodeType)}>
          {getNodeIcon(nodeType)}
        </span>
      </div>

      <!-- Node title and subtitle -->
      <div class="ns-node__title-section">
        {#if title}
          <h3 class="ns-node__title">{title}</h3>
        {/if}
        {#if subtitle}
          <p class="ns-node__subtitle">{subtitle}</p>
        {/if}
      </div>

      <!-- Node actions -->
      {#if showActions && !loading && !disabled}
        <div class="ns-node__actions">
          <slot name="actions">
            <span
              class="ns-node__action-btn"
              role="button"
              tabindex="0"
              title="More actions"
              on:click|stopPropagation={() => handleAction('menu')}
              on:keydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleAction('menu');
                }
              }}
            >
              ⋯
            </span>
          </slot>
        </div>
      {/if}
    </header>

    <!-- Node content -->
    <div class="ns-node__content">
      <slot>
        {#if content}
          <p class="ns-node__default-content">{content}</p>
        {:else}
          <p class="ns-node__empty-content">No content</p>
        {/if}
      </slot>
    </div>

    <!-- Node footer -->
    <footer class="ns-node__footer">
      <slot name="footer" />
    </footer>

    <!-- Selection indicator -->
    {#if selected}
      <div class="ns-node__selection-indicator" aria-hidden="true"></div>
    {/if}

    <!-- Focus ring -->
    <div class="ns-node__focus-ring" aria-hidden="true"></div>
  </div>
{/if}

<style>
  /* Base node styling using design system tokens */
  .ns-node {
    /* Reset button styles when used as button */
    background: none;
    font: inherit;
    text-align: inherit;
    position: relative;
    display: flex;
    flex-direction: column;
    background-color: var(--ns-node-state-idle-background);
    border: 1px solid var(--ns-node-state-idle-border);
    border-radius: var(--ns-radius-lg);
    box-shadow: var(--ns-node-state-idle-shadow);
    padding: var(--ns-spacing-4);
    margin: var(--ns-spacing-2);
    transition: all var(--ns-duration-fast) var(--ns-easing-easeInOut);
    cursor: pointer;
    outline: none;
    min-height: 80px;
    overflow: hidden;
  }

  /* Hover state */
  .ns-node:hover:not(.ns-node--disabled):not(.ns-node--loading) {
    background-color: var(--ns-node-state-hover-background);
    border-color: var(--ns-node-state-hover-border);
    box-shadow: var(--ns-node-state-hover-shadow);
    transform: translateY(-1px);
  }

  /* Focus state */
  .ns-node:focus-visible {
    background-color: var(--ns-node-state-focus-background);
    border-color: var(--ns-node-state-focus-border);
  }

  .ns-node:focus-visible .ns-node__focus-ring {
    opacity: 1;
    transform: scale(1);
  }

  /* Active state */
  .ns-node:active:not(.ns-node--disabled) {
    background-color: var(--ns-node-state-active-background);
    border-color: var(--ns-node-state-active-border);
    box-shadow: var(--ns-node-state-active-shadow);
    transform: translateY(0);
  }

  /* Selected state */
  .ns-node--selected {
    background-color: var(--ns-node-state-selected-background);
    border-color: var(--ns-node-state-selected-border);
    box-shadow: var(--ns-node-state-selected-shadow);
  }

  /* Disabled state */
  .ns-node--disabled {
    background-color: var(--ns-node-state-disabled-background);
    border-color: var(--ns-node-state-disabled-border);
    opacity: var(--ns-node-state-disabled-opacity);
    cursor: not-allowed;
    pointer-events: none;
  }

  /* Loading state */
  .ns-node--loading {
    pointer-events: none;
  }

  /* Dragging state */
  .ns-node--dragging {
    opacity: 0.8;
    transform: rotate(2deg) scale(1.02);
    z-index: 1000;
  }

  /* Compact variant */
  .ns-node--compact {
    padding: var(--ns-spacing-2) var(--ns-spacing-3);
    min-height: 48px;
  }

  .ns-node--compact .ns-node__header {
    margin-bottom: var(--ns-spacing-1);
  }

  /* Node type variants with accent colors */
  .ns-node--text {
    border-left: 4px solid var(--ns-node-text-accent);
  }

  .ns-node--task {
    border-left: 4px solid var(--ns-node-task-accent);
  }

  .ns-node--ai-chat {
    border-left: 4px solid var(--ns-node-aiChat-accent);
  }

  .ns-node--entity {
    border-left: 4px solid var(--ns-node-entity-accent);
  }

  .ns-node--query {
    border-left: 4px solid var(--ns-node-query-accent);
  }

  /* Header layout */
  .ns-node__header {
    display: flex;
    align-items: flex-start;
    gap: var(--ns-spacing-3);
    margin-bottom: var(--ns-spacing-3);
  }

  .ns-node__type-indicator {
    flex-shrink: 0;
    width: calc(var(--ns-font-size-base) * 1);
    height: calc(var(--ns-font-size-base) * 1);
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* Layered circle indicator system using pseudo-elements */
  .ns-node__type-indicator::before,
  .ns-node__type-indicator::after {
    content: '';
    position: absolute;
    border-radius: 50%;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
  }

  /* Background circle (for parent nodes) - 16px with semi-transparent background */
  .ns-node__type-indicator::before {
    width: calc(var(--ns-font-size-base) * 1);
    height: calc(var(--ns-font-size-base) * 1);
    background-color: transparent;
    border: none;
    z-index: 0;
  }

  /* Main circle - 10px solid circle centered in 16px container */
  .ns-node__type-indicator::after {
    width: calc(var(--ns-font-size-base) * 0.625);
    height: calc(var(--ns-font-size-base) * 0.625);
    background-color: var(--ns-node-text-accent);
    z-index: 1;
  }

  /* Node type variant colors for circles */
  .ns-node--text .ns-node__type-indicator::after {
    background-color: var(--ns-node-text-accent);
  }

  .ns-node--task .ns-node__type-indicator::after {
    background-color: var(--ns-node-task-accent);
  }

  .ns-node--ai-chat .ns-node__type-indicator::after {
    background-color: var(--ns-node-aiChat-accent);
  }

  .ns-node--entity .ns-node__type-indicator::after {
    background-color: var(--ns-node-entity-accent);
  }

  .ns-node--query .ns-node__type-indicator::after {
    background-color: var(--ns-node-query-accent);
  }

  /* Parent node indicator - show semi-transparent background circle at 25% opacity */
  .ns-node--text.ns-node--has-children .ns-node__type-indicator::before {
    background-color: var(--ns-node-text-accent);
    opacity: 0.25;
  }

  .ns-node--task.ns-node--has-children .ns-node__type-indicator::before {
    background-color: var(--ns-node-task-accent);
    opacity: 0.25;
  }

  .ns-node--ai-chat.ns-node--has-children .ns-node__type-indicator::before {
    background-color: var(--ns-node-aiChat-accent);
    opacity: 0.25;
  }

  .ns-node--entity.ns-node--has-children .ns-node__type-indicator::before {
    background-color: var(--ns-node-entity-accent);
    opacity: 0.25;
  }

  .ns-node--query.ns-node--has-children .ns-node__type-indicator::before {
    background-color: var(--ns-node-query-accent);
    opacity: 0.25;
  }

  .ns-node__icon {
    /* Hide the emoji icon as we're using circles now */
    display: none;
  }

  .ns-node__title-section {
    flex: 1;
    min-width: 0;
  }

  .ns-node__title {
    margin: 0 0 var(--ns-spacing-1) 0;
    font-size: var(--ns-font-size-base);
    font-weight: var(--ns-font-weight-semibold);
    line-height: var(--ns-line-height-tight);
    color: var(--ns-color-text-primary);
    word-break: break-word;
  }

  .ns-node__subtitle {
    margin: 0;
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-normal);
    color: var(--ns-color-text-secondary);
    word-break: break-word;
  }

  .ns-node__actions {
    flex-shrink: 0;
    opacity: 0;
    transition: opacity var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .ns-node:hover .ns-node__actions,
  .ns-node:focus-within .ns-node__actions {
    opacity: 1;
  }

  .ns-node__action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    background: none;
    border: none;
    border-radius: var(--ns-radius-sm);
    color: var(--ns-color-text-tertiary);
    cursor: pointer;
    transition: all var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .ns-node__action-btn:hover {
    background-color: var(--ns-color-surface-panel);
    color: var(--ns-color-text-secondary);
  }

  /* Content section */
  .ns-node__content {
    flex: 1;
    margin-bottom: var(--ns-spacing-2);
  }

  .ns-node__default-content {
    margin: 0;
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-normal);
    color: var(--ns-color-text-primary);
    word-break: break-word;
  }

  .ns-node__empty-content {
    margin: 0;
    font-size: var(--ns-font-size-sm);
    line-height: var(--ns-line-height-normal);
    color: var(--ns-color-text-placeholder);
    font-style: italic;
  }

  /* Footer section */
  .ns-node__footer {
    margin-top: auto;
  }

  /* Loading overlay */
  .ns-node__loading-overlay {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background-color: var(--ns-color-surface-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--ns-radius-lg);
    z-index: 10;
  }

  .ns-node__spinner {
    width: 20px;
    height: 20px;
    border: 2px solid var(--ns-color-border-default);
    border-top-color: var(--ns-color-primary-500);
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Selection indicator */
  .ns-node__selection-indicator {
    position: absolute;
    top: var(--ns-spacing-2);
    right: var(--ns-spacing-2);
    width: 8px;
    height: 8px;
    background-color: var(--ns-color-primary-500);
    border-radius: 50%;
    animation: pulse 2s infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }

  /* Focus ring */
  .ns-node__focus-ring {
    position: absolute;
    top: -3px;
    left: -3px;
    right: -3px;
    bottom: -3px;
    border: 2px solid var(--ns-color-primary-500);
    border-radius: calc(var(--ns-radius-lg) + 3px);
    opacity: 0;
    transform: scale(0.95);
    transition: all var(--ns-duration-fast) var(--ns-easing-easeInOut);
    pointer-events: none;
  }

  /* Responsive adjustments */
  @media (max-width: 640px) {
    .ns-node {
      margin: var(--ns-spacing-1);
      padding: var(--ns-spacing-3);
    }

    .ns-node__header {
      gap: var(--ns-spacing-2);
      margin-bottom: var(--ns-spacing-2);
    }

    .ns-node__title {
      font-size: var(--ns-font-size-sm);
    }
  }

  /* Print styles */
  @media print {
    .ns-node {
      box-shadow: none;
      border: 1px solid var(--ns-color-border-default);
      break-inside: avoid;
      margin-bottom: var(--ns-spacing-4);
    }

    .ns-node__actions,
    .ns-node__selection-indicator,
    .ns-node__focus-ring {
      display: none;
    }
  }
</style>
