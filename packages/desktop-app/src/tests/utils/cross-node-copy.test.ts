/**
 * Cross-node copy tests.
 *
 * Exercises the pure selection→markdown builder against real (Happy-DOM) node
 * fixtures: partial-first/partial-last clipping, full middle nodes, hierarchy
 * indentation, single-node passthrough, and the delicate rendered↔source offset
 * mapping (markdown syntax stripped in the view; `<br>` counted as one char).
 */

import { describe, it, expect, afterEach } from 'vitest';
import { buildCrossNodeCopy, type CopyNodeInfo } from '../../lib/utils/cross-node-copy';
import { renderedOffsetTo } from '../../lib/utils/rendered-text-offset';

interface NodeFixture {
  id: string;
  /** Source markdown (with syntax). */
  content: string;
  /** Rendered view markup (syntax stripped; may contain <br>). */
  view: string;
  depth: number;
}

function buildViewer(nodes: NodeFixture[]): {
  root: HTMLElement;
  resolveNode: (id: string) => CopyNodeInfo | null;
  viewOf: (id: string) => HTMLElement;
} {
  const root = document.createElement('div');
  const info = new Map<string, CopyNodeInfo>();
  for (const n of nodes) {
    const nodeEl = document.createElement('div');
    nodeEl.className = 'node'; // node roots carry class `node` (base-node.svelte)
    nodeEl.setAttribute('data-node-id', n.id);
    const view = document.createElement('div');
    view.className = 'node__content--view';
    view.innerHTML = n.view;
    nodeEl.appendChild(view);
    root.appendChild(nodeEl);
    info.set(n.id, { content: n.content, depth: n.depth });
  }
  document.body.appendChild(root);
  return {
    root,
    resolveNode: (id) => info.get(id) ?? null,
    viewOf: (id) => root.querySelector(`[data-node-id="${id}"] .node__content--view`) as HTMLElement
  };
}

/** A minimal Selection over one range (avoids Happy-DOM Selection quirks). */
function selectionOf(range: Range): Selection {
  return {
    rangeCount: 1,
    isCollapsed: range.collapsed,
    getRangeAt: () => range
  } as unknown as Selection;
}

/** First text node inside an element (the common view case). */
function textOf(el: HTMLElement): Text {
  return el.firstChild as Text;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('renderedOffsetTo', () => {
  it('counts plain text offsets', () => {
    const el = document.createElement('div');
    el.innerHTML = 'Hello world';
    expect(renderedOffsetTo(el, el.firstChild!, 6)).toBe(6);
  });

  it('counts a <br> as one character', () => {
    const el = document.createElement('div');
    el.innerHTML = 'line1<br>line2';
    const secondText = el.childNodes[2]; // "line2"
    // 5 (line1) + 1 (br) + 2 (offset into line2) = 8
    expect(renderedOffsetTo(el, secondText, 2)).toBe(8);
  });

  it('returns null when the position is outside the element', () => {
    const el = document.createElement('div');
    el.innerHTML = 'inside';
    const other = document.createElement('span');
    other.textContent = 'elsewhere';
    expect(renderedOffsetTo(el, other.firstChild!, 1)).toBeNull();
  });
});

describe('buildCrossNodeCopy', () => {
  it('returns null for a collapsed selection', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'Hello', view: 'Hello', depth: 0 }
    ]);
    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 2);
    range.collapse(true);
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBeNull();
  });

  it('returns null for a single-node selection (native copy)', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'Hello world', view: 'Hello world', depth: 0 }
    ]);
    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 0);
    range.setEnd(textOf(viewOf('a')), 5);
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBeNull();
  });

  it('clips the first and last node to the selection offsets', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'Hello world', view: 'Hello world', depth: 0 },
      { id: 'b', content: 'Second line', view: 'Second line', depth: 0 }
    ]);
    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 6); // "world"
    range.setEnd(textOf(viewOf('b')), 6); // "Second"
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBe(
      'world\nSecond'
    );
  });

  it('keeps middle nodes whole and maps markdown syntax at the edges', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: '**Hello** world', view: 'Hello world', depth: 0 },
      { id: 'b', content: 'middle node', view: 'middle node', depth: 0 },
      { id: 'c', content: 'end here', view: 'end here', depth: 0 }
    ]);
    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 6); // rendered 'w' → source offset 10 → "world"
    range.setEnd(textOf(viewOf('c')), 3); // "end"
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBe(
      'world\nmiddle node\nend'
    );
  });

  it('preserves nesting as indentation relative to the shallowest node', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'parent', view: 'parent', depth: 0 },
      { id: 'b', content: 'child', view: 'child', depth: 1 },
      { id: 'c', content: 'grandchild', view: 'grandchild', depth: 2 }
    ]);
    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 0);
    range.setEnd(textOf(viewOf('c')), 'grandchild'.length);
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBe(
      'parent\n  child\n    grandchild'
    );
  });

  it('counts <br> when clipping the first node', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'line1\nline2', view: 'line1<br>line2', depth: 0 },
      { id: 'b', content: 'second', view: 'second', depth: 0 }
    ]);
    const aView = viewOf('a');
    const range = document.createRange();
    range.setStart(aView.childNodes[2], 2); // "line2" offset 2 → rendered 8 → source slice(8) = "ne2"
    range.setEnd(textOf(viewOf('b')), 'second'.length);
    expect(buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode })).toBe(
      'ne2\nsecond'
    );
  });

  it('ignores inline reference chips that carry a data-node-id', () => {
    // A node's view can contain an inline `<a class="ns-noderef" data-node-id=…>`
    // reference to ANOTHER node. That chip must not be treated as a node boundary
    // (no phantom line, no wrong-node clipping) — only `.node[data-node-id]` roots
    // count.
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'see [Ref](nodespace://z) now', view: 'see Ref now', depth: 0 },
      { id: 'b', content: 'tail node', view: 'tail node', depth: 0 }
    ]);
    // Inject a reference chip inside node a's view, wrapping "Ref".
    const aView = viewOf('a');
    aView.innerHTML = 'see <a class="ns-noderef" data-node-id="z">Ref</a> now';

    // Make the referenced node 'z' resolvable with distinct content, so that if
    // the chip were (wrongly) treated as a spanned node it would inject a visible
    // third line — this is what makes the length assertion below discriminate.
    const resolve = (id: string) =>
      id === 'z' ? { content: 'PHANTOM REFERENCED NODE', depth: 0 } : resolveNode(id);

    const range = document.createRange();
    range.setStart(aView.firstChild!, 0); // start of node a ("see …")
    range.setEnd(textOf(viewOf('b')), 'tail node'.length);
    const result = buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode: resolve });

    // Two lines only (a, b) — the reference chip 'z' must not add a third.
    expect(result).not.toBeNull();
    expect(result).not.toContain('PHANTOM');
    expect(result!.split('\n')).toHaveLength(2);
    expect(result).toContain('tail node');
    // node a is whole (selection starts at its beginning): source syntax preserved.
    expect(result!.split('\n')[0]).toBe('see [Ref](nodespace://z) now');
  });

  it('resolves an endpoint inside a reference chip to the containing node', () => {
    const { root, resolveNode, viewOf } = buildViewer([
      { id: 'a', content: 'x', view: 'x', depth: 0 },
      { id: 'b', content: 'ref here', view: 'ref here', depth: 0 }
    ]);
    const bView = viewOf('b');
    bView.innerHTML = 'ref <a class="ns-noderef" data-node-id="z">here</a>';

    const range = document.createRange();
    range.setStart(textOf(viewOf('a')), 0);
    // End inside the reference chip's text — must resolve to node b, not node z.
    const chipText = bView.querySelector('a')!.firstChild!;
    range.setEnd(chipText, 2);
    const result = buildCrossNodeCopy({ selection: selectionOf(range), root, resolveNode });

    expect(result).not.toBeNull();
    expect(result!.split('\n')).toHaveLength(2); // a and b only
    expect(result!.split('\n')[0]).toBe('x');
    // Endpoint resolved to node b → b is the LAST node and is clipped to the
    // selection end (rendered offset 6 → "ref he"). If the chip 'z' were treated
    // as the end node, b would be a middle node and stay unclipped ("ref here").
    expect(result!.split('\n')[1]).toBe('ref he');
  });
});
