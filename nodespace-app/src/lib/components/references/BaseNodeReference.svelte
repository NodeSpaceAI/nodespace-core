<!--
  BaseNodeReference - Foundation Reference Component
  
  Provides the base implementation for all node reference decorations.
  All other reference components should extend or compose with this component.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  // Required props
  export let nodeId: string = '';
  export let content: string = '';
  export let href: string = '';
  export let nodeType: string = 'base';

  // Optional styling props
  export let className: string = '';
  export let style: string = '';
  export let ariaLabel: string = '';

  // Optional icon override
  export let icon: string = '';

  // Optional disabled state
  export let disabled: boolean = false;

  const dispatch = createEventDispatcher();

  // Default icons by node type
  const nodeTypeIcons: Record<string, string> = {
    base: '📄',
    text: '📝',
    task: '☐',
    user: '👤',
    date: '📅',
    document: '📄',
    ai_chat: '🤖',
    entity: '🏷️',
    query: '🔍'
  };

  $: displayIcon = icon || nodeTypeIcons[nodeType] || nodeTypeIcons.base;
  $: computedAriaLabel = ariaLabel || `Reference to ${nodeType}: ${content}`;

  function handleClick(event: MouseEvent) {
    if (disabled) {
      event.preventDefault();
      return;
    }

    dispatch('nodeClick', {
      nodeId,
      href,
      nodeType,
      content,
      event
    });
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (disabled) return;

    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      handleClick(event as MouseEvent);
    }
  }
</script>

<a
  {href}
  class="ns-noderef ns-noderef--{nodeType} {className}"
  class:ns-noderef--disabled={disabled}
  {style}
  data-node-id={nodeId}
  data-node-type={nodeType}
  tabindex={disabled ? -1 : 0}
  aria-label={computedAriaLabel}
  on:click={handleClick}
  on:keydown={handleKeyDown}
>
  <span class="ns-noderef__icon" aria-hidden="true">
    {displayIcon}
  </span>

  <span class="ns-noderef__content">
    <slot name="content">
      {content}
    </slot>
  </span>

  <slot name="extras"></slot>
</a>

<style>
  .ns-noderef {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.125rem 0.375rem;
    border-radius: 0.25rem;
    text-decoration: none;
    color: hsl(var(--foreground));
    transition: all 0.15s ease;
    cursor: pointer;
    border: 1px solid transparent;
    font-size: 0.875rem;
    line-height: 1.25;
    max-width: 100%;
  }

  .ns-noderef:hover {
    background: hsl(var(--accent) / 0.1);
    border-color: hsl(var(--accent) / 0.3);
  }

  .ns-noderef:focus {
    outline: none;
    background: hsl(var(--accent) / 0.1);
    border-color: hsl(var(--accent));
    box-shadow: 0 0 0 2px hsl(var(--accent) / 0.2);
  }

  .ns-noderef--disabled {
    opacity: 0.5;
    cursor: not-allowed;
    pointer-events: none;
  }

  .ns-noderef__icon {
    flex-shrink: 0;
    font-size: 1em;
    line-height: 1;
  }

  .ns-noderef__content {
    flex: 1;
    min-width: 0; /* Allow text truncation */
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Node type specific styling */
  .ns-noderef--task {
    border-color: hsl(var(--chart-2) / 0.3);
  }

  .ns-noderef--task:hover {
    background: hsl(var(--chart-2) / 0.1);
    border-color: hsl(var(--chart-2) / 0.5);
  }

  .ns-noderef--user {
    border-color: hsl(var(--chart-1) / 0.3);
  }

  .ns-noderef--user:hover {
    background: hsl(var(--chart-1) / 0.1);
    border-color: hsl(var(--chart-1) / 0.5);
  }

  .ns-noderef--date {
    border-color: hsl(var(--chart-3) / 0.3);
  }

  .ns-noderef--date:hover {
    background: hsl(var(--chart-3) / 0.1);
    border-color: hsl(var(--chart-3) / 0.5);
  }

  .ns-noderef--document {
    border-color: hsl(var(--chart-4) / 0.3);
  }

  .ns-noderef--document:hover {
    background: hsl(var(--chart-4) / 0.1);
    border-color: hsl(var(--chart-4) / 0.5);
  }

  .ns-noderef--ai_chat {
    border-color: hsl(var(--chart-5) / 0.3);
  }

  .ns-noderef--ai_chat:hover {
    background: hsl(var(--chart-5) / 0.1);
    border-color: hsl(var(--chart-5) / 0.5);
  }

  /* Error handling and fallback styling */
  :global(.ns-fallback-component) {
    background: hsl(var(--muted) / 0.3);
    border: 1px dashed hsl(var(--muted-foreground) / 0.5);
    opacity: 0.8;
  }

  :global(.ns-hydration-error) {
    background: hsl(var(--destructive) / 0.1);
    border: 1px solid hsl(var(--destructive) / 0.3);
    border-radius: 0.25rem;
    padding: 0.25rem;
  }

  :global(.ns-error-fallback) {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    color: hsl(var(--destructive));
    font-size: 0.875rem;
  }

  :global(.ns-error-icon) {
    font-size: 0.75rem;
  }

  :global(.ns-error-type) {
    font-size: 0.75rem;
    opacity: 0.7;
    font-weight: 500;
  }
</style>
