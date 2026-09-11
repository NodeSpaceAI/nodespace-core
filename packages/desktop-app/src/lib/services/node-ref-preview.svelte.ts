/**
 * Node Reference Hover Preview Controller
 *
 * Backs the hover/focus preview card for node references. Every reference the
 * app renders — the markdown-link form (`<a href="nodespace://id" class="ns-noderef">`
 * from view-mode-renderer) and the bare `[[id]]` wikilink form (NodeRefInline) —
 * resolves to the same `a.ns-noderef` anchor, so a single document-level trigger
 * in app-shell drives them all.
 *
 * The controller owns the timing (show after a short delay, so quick mouse-throughs
 * don't flash cards) and the resolved content; positioning and rendering live in
 * node-ref-preview.svelte. Reads go through SharedNodeStore, cache-first.
 */

import { sharedNodeStore } from './shared-node-store.svelte';
import { extractNodeIdFromHref } from '$lib/utils/external-links';
import { createLogger } from '$lib/utils/logger';
import type { Node } from '$lib/types/node';

const log = createLogger('NodeRefPreview');

/**
 * Owner key this singleton pins reachable nodes under (see
 * SharedNodeStore.pinNodes). A previewed node is routinely resolved via
 * ensureNode() with no structureTree relationship to whatever open tab the
 * pointer/focus happens to be over, so it isn't reachable via the
 * structureTree walk on its own. There's only ever one card/controller
 * instance, so a single fixed owner key is enough — pinNodes replaces the
 * owner's whole pin set on each call, so pinning the newly-hovered id
 * automatically releases the previously-hovered one.
 */
const PIN_OWNER_ID = 'node-ref-preview';

/** Delay before a hovered/focused reference reveals its preview card. */
export const PREVIEW_DELAY_MS = 450;

/** Maximum length of the content snippet shown in the card. */
export const SNIPPET_MAX_LENGTH = 220;

/**
 * DOM id of the (single) preview card. The active reference anchor points its
 * `aria-describedby` here while the card is shown, so screen-reader users get the
 * same title/snippet on focus that sighted users get on hover.
 */
export const PREVIEW_CARD_ID = 'node-ref-preview-card';

/**
 * Title shown for a resolved node: indexed title first, then the first content
 * line, then empty (the card falls back to the raw id in that case).
 */
export function buildPreviewTitle(node: Node): string {
  return node.title?.trim() || node.content?.split('\n')[0]?.trim() || '';
}

/**
 * A short, single-line content snippet for the card. The card already shows the
 * title on top; when the first content line is that same title (either indexed,
 * or derived from the first line when there is no indexed title) it is dropped
 * from the snippet so the text isn't shown twice.
 */
export function buildPreviewSnippet(node: Node, maxLength = SNIPPET_MAX_LENGTH): string {
  const content = (node.content ?? '').trim();
  if (!content) return '';

  const newlineIndex = content.indexOf('\n');
  const firstLine = (newlineIndex === -1 ? content : content.slice(0, newlineIndex)).trim();

  let body = content;
  if (firstLine === buildPreviewTitle(node)) {
    body = newlineIndex === -1 ? '' : content.slice(newlineIndex + 1);
  }

  const collapsed = body.replace(/\s+/g, ' ').trim();
  if (collapsed.length <= maxLength) return collapsed;
  return collapsed.slice(0, maxLength).trimEnd() + '…';
}

export interface NodeRefPreviewState {
  /** Whether the card is currently shown. */
  visible: boolean;
  /** The node id being previewed (also the card's not-found fallback text). */
  nodeId: string | null;
  /** Anchor the card is positioned against. */
  anchor: HTMLElement | null;
  /** True while the node is being resolved. */
  loading: boolean;
  /** True once resolution settled with no backing node. */
  notFound: boolean;
  title: string;
  snippet: string;
}

function emptyState(): NodeRefPreviewState {
  return {
    visible: false,
    nodeId: null,
    anchor: null,
    loading: false,
    notFound: false,
    title: '',
    snippet: ''
  };
}

class NodeRefPreviewController {
  state = $state<NodeRefPreviewState>(emptyState());

  #timer: ReturnType<typeof setTimeout> | null = null;
  /** The id we intend to show; guards async races when the pointer moves on. */
  #pendingId: string | null = null;
  /** The anchor we intend to show it against (paired with #pendingId). */
  #pendingAnchor: HTMLElement | null = null;
  /** Anchor currently carrying our aria-describedby, so we can clear it. */
  #describedAnchor: HTMLElement | null = null;

  /**
   * Arm a preview for a hovered/focused reference anchor. No-op for non-noderef
   * anchors and for broken (deleted) references. Idempotent while the same
   * anchor stays active.
   */
  requestPreview(anchor: HTMLElement): void {
    const href = anchor.getAttribute('href') ?? '';

    // Deleted references are inert broken links — never preview them.
    if (href.includes('deleted=true')) return;

    const nodeId = extractNodeIdFromHref(href);
    if (!nodeId) return;

    // Same reference (same id AND same anchor) already scheduled or shown: leave
    // the pending timer alone. Crossing nested spans inside a reference fires
    // repeated mouseover events on the same anchor — resetting the timer on each
    // would keep the delay from ever elapsing. A different anchor for the same id
    // (the same node referenced twice on a page) does re-anchor.
    if (this.#pendingId === nodeId && this.#pendingAnchor === anchor) return;

    this.#clearTimer();
    this.#pendingId = nodeId;
    this.#pendingAnchor = anchor;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#reveal(nodeId, anchor);
    }, PREVIEW_DELAY_MS);
  }

  async #reveal(nodeId: string, anchor: HTMLElement): Promise<void> {
    // Pin the node reachable for as long as the card may be showing it —
    // see PIN_OWNER_ID's doc comment. Not strictly required for correctness
    // today (the card's title/snippet below are a one-time snapshot, not a
    // live store read, so an eviction after this point can't flip an
    // already-shown card to "not found"), but it keeps this resolution
    // consistent with every other sharedNodeStore-backed reference display
    // and avoids a spurious re-fetch if the same id is re-hovered later.
    sharedNodeStore.pinNodes(PIN_OWNER_ID, [nodeId]);

    // Show the card immediately in a loading state, anchored in place.
    this.state.nodeId = nodeId;
    this.state.anchor = anchor;
    this.state.loading = true;
    this.state.notFound = false;
    this.state.title = '';
    this.state.snippet = '';
    this.state.visible = true;
    this.#setDescribedBy(anchor);

    let node = sharedNodeStore.getNode(nodeId);
    // Re-confirm against the backend if uncached, or cached but possibly
    // stale from a daemon reconnect (a WatchNodes outage can silently drop
    // the update that would have kept it current — see
    // SharedNodeStore.isPossiblyStale).
    if (!node || sharedNodeStore.isPossiblyStale(nodeId)) {
      try {
        node = await sharedNodeStore.ensureNode(nodeId);
      } catch (error) {
        log.warn(`Failed to resolve node reference ${nodeId}:`, error);
      }
    }

    // The pointer/focus may have moved on (or the card was hidden) while we
    // awaited — abandon this stale resolution rather than overwrite newer state.
    if (this.#pendingId !== nodeId || !this.state.visible) return;

    if (!node) {
      this.state.loading = false;
      this.state.notFound = true;
      return;
    }

    this.state.title = buildPreviewTitle(node);
    this.state.snippet = buildPreviewSnippet(node);
    this.state.loading = false;
  }

  /** Dismiss the card and cancel any pending reveal. */
  hide(): void {
    this.#clearTimer();
    this.#pendingId = null;
    this.#pendingAnchor = null;
    this.#setDescribedBy(null);
    sharedNodeStore.unpinAll(PIN_OWNER_ID);
    if (this.state.visible || this.state.anchor) {
      this.state = emptyState();
    }
  }

  /**
   * Point the given anchor's `aria-describedby` at the card (clearing it from any
   * previously described anchor). Passing null clears it entirely.
   */
  #setDescribedBy(anchor: HTMLElement | null): void {
    if (this.#describedAnchor && this.#describedAnchor !== anchor) {
      this.#describedAnchor.removeAttribute('aria-describedby');
    }
    if (anchor) {
      anchor.setAttribute('aria-describedby', PREVIEW_CARD_ID);
    }
    this.#describedAnchor = anchor;
  }

  #clearTimer(): void {
    if (this.#timer) {
      clearTimeout(this.#timer);
      this.#timer = null;
    }
  }
}

/** Shared singleton — the whole app previews through one card. */
export const nodeRefPreview = new NodeRefPreviewController();
