import { describe, it, expect, afterEach, vi } from 'vitest';
import {
  createMockElementForView,
  findCharacterFromClick,
  findViewOffsetFromClick,
  MAX_MOCK_ELEMENT_CHARS
} from '$lib/design/components/cursor-positioning';

/**
 * Click-to-caret positioning must cost the same regardless of content
 * length. The primary path (`findViewOffsetFromClick`) hit-tests the live,
 * already-rendered view element via the browser's native
 * `caretRangeFromPoint` — no DOM construction. The fallback
 * (`createMockElementForView` + `findCharacterFromClick`) is only used when
 * native hit-testing is unavailable (this repo's test environment, Happy-DOM,
 * has none — confirmed below), and is capped so it never builds one span per
 * character of arbitrarily long content.
 */

type CaretRangeFn = (x: number, y: number) => Range | null;

/**
 * Happy-DOM doesn't implement `caretRangeFromPoint` at all (it's not on the
 * prototype), so `vi.spyOn` can't be used to stub it — spyOn requires the
 * property to already exist as a function. Define it directly instead.
 */
function installCaretRangeFromPoint(impl: CaretRangeFn): void {
  Object.defineProperty(document, 'caretRangeFromPoint', {
    value: impl,
    configurable: true,
    writable: true
  });
}

function uninstallCaretRangeFromPoint(): void {
  delete (document as unknown as Record<string, unknown>).caretRangeFromPoint;
}

/** Install a fixed DOM position as caretRangeFromPoint's resolved result. */
function stubCaretRangeFromPoint(container: Node, offset: number): void {
  const range = document.createRange();
  range.setStart(container, offset);
  range.collapse(true);
  installCaretRangeFromPoint(() => range);
}

/** A monospace grid layout: char index -> (row, col) from content, ignoring wrapping. */
function gridPositionFor(content: string, index: number): { row: number; col: number } {
  const before = content.slice(0, index);
  const row = (before.match(/\n/g) ?? []).length;
  const lastNl = before.lastIndexOf('\n');
  const col = index - (lastNl + 1);
  return { row, col };
}

const CHAR_W = 10;
const LINE_H = 20;

function makeRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
    x: left,
    y: top,
    toJSON: () => ({})
  } as DOMRect;
}

/**
 * Stub geometry for the OLD (mock-element) code path: every `[data-position]`
 * span gets a deterministic monospace-grid rect derived from `content`, so
 * `findCharacterFromClick`'s distance search resolves to a known index for a
 * given click point. The mock container itself (no `data-position`) sits at
 * the origin.
 */
function stubMockElementGeometry(content: string) {
  return vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (
    this: Element
  ) {
    const position = (this as HTMLElement).dataset?.position;
    if (position === undefined) {
      return makeRect(0, 0, 0, 0);
    }
    const { row, col } = gridPositionFor(content, Number(position));
    return makeRect(col * CHAR_W, row * LINE_H, CHAR_W, LINE_H);
  });
}

/** Click point at the center of the grid cell for `index` in `content`. */
function clickPointFor(content: string, index: number): { x: number; y: number } {
  const { row, col } = gridPositionFor(content, index);
  return { x: col * CHAR_W + CHAR_W / 2, y: row * LINE_H + LINE_H / 2 };
}

/** Build a live view element rendering `content` with real text nodes + <br>. */
function buildViewElement(content: string): HTMLDivElement {
  const view = document.createElement('div');
  const lines = content.split('\n');
  lines.forEach((line, i) => {
    if (i > 0) view.appendChild(document.createElement('br'));
    view.appendChild(document.createTextNode(line));
  });
  document.body.appendChild(view);
  return view;
}

/** Locate the (text node, offset) inside `view` for a rendered-text `index`, mirroring buildViewElement. */
function domPositionFor(view: HTMLDivElement, content: string, index: number): [Node, number] {
  const { row, col } = gridPositionFor(content, index);
  const textNodes = Array.from(view.childNodes).filter((n) => n.nodeType === Node.TEXT_NODE);
  return [textNodes[row], col];
}

describe('findViewOffsetFromClick (native caret hit-testing)', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    uninstallCaretRangeFromPoint();
    document.body.innerHTML = '';
  });

  it('returns null when caretRangeFromPoint is unavailable (e.g. Happy-DOM)', () => {
    const view = buildViewElement('Hello world');
    // Happy-DOM does not implement caretRangeFromPoint, so this is the ambient
    // default in this suite — asserted explicitly for clarity/documentation.
    expect(typeof document.caretRangeFromPoint).not.toBe('function');
    expect(findViewOffsetFromClick(view, 5, 5)).toBeNull();
  });

  it('returns null when caretRangeFromPoint returns null', () => {
    const view = buildViewElement('Hello world');
    installCaretRangeFromPoint(() => null);
    expect(findViewOffsetFromClick(view, 5, 5)).toBeNull();
  });

  it('returns null when caretRangeFromPoint resolves outside the view element', () => {
    const view = buildViewElement('Hello world');
    const other = document.createElement('div');
    other.textContent = 'elsewhere';
    document.body.appendChild(other);
    stubCaretRangeFromPoint(other.firstChild!, 2);

    expect(findViewOffsetFromClick(view, 5, 5)).toBeNull();
  });

  it('returns null and does not throw when caretRangeFromPoint itself throws', () => {
    const view = buildViewElement('Hello world');
    installCaretRangeFromPoint(() => {
      throw new Error('boom');
    });
    expect(() => findViewOffsetFromClick(view, 5, 5)).not.toThrow();
    expect(findViewOffsetFromClick(view, 5, 5)).toBeNull();
  });

  it('resolves the start, middle and end of single-line content', () => {
    const content = 'Hello world';
    const view = buildViewElement(content);

    for (const index of [0, 6, content.length]) {
      const [container, offset] = domPositionFor(view, content, index);
      stubCaretRangeFromPoint(container, offset);
      expect(findViewOffsetFromClick(view, 0, 0)).toBe(index);
    }
  });

  it('resolves a position on a later line, counting <br> as +1', () => {
    const content = 'Line 1\nLine 2\nLine 3';
    const view = buildViewElement(content);

    // 'L' of "Line 3" — after "Line 1\n" (7) + "Line 2\n" (7) = 14
    const targetIndex = 14;
    const [container, offset] = domPositionFor(view, content, targetIndex);
    stubCaretRangeFromPoint(container, offset);

    expect(findViewOffsetFromClick(view, 0, 0)).toBe(targetIndex);
  });
});

describe('createMockElementForView: bounded span creation (fallback path)', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('creates one span per character for content within the cap (existing behavior)', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);

    const content = 'a'.repeat(MAX_MOCK_ELEMENT_CHARS);
    const mockElement = createMockElementForView(viewDiv, content);

    expect(mockElement.querySelectorAll('[data-position]').length).toBe(content.length);

    mockElement.remove();
    viewDiv.remove();
  });

  it('does not create one span per character for long content (50k chars)', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);

    const longContent = 'a'.repeat(50_000);
    const createElementSpy = vi.spyOn(document, 'createElement');

    const mockElement = createMockElementForView(viewDiv, longContent);

    const spanCreations = createElementSpy.mock.calls.filter(([tag]) => tag === 'span').length;

    // Bounded by the cap, nowhere near one span per character of 50,000 chars.
    expect(spanCreations).toBe(MAX_MOCK_ELEMENT_CHARS);
    expect(spanCreations).toBeLessThan(longContent.length);
    expect(mockElement.querySelectorAll('[data-position]').length).toBe(MAX_MOCK_ELEMENT_CHARS);

    mockElement.remove();
    viewDiv.remove();
    createElementSpy.mockRestore();
  });

  it('windows the capped fallback around the click point instead of always keeping a prefix', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);
    const content = 'a'.repeat(50_000);

    // Happy-DOM has no real layout, so getBoundingClientRect defaults to all
    // zeros; stub the view element's rect the way it would actually appear —
    // this is the TRUE rendered geometry the window-centering estimate reads
    // from, unlike the mock's own (irrelevant here) geometry.
    vi.spyOn(viewDiv, 'getBoundingClientRect').mockReturnValue({
      left: 0,
      top: 0,
      right: 500,
      bottom: 1000,
      width: 500,
      height: 1000,
      x: 0,
      y: 0,
      toJSON: () => ({})
    } as DOMRect);

    // A click at the very bottom of the element should window around the end
    // of `content`, not the start.
    const mockElement = createMockElementForView(viewDiv, content, { x: 0, y: 1000 });
    const spans = mockElement.querySelectorAll('[data-position]');
    const positions = Array.from(spans).map((s) => Number((s as HTMLElement).dataset.position));

    expect(spans.length).toBe(MAX_MOCK_ELEMENT_CHARS);
    // Windowed at the true end of content, not clamped to the first 4000 chars.
    expect(Math.min(...positions)).toBe(content.length - MAX_MOCK_ELEMENT_CHARS);
    expect(Math.max(...positions)).toBe(content.length - 1);

    mockElement.remove();
    viewDiv.remove();
  });

  it('windows the capped fallback around a middle click point', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);
    const content = 'a'.repeat(50_000);

    vi.spyOn(viewDiv, 'getBoundingClientRect').mockReturnValue({
      left: 0,
      top: 0,
      right: 500,
      bottom: 1000,
      width: 500,
      height: 1000,
      x: 0,
      y: 0,
      toJSON: () => ({})
    } as DOMRect);

    // A click at the vertical midpoint should window around roughly the
    // midpoint of `content`, not the start.
    const mockElement = createMockElementForView(viewDiv, content, { x: 0, y: 500 });
    const spans = mockElement.querySelectorAll('[data-position]');
    const positions = Array.from(spans).map((s) => Number((s as HTMLElement).dataset.position));
    const windowStart = Math.min(...positions);

    expect(spans.length).toBe(MAX_MOCK_ELEMENT_CHARS);
    // Roughly centered on content.length / 2 = 25,000 (within one window width).
    expect(windowStart).toBeGreaterThan(0);
    expect(windowStart).toBeLessThan(content.length - MAX_MOCK_ELEMENT_CHARS);
    expect(Math.abs(windowStart + MAX_MOCK_ELEMENT_CHARS / 2 - content.length / 2)).toBeLessThan(
      MAX_MOCK_ELEMENT_CHARS
    );

    mockElement.remove();
    viewDiv.remove();
  });

  it('without a click point, windows from the start (unchanged existing behavior)', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);
    const content = 'a'.repeat(50_000);

    const mockElement = createMockElementForView(viewDiv, content);
    const spans = mockElement.querySelectorAll('[data-position]');
    const positions = Array.from(spans).map((s) => Number((s as HTMLElement).dataset.position));

    expect(Math.min(...positions)).toBe(0);
    expect(Math.max(...positions)).toBe(MAX_MOCK_ELEMENT_CHARS - 1);

    mockElement.remove();
    viewDiv.remove();
  });

  it('caps span creation for long multi-line content too', () => {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);

    // 10,000 short lines - well past the cap once flattened to characters.
    const longContent = Array.from({ length: 10_000 }, (_, i) => `line ${i}`).join('\n');
    const createElementSpy = vi.spyOn(document, 'createElement');

    const mockElement = createMockElementForView(viewDiv, longContent);
    const spanCreations = createElementSpy.mock.calls.filter(([tag]) => tag === 'span').length;
    const brCreations = createElementSpy.mock.calls.filter(([tag]) => tag === 'br').length;

    // Span count is bounded exactly by the cap (one span per character of the
    // capped content, including newline placeholders); each newline within
    // that window also gets a <br>, so total elements can be up to ~2x the
    // cap in the worst case — still a constant bound, not one element per
    // character of the full (potentially unbounded) content.
    expect(spanCreations).toBe(MAX_MOCK_ELEMENT_CHARS);
    expect(brCreations).toBeLessThanOrEqual(MAX_MOCK_ELEMENT_CHARS);
    expect(spanCreations + brCreations).toBeLessThan(longContent.length);

    mockElement.remove();
    viewDiv.remove();
    createElementSpy.mockRestore();
  });
});

describe('old vs new click-to-caret positioning agree on the same target character', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    uninstallCaretRangeFromPoint();
    document.body.innerHTML = '';
  });

  function oldApproachIndex(content: string, targetIndex: number): number {
    const viewDiv = document.createElement('div');
    document.body.appendChild(viewDiv);
    const geometrySpy = stubMockElementGeometry(content);

    const mockElement = createMockElementForView(viewDiv, content);
    const mockRect = mockElement.getBoundingClientRect();
    const { x, y } = clickPointFor(content, targetIndex);

    const result = findCharacterFromClick(mockElement, x, y, {
      left: mockRect.left,
      top: mockRect.top,
      width: mockRect.width,
      height: mockRect.height
    });

    mockElement.remove();
    viewDiv.remove();
    geometrySpy.mockRestore();
    return result.index;
  }

  function newApproachIndex(content: string, targetIndex: number): number | null {
    const view = buildViewElement(content);
    const [container, offset] = domPositionFor(view, content, targetIndex);
    stubCaretRangeFromPoint(container, offset);

    const index = findViewOffsetFromClick(view, 0, 0);

    view.remove();
    uninstallCaretRangeFromPoint();
    return index;
  }

  it('agree at the start of single-line content', () => {
    const content = 'Hello world';
    expect(newApproachIndex(content, 0)).toBe(oldApproachIndex(content, 0));
  });

  it('agree in the middle of single-line content', () => {
    const content = 'Hello world';
    expect(newApproachIndex(content, 6)).toBe(oldApproachIndex(content, 6));
  });

  it('agree at the end of single-line content', () => {
    const content = 'Hello world';
    expect(newApproachIndex(content, content.length)).toBe(
      oldApproachIndex(content, content.length)
    );
  });

  it('agree on a later line of multi-line content', () => {
    const content = 'Line 1\nLine 2\nLine 3';
    const targetIndex = 14; // 'L' of "Line 3"
    expect(newApproachIndex(content, targetIndex)).toBe(oldApproachIndex(content, targetIndex));
  });
});
