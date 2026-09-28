/**
 * Cursor Positioning Utilities for ContentEditable Elements
 *
 * Character-level coordinate mapping for precise cursor positioning.
 * Uses mock element with character spans for accurate measurements.
 * Optimized for cross-browser compatibility and < 50ms performance.
 */

import { createLogger } from '$lib/utils/logger';
import { renderedOffsetTo } from '$lib/utils/rendered-text-offset';

const log = createLogger('CursorPositioning');

export interface PositionResult {
  index: number;
  distance: number;
  accuracy: 'exact' | 'approximate';
}

/**
 * Performance-optimized position finding with early exit conditions
 * @param mockElement - The hidden mock element with character spans
 * @param clickX - Click X coordinate relative to the page
 * @param clickY - Click Y coordinate relative to the page
 * @param editableRect - Bounding rectangle of the target editable element
 * @returns Character index optimized for < 50ms performance
 */
export function findCharacterFromClick(
  mockElement: HTMLDivElement,
  clickX: number,
  clickY: number,
  editableRect: { left: number; top: number; width: number; height: number }
): PositionResult {
  const startTime = performance.now();

  const relativeX = clickX - editableRect.left;
  const relativeY = clickY - editableRect.top;

  let bestMatch: PositionResult = { index: 0, distance: Infinity, accuracy: 'approximate' };
  let bestMatchSpan: HTMLElement | null = null;

  const allSpans = mockElement.querySelectorAll('[data-position]');

  if (allSpans.length === 0) {
    const duration = performance.now() - startTime;
    performanceMonitor.recordMeasurement(duration);
    return bestMatch;
  }

  const mockRect = mockElement.getBoundingClientRect();

  // Binary search approach for large content optimization
  let left = 0;
  let right = allSpans.length - 1;
  let candidates: HTMLElement[] = [];

  // If content is short, check all spans
  if (allSpans.length < 100) {
    candidates = Array.from(allSpans) as HTMLElement[];
  } else {
    // For longer content, use binary search to narrow down candidates
    while (left <= right && candidates.length < 50) {
      const mid = Math.floor((left + right) / 2);
      const span = allSpans[mid];
      const rect = span.getBoundingClientRect();
      const spanY = rect.top - mockRect.top;

      candidates.push(span as HTMLElement);

      if (spanY < relativeY) {
        left = mid + 1;
      } else if (spanY > relativeY) {
        right = mid - 1;
      } else {
        // Found same line, expand search around this position
        const expansion = Math.min(20, allSpans.length - 1);
        const start = Math.max(0, mid - expansion);
        const end = Math.min(allSpans.length - 1, mid + expansion);

        for (let i = start; i <= end; i++) {
          if (i !== mid) candidates.push(allSpans[i] as HTMLElement);
        }
        break;
      }
    }
  }

  // Check candidates for best match
  for (const span of candidates) {
    const rect = span.getBoundingClientRect();
    const spanCenterX = rect.left - mockRect.left + rect.width / 2;
    const spanCenterY = rect.top - mockRect.top + rect.height / 2;

    const distance = Math.sqrt(
      Math.pow(spanCenterX - relativeX, 2) + Math.pow(spanCenterY - relativeY, 2)
    );

    if (distance < bestMatch.distance) {
      const position = parseInt(span.dataset.position || '0');
      bestMatch = {
        index: position,
        distance,
        accuracy: distance < 5 ? 'exact' : 'approximate'
      };
      bestMatchSpan = span;
    }

    // Early exit if we find a very close match
    if (bestMatch.accuracy === 'exact' && bestMatch.distance < 2) {
      break;
    }
  }

  // Determine if cursor should go before or after the matched character
  // based on whether click is to the left or right of the character's center
  if (bestMatchSpan) {
    const bestRect = bestMatchSpan.getBoundingClientRect();
    const bestCenterX = bestRect.left - mockRect.left + bestRect.width / 2;
    const maxIndex = mockElement.textContent?.length ?? 0;

    // If click is to the right of the character's center, position cursor after it
    // but ensure we don't exceed content bounds
    if (relativeX > bestCenterX && bestMatch.index < maxIndex) {
      bestMatch.index += 1;
    }
  }

  const duration = performance.now() - startTime;
  performanceMonitor.recordMeasurement(duration);

  // Use performance monitor for consistent logging
  if (performanceMonitor.shouldWarnAboutPerformance()) {
    const stats = performanceMonitor.getStats();
    log.warn(
      `Cursor positioning performance degrading: avg=${stats.average.toFixed(2)}ms, recent=${stats.recent.toFixed(2)}ms, max=${stats.max.toFixed(2)}ms`
    );
  }

  return bestMatch;
}

/**
 * Handle complex text scenarios (emojis, RTL, etc.) for ContentEditable
 * @param content - Text content to analyze
 * @returns Information about text complexity
 */
export function analyzeTextComplexity(content: string): {
  hasEmojis: boolean;
  hasRTL: boolean;
  hasComplexGraphemes: boolean;
  estimatedComplexity: 'low' | 'medium' | 'high';
} {
  const emojiRegex =
    /[\u{1F600}-\u{1F64F}]|[\u{1F300}-\u{1F5FF}]|[\u{1F680}-\u{1F6FF}]|[\u{1F1E0}-\u{1F1FF}]|[\u{2600}-\u{26FF}]|[\u{2700}-\u{27BF}]/gu;
  const rtlRegex = /[\u0590-\u05FF\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF]/;
  const complexGraphemeRegex =
    /[\u0300-\u036F]|[\u1AB0-\u1AFF]|[\u1DC0-\u1DFF]|[\u20D0-\u20FF]|[\uFE20-\uFE2F]/;

  const hasEmojis = emojiRegex.test(content);
  const hasRTL = rtlRegex.test(content);
  const hasComplexGraphemes = complexGraphemeRegex.test(content);

  let estimatedComplexity: 'low' | 'medium' | 'high' = 'low';

  if (hasEmojis || hasComplexGraphemes) {
    estimatedComplexity = hasRTL ? 'high' : 'medium';
  } else if (hasRTL) {
    estimatedComplexity = 'medium';
  }

  return {
    hasEmojis,
    hasRTL,
    hasComplexGraphemes,
    estimatedComplexity
  };
}

/**
 * Performance monitoring utility for cursor positioning operations
 */
export class PositioningPerformanceMonitor {
  private measurements: number[] = [];
  private readonly maxMeasurements = 100;

  recordMeasurement(duration: number) {
    this.measurements.push(duration);

    // Keep only recent measurements
    if (this.measurements.length > this.maxMeasurements) {
      this.measurements.shift();
    }
  }

  getStats() {
    if (this.measurements.length === 0) {
      return { average: 0, max: 0, min: 0, recent: 0 };
    }

    const sum = this.measurements.reduce((a, b) => a + b, 0);
    const average = sum / this.measurements.length;
    const max = Math.max(...this.measurements);
    const min = Math.min(...this.measurements);
    const recent = this.measurements[this.measurements.length - 1];

    return { average, max, min, recent };
  }

  shouldWarnAboutPerformance(): boolean {
    const stats = this.getStats();
    return stats.average > 30 || stats.recent > 50;
  }
}

// Global performance monitor instance
export const performanceMonitor = new PositioningPerformanceMonitor();

/**
 * Find the rendered-text character offset for a click on a node's view
 * element, using the browser's native caret hit-testing (`caretRangeFromPoint`)
 * directly against the already-rendered live element — no DOM construction,
 * so cost is independent of content length.
 *
 * @param viewElement - The live, already-rendered view div (not a mock)
 * @param clientX - Click X in viewport coordinates (`event.clientX`)
 * @param clientY - Click Y in viewport coordinates (`event.clientY`)
 * @returns The rendered-text character offset (matching `extractTextWithLineBreaks`
 *   / `renderedOffsetTo` counting: text length, +1 per `<br>`), or null when
 *   native caret hit-testing isn't available in this environment (e.g.
 *   Happy-DOM) or the hit point falls outside `viewElement`. Callers should
 *   fall back to `createMockElementForView` + `findCharacterFromClick` in
 *   that case.
 */
export function findViewOffsetFromClick(
  viewElement: HTMLElement,
  clientX: number,
  clientY: number
): number | null {
  const startTime = performance.now();
  const doc = viewElement.ownerDocument;

  if (typeof doc?.caretRangeFromPoint !== 'function') {
    return null;
  }

  let range: Range | null = null;
  try {
    range = doc.caretRangeFromPoint(clientX, clientY);
  } catch (e) {
    log.warn('caretRangeFromPoint threw; falling back to mock-element positioning', e);
    return null;
  }

  if (!range) {
    return null;
  }

  const offset = renderedOffsetTo(viewElement, range.startContainer, range.startOffset);

  const duration = performance.now() - startTime;
  performanceMonitor.recordMeasurement(duration);

  return offset;
}

/**
 * Hard cap on how many character spans `createMockElementForView` will ever
 * build. The native path (`findViewOffsetFromClick`) never pays this cost —
 * this only bounds the degraded fallback used when native caret hit-testing
 * is unavailable, trading exact positioning beyond the cap for a bounded,
 * predictable cost instead of one span per character of arbitrarily long
 * content.
 */
export const MAX_MOCK_ELEMENT_CHARS = 4000;

/**
 * Create temporary mock element with character spans for view div
 * Mirrors exact rendering from view mode for accurate click positioning
 *
 * IMPORTANT: Caller must call mockElement.remove() after use!
 *
 * @param viewElement - The view div element to mirror
 * @param content - The view content to wrap in character spans
 * @returns Mock div element with character spans (caller must remove!)
 */
export function createMockElementForView(
  viewElement: HTMLDivElement,
  content: string
): HTMLDivElement {
  const mockElement = document.createElement('div');

  // Position mock element at same location as view element for accurate coordinate matching
  const viewRect = viewElement.getBoundingClientRect();
  let cssText = `
    position: absolute;
    visibility: hidden;
    top: ${viewRect.top}px;
    left: ${viewRect.left}px;
    width: ${viewRect.width}px;
    white-space: pre-wrap;
    word-wrap: break-word;
  `;

  // Feature detection - getComputedStyle may not be available in test environments
  try {
    if (typeof window !== 'undefined' && window.getComputedStyle) {
      const computedStyle = window.getComputedStyle(viewElement);
      cssText += `
        font-family: ${computedStyle.fontFamily};
        font-size: ${computedStyle.fontSize};
        font-weight: ${computedStyle.fontWeight};
        line-height: ${computedStyle.lineHeight};
        letter-spacing: ${computedStyle.letterSpacing};
        padding: ${computedStyle.padding};
      `;
    }
  } catch (e) {
    // Gracefully handle errors in test environments
    log.warn('Could not copy computed styles for mock element', e);
  }

  mockElement.style.cssText = cssText;

  // Wrap each character in span with data-position attribute
  // This allows findCharacterFromClick to map coordinates → position
  // Capped: this fallback only runs when native caret hit-testing is
  // unavailable, so beyond the cap positioning is approximate rather than
  // building one span per character of arbitrarily long content.
  if (content.length > MAX_MOCK_ELEMENT_CHARS) {
    log.warn(
      `createMockElementForView: content length ${content.length} exceeds cap ${MAX_MOCK_ELEMENT_CHARS}; positioning beyond the cap will be approximate`
    );
  }
  const cappedContent = content.slice(0, MAX_MOCK_ELEMENT_CHARS);

  cappedContent.split('').forEach((char, index) => {
    if (char === '\n') {
      // Handle newlines: add span + <br> (matches view rendering)
      const span = document.createElement('span');
      span.dataset.position = String(index);
      span.textContent = '0'; // Placeholder character for height calculation
      mockElement.appendChild(span);
      mockElement.appendChild(document.createElement('br'));
    } else {
      // Regular character
      const span = document.createElement('span');
      span.dataset.position = String(index);
      span.textContent = char;
      mockElement.appendChild(span);
    }
  });

  // Append to body for rendering (required for getBoundingClientRect)
  // Error handling for edge cases where document.body might not be available
  if (typeof document === 'undefined' || !document.body) {
    throw new Error(
      'Cannot create mock element for cursor positioning: document.body not available'
    );
  }
  document.body.appendChild(mockElement);
  return mockElement;
}

/**
 * Validate if click is within reasonable bounds of the text area
 * @param clickX - Click X coordinate
 * @param clickY - Click Y coordinate
 * @param editableRect - Bounding rectangle of editable element (textarea or contenteditable)
 * @returns True if click is within extended bounds
 */
export function isClickWithinTextBounds(
  clickX: number,
  clickY: number,
  editableRect: { left: number; top: number; width: number; height: number }
): boolean {
  // Allow some padding around the editable element for better UX
  const padding = 10;

  return (
    clickX >= editableRect.left - padding &&
    clickX <= editableRect.left + editableRect.width + padding &&
    clickY >= editableRect.top - padding &&
    clickY <= editableRect.top + editableRect.height + padding
  );
}
