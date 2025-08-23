<!--
BaseNode Reference Component
Core component for rendering node references in the universal reference system.
Minimal version for testing compatibility.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let nodeId: string = '';
  export let nodeType: string = 'text';
  export let title: string = '';
  export let content: string = '';
  export let uri: string = '';
  export let icon: string = '📝';
  export let color: string = '';
  export let ariaLabel: string = '';
  export let metadata: Record<string, unknown> = {};
  export let displayContext: 'inline' | 'popup' | 'preview' = 'inline';
  export let href: string = uri || '';
  export let className: string = '';

  const dispatch = createEventDispatcher<{
    navigate: { nodeId: string; uri: string };
    hover: { nodeId: string; metadata: Record<string, unknown> };
    focus: { nodeId: string };
  }>();

  function handleClick(event: MouseEvent) {
    event.preventDefault();
    dispatch('navigate', { nodeId, uri });
  }

  function handleMouseEnter() {
    dispatch('hover', { nodeId, metadata });
  }

  function handleFocus() {
    dispatch('focus', { nodeId });
  }

  $: computedAriaLabel = ariaLabel || `${nodeType}: ${title}`;
  $: displayText = title || content || nodeId || 'Unknown Node';
  $: finalHref = href || uri || '#';
  $: displayStyle =
    displayContext === 'popup' ? 'popup' : displayContext === 'preview' ? 'preview' : 'inline';
  $: colorStyle = color ? `color: ${color};` : '';
</script>

<a
  href={finalHref}
  class="ns-noderef ns-noderef-valid ns-noderef-{displayStyle} {className}"
  data-node-id={nodeId}
  data-node-type={nodeType}
  data-uri={uri}
  aria-label={computedAriaLabel}
  style={colorStyle}
  on:click={handleClick}
  on:mouseenter={handleMouseEnter}
  on:focus={handleFocus}
  tabindex="0"
>
  <span aria-hidden="true">{icon}</span>
  <span>{displayText}</span>
</a>
