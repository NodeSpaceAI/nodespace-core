<!--
  Simplified Base Node Component
  
  Clean foundational visual pattern for all node types in NodeSpace.
  Simplified from ~400 lines to ~120-150 lines by removing over-engineering.
-->

<script context="module" lang="ts">
  // Node type for styling variants
  export type NodeType = 'text' | 'task' | 'ai-chat' | 'entity' | 'query';
</script>

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import Icon, { type IconName } from '../icons/index.js';

  // Essential Props (Simplified to 5 core + processing)
  export let nodeType: NodeType = 'text';
  export let nodeId: string = '';
  export let content: string = '';
  export let hasChildren: boolean = false;
  export let className: string = '';

  // SVG Icon System
  export let iconName: IconName | undefined = undefined;

  // Processing State System
  export let isProcessing: boolean = false;
  export let processingAnimation: 'blink' | 'pulse' | 'spin' | 'fade' = 'pulse';
  export let processingIcon: IconName | undefined = undefined;

  // Simple event dispatcher for click events
  const dispatch = createEventDispatcher<{
    click: { nodeId: string; event: MouseEvent };
  }>();

  // Event handler
  function handleClick(event: MouseEvent) {
    dispatch('click', { nodeId, event });
  }

  function handleKeyDown(event: KeyboardEvent) {
    // Handle click with Enter or Space
    if (event.code === 'Enter' || event.code === 'Space') {
      event.preventDefault();
      dispatch('click', { nodeId, event: new MouseEvent('click') });
    }
  }

  // Get node type label for accessibility
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

  // CSS classes
  $: nodeClasses = [
    'ns-node',
    `ns-node--${nodeType}`,
    hasChildren && 'ns-node--has-children',
    isProcessing && 'ns-node--processing',
    className
  ]
    .filter(Boolean)
    .join(' ');

  // Icon selection: processing icon overrides regular icon when processing
  $: displayIcon = isProcessing && processingIcon ? processingIcon : iconName;

  // Animation class for processing state
  $: iconAnimationClass = isProcessing ? `ns-node__icon--${processingAnimation}` : '';
</script>

<!-- Simplified node container - single template approach -->
<button
  type="button"
  class={nodeClasses}
  aria-label="{getNodeTypeLabel(nodeType)}: {content || 'Empty node'}"
  data-node-id={nodeId}
  data-node-type={nodeType}
  on:click={handleClick}
  on:keydown={handleKeyDown}
>
  <!-- Node header -->
  <header class="ns-node__header">
    <!-- Icon or Circle Indicator -->
    <div class="ns-node__indicator" data-node-type={nodeType}>
      {#if displayIcon}
        <Icon name={displayIcon} size={16} className="ns-node__icon {iconAnimationClass}" />
      {:else}
        <!-- Circle fallback indicator -->
        <div
          class="ns-node__circle {iconAnimationClass}"
          class:ns-node__circle--has-children={hasChildren}
        ></div>
      {/if}
    </div>

    <!-- Node content -->
    <div class="ns-node__content">
      <slot>
        {#if content}
          <span class="ns-node__text">{content}</span>
        {:else}
          <span class="ns-node__empty">Empty</span>
        {/if}
      </slot>
    </div>
  </header>
</button>

<style>
  /* Simplified Base Node Styling */
  .ns-node {
    /* Reset button styles */
    background: none;
    font: inherit;
    text-align: left;

    /* Layout */
    display: flex;
    width: 100%;
    padding: 12px;
    margin: 4px 0;

    /* Styling */
    background-color: transparent;
    border: none;
    border-radius: 8px;
    cursor: text;
    outline: none;
    transition: all 200ms ease;
  }

  /* Hover state - minimal for clean appearance - no styles needed */

  /* Focus state */
  .ns-node:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }

  /* Processing state - indication through circle animation, no border styles needed */

  /* Header layout */
  .ns-node__header {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  /* Indicator container */
  .ns-node__indicator {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* SVG Icon styling */
  .ns-node__icon {
    color: currentColor;
  }

  /* Circle fallback indicator */
  .ns-node__circle {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    position: relative;
  }

  /* Parent nodes get a ring effect */
  .ns-node__circle--has-children::before {
    content: '';
    position: absolute;
    top: -3px;
    left: -3px;
    right: -3px;
    bottom: -3px;
    border-radius: 50%;
    border: 1px solid currentColor;
    opacity: 0.3;
  }

  /* Node type colors for circles */
  .ns-node--text .ns-node__circle {
    background-color: hsl(142 71% 45%);
  }
  .ns-node--task .ns-node__circle {
    background-color: hsl(25 95% 53%);
  }
  .ns-node--ai-chat .ns-node__circle {
    background-color: hsl(221 83% 53%);
  }
  .ns-node--entity .ns-node__circle {
    background-color: hsl(271 81% 56%);
  }
  .ns-node--query .ns-node__circle {
    background-color: hsl(330 81% 60%);
  }

  /* Content section */
  .ns-node__content {
    flex: 1;
    min-width: 0; /* Prevent overflow */
  }

  .ns-node__text {
    color: hsl(var(--foreground));
    font-size: 14px;
    line-height: 1.4;
    word-break: break-word;
  }

  .ns-node__empty {
    color: hsl(var(--muted-foreground));
    font-size: 14px;
    font-style: italic;
  }

  /* Processing Animations */
  .ns-node__icon--pulse,
  .ns-node__circle--pulse {
    animation: pulse 2s infinite;
  }

  .ns-node__icon--blink,
  .ns-node__circle--blink {
    animation: blink 1s infinite;
  }

  .ns-node__icon--spin,
  .ns-node__circle--spin {
    animation: spin 1s linear infinite;
  }

  .ns-node__icon--fade,
  .ns-node__circle--fade {
    animation: fade 2s infinite;
  }

  /* Animation keyframes */
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.7;
      transform: scale(1.1);
    }
  }

  @keyframes blink {
    0%,
    50% {
      opacity: 1;
    }
    51%,
    100% {
      opacity: 0.3;
    }
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes fade {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  /* Responsive adjustments */
  @media (max-width: 640px) {
    .ns-node {
      padding: 8px;
      margin: 2px 0;
    }

    .ns-node__header {
      gap: 8px;
    }

    .ns-node__text {
      font-size: 13px;
    }
  }
</style>
