<!--
  NodeRefPreview

  The single hover/focus preview card for node references. Driven by the shared
  nodeRefPreview controller (timing + resolved content); this component owns only
  presentation and viewport-aware positioning against the active anchor.

  Rendered once, near the root of app-shell. The card is pointer-events:none so it
  never interferes with the reference beneath it and can't trap the pointer — it is
  dismissed purely by the anchor losing hover/focus (handled in app-shell).
-->

<script lang="ts">
  import { nodeRefPreview, PREVIEW_CARD_ID } from '$lib/services/node-ref-preview.svelte';

  const preview = $derived(nodeRefPreview.state);

  // Gap between the anchor and the card; viewport margin for clamping.
  const GAP = 6;
  const MARGIN = 8;

  let cardEl = $state<HTMLElement | null>(null);
  let coords = $state<{ top: number; left: number } | null>(null);

  $effect(() => {
    // Register the resolved-content fields as dependencies so the effect re-runs
    // (and re-measures) when the card grows from its loading size to its final
    // title + snippet size — otherwise the flip-above/clamp decision would be made
    // against the short loading card and a tall card near the viewport bottom would
    // overflow. getBoundingClientRect isn't reactive, but these deps change exactly
    // when a fresh measurement is needed, and app-shell hides on scroll so a stale
    // rect can't linger.
    void preview.loading;
    void preview.notFound;
    void preview.title;
    void preview.snippet;

    if (!preview.visible || !preview.anchor || !cardEl) {
      coords = null;
      return;
    }

    const anchorRect = preview.anchor.getBoundingClientRect();
    const cardRect = cardEl.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Prefer below the anchor; flip above if it would overflow and there's room.
    let top = anchorRect.bottom + GAP;
    if (top + cardRect.height + MARGIN > vh) {
      const above = anchorRect.top - GAP - cardRect.height;
      if (above >= MARGIN) top = above;
    }

    // Left-align to the anchor, clamped into the viewport.
    let left = anchorRect.left;
    if (left + cardRect.width + MARGIN > vw) left = vw - cardRect.width - MARGIN;
    if (left < MARGIN) left = MARGIN;

    coords = { top, left };
  });
</script>

{#if preview.visible}
  <div
    bind:this={cardEl}
    id={PREVIEW_CARD_ID}
    class="node-ref-preview"
    class:node-ref-preview--placed={coords !== null}
    role="tooltip"
    style={coords ? `top: ${coords.top}px; left: ${coords.left}px;` : ''}
  >
    {#if preview.loading}
      <div class="node-ref-preview__status">Loading…</div>
    {:else if preview.notFound}
      <div class="node-ref-preview__status">Node not found</div>
    {:else}
      <div class="node-ref-preview__title">{preview.title || preview.nodeId}</div>
      {#if preview.snippet}
        <div class="node-ref-preview__snippet">{preview.snippet}</div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .node-ref-preview {
    position: fixed;
    /* Hidden until measured/placed to avoid a flash at (0, 0). */
    top: 0;
    left: 0;
    visibility: hidden;
    opacity: 0;
    z-index: 1000;
    max-width: 320px;
    padding: 0.625rem 0.75rem;
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: calc(var(--radius) * 0.75);
    box-shadow: 0 4px 16px hsl(var(--border) / 0.35);
    /* Never intercept pointer events: the card can't trap the cursor, so the
       reference below it drives show/hide entirely through hover/focus. */
    pointer-events: none;
  }

  .node-ref-preview--placed {
    visibility: visible;
    opacity: 1;
  }

  .node-ref-preview__title {
    font-weight: 600;
    font-size: 0.875rem;
    line-height: 1.3;
    /* Clamp a long title to two lines. */
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .node-ref-preview__snippet {
    margin-top: 0.375rem;
    font-size: 0.8125rem;
    line-height: 1.4;
    color: hsl(var(--muted-foreground));
    /* Clamp the snippet to three lines. */
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .node-ref-preview__status {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }
</style>
