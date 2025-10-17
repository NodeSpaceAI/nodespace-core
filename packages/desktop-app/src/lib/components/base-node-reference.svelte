<!--
BaseNode Reference Component
Core component for rendering node references in the universal reference system.
Minimal version for testing compatibility.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  // Svelte 5 runes: Props destructuring with defaults
  let {
    nodeId = '',
    nodeType = 'text',
    title = '',
    content = '',
    uri = '',
    icon = '📝',
    color = '',
    ariaLabel = '',
    metadata = {},
    displayContext = 'inline' as 'inline' | 'popup' | 'preview',
    href = uri || '',
    className = ''
  }: {
    nodeId?: string;
    nodeType?: string;
    title?: string;
    content?: string;
    uri?: string;
    icon?: string;
    color?: string;
    ariaLabel?: string;
    metadata?: Record<string, unknown>;
    displayContext?: 'inline' | 'popup' | 'preview';
    href?: string;
    className?: string;
  } = $props();

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

  // Svelte 5 runes: Derived reactive values
  let computedAriaLabel = $derived(ariaLabel || `${nodeType}: ${title}`);
  let displayText = $derived(title || content || nodeId || 'Unknown Node');
  let finalHref = $derived(href || uri || '#');
  let displayStyle = $derived(
    displayContext === 'popup' ? 'popup' : displayContext === 'preview' ? 'preview' : 'inline'
  );
  let colorStyle = $derived(color ? `color: ${color};` : '');
</script>

<a
  href={finalHref}
  class="ns-noderef ns-noderef-valid ns-noderef-{displayStyle} {className}"
  data-node-id={nodeId}
  data-node-type={nodeType}
  data-uri={uri}
  aria-label={computedAriaLabel}
  style={colorStyle}
  onclick={handleClick}
  onmouseenter={handleMouseEnter}
  onfocus={handleFocus}
  tabindex="0"
>
  <span aria-hidden="true">{icon}</span>
  <span>{displayText}</span>
</a>
