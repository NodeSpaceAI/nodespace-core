/**
 * Cross-node copy.
 *
 * A page of nodes is ordinary DOM — only the focused node is a textarea, the
 * rest are plain view `<div>`s (see base-node.svelte) — so a text selection can
 * span several nodes. Part 1 stopped the view-container click handler from
 * collapsing such a selection. This module turns the selection into useful
 * clipboard text on `copy`: the underlying node **source** markdown (with syntax),
 * clipped to the selection at the first and last node, indented to preserve
 * nesting — rather than the browser default (rendered text, syntax already
 * stripped by ViewModeRenderer, no node boundaries).
 *
 * A single-node selection is left to the browser (returns null): what already
 * works is not intercepted.
 *
 * The rendered↔source offset mapping is the delicate part. Rendered offsets are
 * counted exactly the way base-node.svelte's `extractTextWithLineBreaks` counts
 * them (text length, +1 per `<br>`), then mapped to source offsets with
 * `mapViewPositionToEditPosition` — the same mapping the click-to-edit-at-character
 * handler uses.
 *
 * Known limitations, both confined to a single clip boundary (whole and middle
 * nodes are always exact source): (1) the shared offset mapper accounts for
 * emphasis/code/heading/list syntax but not markdown-link / node-reference source
 * (`[Title](nodespace://id)`), so a first/last-node boundary that falls *after* a
 * reference within that node can map imprecisely; (2) a boundary landing inside a
 * paired inline marker slices mid-pair and can yield an unbalanced marker
 * (e.g. `**bold`). Both are inherent to slicing source at a rendered boundary.
 *
 * Note `[data-node-id]` is NOT unique to node roots: inline references render as
 * `<a class="ns-noderef" data-node-id={refId}>` inside a node's view. Node roots
 * carry class `node` (base-node.svelte), so the scan is scoped to
 * `.node[data-node-id]` to exclude reference chips.
 */

import { isActiveTextSelection } from './text-selection';
import { mapViewPositionToEditPosition } from './view-edit-mapper';
import { renderedOffsetTo, extractRenderedText } from './rendered-text-offset';

/** Source content + tree depth for one node, resolved from the store/viewer. */
export interface CopyNodeInfo {
  /** The node's source markdown (with syntax). */
  content: string;
  /** Tree depth, used to preserve nesting as indentation. */
  depth: number;
}

export type CopyNodeResolver = (nodeId: string) => CopyNodeInfo | null;

export interface BuildCrossNodeCopyParams {
  selection: Selection | null;
  /** The viewer container to scope the node scan to (one pane). */
  root: HTMLElement;
  resolveNode: CopyNodeResolver;
}

/** Spaces of indentation per relative depth level in the copied markdown. */
const INDENT_UNIT = '  ';

/** CSS selector for a node root (excludes inline `.ns-noderef` reference chips). */
const NODE_ROOT_SELECTOR = '.node[data-node-id]';

/** The nearest node-root ancestor of a DOM node, within `root`. */
function closestNodeElement(node: Node | null, root: HTMLElement): HTMLElement | null {
  let el = node instanceof Element ? node : (node?.parentElement ?? null);
  el = el?.closest(NODE_ROOT_SELECTOR) ?? null;
  return el instanceof HTMLElement && root.contains(el) ? el : null;
}

/** A node's own rendered view element (absent while the node is being edited). */
function viewElementOf(nodeEl: HTMLElement): HTMLElement | null {
  const view = nodeEl.querySelector('.node__content--view');
  return view instanceof HTMLElement ? view : null;
}

/** Source offset for a rendered position within a node, via the shared mapper. */
function sourceOffsetAt(
  viewEl: HTMLElement,
  container: Node,
  offset: number,
  content: string
): number | null {
  const rendered = renderedOffsetTo(viewEl, container, offset);
  if (rendered === null) return null;
  const viewText = extractRenderedText(viewEl);
  return mapViewPositionToEditPosition(rendered, viewText, content);
}

/** Indent every line of `content` by `levels` units. */
function indent(content: string, levels: number): string {
  if (levels <= 0) return content;
  const pad = INDENT_UNIT.repeat(levels);
  return content
    .split('\n')
    .map((line) => pad + line)
    .join('\n');
}

/**
 * Build the clipboard markdown for a cross-node text selection, or return null
 * when the selection is empty or confined to a single node (leave it to the
 * browser). The caller is responsible for `preventDefault` + `setData` only when
 * this returns a string.
 */
export function buildCrossNodeCopy(params: BuildCrossNodeCopyParams): string | null {
  const { selection, root, resolveNode } = params;
  if (!isActiveTextSelection(selection)) return null;

  const range = selection!.getRangeAt(0);
  const startEl = closestNodeElement(range.startContainer, root);
  const endEl = closestNodeElement(range.endContainer, root);
  if (!startEl || !endEl) return null;

  const startId = startEl.dataset.nodeId;
  const endId = endEl.dataset.nodeId;
  if (!startId || !endId || startId === endId) {
    // Single node (or unresolved) — native copy already does the right thing.
    return null;
  }

  // Collect the spanned node roots in document order, scoped to this viewer.
  // A DOM Range is always normalized (startContainer precedes endContainer in
  // document order), so startEl is at or before endEl and no direction swap is
  // needed: the range start clips the first node, the range end clips the last.
  const allNodeEls = Array.from(root.querySelectorAll<HTMLElement>(NODE_ROOT_SELECTOR));
  const startIdx = allNodeEls.indexOf(startEl);
  const endIdx = allNodeEls.indexOf(endEl);
  if (startIdx === -1 || endIdx === -1 || startIdx > endIdx) return null;
  const spanned = allNodeEls.slice(startIdx, endIdx + 1);

  // The selection's shallowest node anchors indentation at column 0.
  const infos = spanned.map((el) => resolveNode(el.dataset.nodeId ?? ''));
  const minDepth = infos.reduce(
    (min, info) => (info ? Math.min(min, info.depth) : min),
    Number.POSITIVE_INFINITY
  );
  const baseDepth = Number.isFinite(minDepth) ? minDepth : 0;

  const clipStart = { container: range.startContainer, offset: range.startOffset };
  const clipEnd = { container: range.endContainer, offset: range.endOffset };

  const lines: string[] = [];

  spanned.forEach((el, i) => {
    const info = infos[i];
    if (!info) return;
    let content = info.content;
    const viewEl = viewElementOf(el);

    if (viewEl) {
      if (i === 0) {
        const from = sourceOffsetAt(viewEl, clipStart.container, clipStart.offset, content);
        if (from !== null) content = content.slice(from);
      }
      if (i === spanned.length - 1) {
        const to = sourceOffsetAt(viewEl, clipEnd.container, clipEnd.offset, content);
        // `to` is an offset into the (already start-clipped) content only when the
        // same node is both first and last — which can't happen here (ids differ),
        // so the last node was never start-clipped and `to` indexes full content.
        if (to !== null) content = content.slice(0, to);
      }
    }

    lines.push(indent(content, info.depth - baseDepth));
  });

  return lines.join('\n');
}
