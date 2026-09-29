/**
 * Caret geometry for a textarea, including soft-wrapped lines.
 *
 * A textarea exposes no client rects for its caret, and its value only
 * records hard line breaks (`\n`), not where the text wraps. To find which
 * visual row a caret position sits on, and its horizontal offset there, this
 * lays the text out in a hidden mirror element with the textarea's own
 * width, padding, font and wrapping rules, and measures where the text from
 * the position onward begins.
 */

/** Caret location in the textarea's content box, in CSS pixels. */
export interface CaretPoint {
  /** Top of the caret's visual row. Equal tops mean the same row. */
  top: number;
  /** Horizontal offset from the start of the row's text (excludes padding). */
  left: number;
}

/** Style properties that affect how the textarea lays out and wraps text. */
const MIRRORED_PROPERTIES = [
  'paddingTop',
  'paddingRight',
  'paddingBottom',
  'paddingLeft',
  'fontFamily',
  'fontSize',
  'fontStyle',
  'fontWeight',
  'fontVariant',
  'fontStretch',
  'fontFeatureSettings',
  'letterSpacing',
  'wordSpacing',
  'lineHeight',
  'textIndent',
  'textTransform',
  'tabSize',
  'whiteSpace',
  'wordBreak',
  'overflowWrap'
] as const;

/**
 * A hidden layout copy of one textarea, reusable for several measurements
 * of the same value. Call `dispose()` when done.
 */
export class TextareaCaretMirror {
  private readonly mirror: HTMLDivElement;
  private readonly paddingLeft: number;
  private readonly paddingTop: number;
  private readonly value: string;

  private constructor(
    textarea: HTMLTextAreaElement,
    style: ReturnType<typeof window.getComputedStyle>
  ) {
    this.value = textarea.value;
    this.paddingLeft = parseFloat(style.paddingLeft) || 0;
    this.paddingTop = parseFloat(style.paddingTop) || 0;

    this.mirror = document.createElement('div');
    const mirrorStyle = this.mirror.style;
    for (const property of MIRRORED_PROPERTIES) {
      mirrorStyle[property] = style[property];
    }
    // The textarea's text wraps within its padding box: its width minus
    // borders and any vertical scrollbar. offsetWidth - clientWidth is
    // exactly that chrome; the unrounded rect width keeps a fractional
    // layout width, where a word that just fits must still fit.
    const chrome = textarea.offsetWidth - textarea.clientWidth;
    mirrorStyle.boxSizing = 'border-box';
    mirrorStyle.width = `${textarea.getBoundingClientRect().width - chrome}px`;
    mirrorStyle.border = '0';
    mirrorStyle.position = 'absolute';
    mirrorStyle.visibility = 'hidden';
    mirrorStyle.top = '0';
    mirrorStyle.left = '-9999px';
    mirrorStyle.overflow = 'hidden';
    document.body.appendChild(this.mirror);
  }

  /**
   * Mirror `textarea`, or return null when it has no layout to measure
   * (not rendered, or an environment without layout such as Happy-DOM).
   */
  static create(textarea: HTMLTextAreaElement): TextareaCaretMirror | null {
    if (typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') {
      return null;
    }
    if (!textarea.isConnected || textarea.clientWidth === 0) return null;

    const mirror = new TextareaCaretMirror(textarea, window.getComputedStyle(textarea));
    if (mirror.pointAt(0) === null) {
      mirror.dispose();
      return null;
    }
    return mirror;
  }

  /**
   * Caret location for `position` in the mirrored value, or null when the
   * mirror produced no layout.
   *
   * The whole value is laid out, with the text from `position` onward in a
   * span whose first fragment starts where the caret sits. Laying out only
   * the text before the caret would be wrong at every row start: the word
   * the textarea wraps whole would be cut at the caret, and its prefix would
   * still fit on the row above.
   */
  pointAt(position: number): CaretPoint | null {
    const clamped = Math.max(0, Math.min(position, this.value.length));
    this.mirror.textContent = this.value.slice(0, clamped);
    const marker = document.createElement('span');
    // At the end of the value, a placeholder gives the span a line box.
    marker.textContent = this.value.slice(clamped) || '.';
    this.mirror.appendChild(marker);
    if (marker.offsetHeight === 0) return null;
    return {
      top: marker.offsetTop - this.paddingTop,
      left: marker.offsetLeft - this.paddingLeft
    };
  }

  dispose(): void {
    this.mirror.remove();
  }
}

/** Whether two caret points are on the same visual row. */
export function isSameRow(a: CaretPoint, b: CaretPoint): boolean {
  return Math.abs(a.top - b.top) < 1;
}
