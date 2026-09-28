/**
 * Rendered-text offset mapping utilities.
 *
 * A node's view DOM (rendered markdown: text nodes, `<br>`, and formatting
 * elements like `<strong>`/`<em>`/`<a>`) doesn't carry character offsets
 * directly. These helpers convert between a DOM `(container, offset)`
 * position and a flat character index into the "rendered text" — text node
 * content concatenated in document order, with one `\n` per `<br>` — which is
 * exactly what the user sees once markdown syntax is stripped for view-mode
 * rendering.
 *
 * Shared by cross-node copy (mapping a browser Selection's DOM range to
 * source offsets) and click-to-edit-at-position (mapping a native caret
 * hit-test's DOM position to a source offset).
 */

/** Rendered text of a view element: text node content + `\n` per `<br>`. */
export function extractRenderedText(element: HTMLElement): string {
  let text = '';
  const walk = (node: Node): void => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += node.textContent ?? '';
    } else if (node.nodeName === 'BR') {
      text += '\n';
    } else {
      node.childNodes.forEach(walk);
    }
  };
  walk(element);
  return text;
}

/** Total rendered length of a subtree: text length, +1 per `<br>`. */
export function renderedLength(node: Node): number {
  if (node.nodeType === Node.TEXT_NODE) {
    return (node.textContent ?? '').length;
  }
  if (node.nodeName === 'BR') {
    return 1;
  }
  let total = 0;
  node.childNodes.forEach((child) => {
    total += renderedLength(child);
  });
  return total;
}

/**
 * Rendered offset (chars, +1 per `<br>`) from the start of `viewEl` to the DOM
 * position `(container, offset)`, mirroring `extractRenderedText`. Returns
 * null when the position isn't inside `viewEl` (caller decides the fallback).
 */
export function renderedOffsetTo(
  viewEl: HTMLElement,
  container: Node,
  offset: number
): number | null {
  if (!viewEl.contains(container) && container !== viewEl) {
    return null;
  }

  let count = 0;
  let done = false;

  const walk = (node: Node): void => {
    if (done) return;

    // Element container: `offset` is an index into its child nodes.
    if (node === container && node.nodeType !== Node.TEXT_NODE) {
      const children = Array.from(node.childNodes);
      for (let i = 0; i < offset && i < children.length; i++) {
        count += renderedLength(children[i]);
      }
      done = true;
      return;
    }

    if (node.nodeType === Node.TEXT_NODE) {
      if (node === container) {
        count += Math.min(offset, (node.textContent ?? '').length);
        done = true;
        return;
      }
      count += (node.textContent ?? '').length;
      return;
    }

    if (node.nodeName === 'BR') {
      count += 1;
      return;
    }

    node.childNodes.forEach(walk);
  };

  walk(viewEl);
  return count;
}
