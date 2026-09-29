/**
 * Caret geometry for a textarea, including soft-wrapped lines.
 *
 * A textarea exposes no client rects for its caret, and its value only
 * records hard line breaks (`\n`), not where the text wraps. To find which
 * visual row a caret position sits on, and its horizontal offset there, this
 * lays the text out in a hidden mirror element with the textarea's own
 * width, padding, font and wrapping rules, and measures a zero-width marker
 * placed at the position.
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

  private constructor(textarea: HTMLTextAreaElement, style: ReturnType<typeof window.getComputedStyle>) {
    this.value = textarea.value;
    this.paddingLeft = parseFloat(style.paddingLeft) || 0;
    this.paddingTop = parseFloat(style.paddingTop) || 0;

    this.mirror = document.createElement('div');
    const mirrorStyle = this.mirror.style;
    for (const property of MIRRORED_PROPERTIES) {
      mirrorStyle[property] = style[property];
    }
    // clientWidth is the textarea's padding box: its width minus borders and
    // any vertical scrollbar, which is the width its text wraps within.
    mirrorStyle.boxSizing = 'border-box';
    mirrorStyle.width = `${textarea.clientWidth}px`;
    mirrorStyle.border = '0';
    mirrorStyle.position = 'absolute';
    mirrorStyle.visibility = 'hidden';
    mirrorStyle.top = '0';
    mirrorStyle.left = '-9999px';
    mirrorStyle.overflow = 'hidden';
    // A textarea always wraps (unless wrap="off"), whatever its computed
    // white-space reports.
    if (textarea.wrap !== 'off') mirrorStyle.whiteSpace = 'pre-wrap';
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

  /** Caret location for `position` in the mirrored value, or null when the
   * mirror produced no layout. */
  pointAt(position: number): CaretPoint | null {
    const clamped = Math.max(0, Math.min(position, this.value.length));
    this.mirror.textContent = this.value.slice(0, clamped);
    const marker = document.createElement('span');
    // Zero-width space: gives the marker a line box without taking width.
    marker.textContent = '​';
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
