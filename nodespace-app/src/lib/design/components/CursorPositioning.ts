/**
 * Cursor Positioning Utilities for ContentEditable Elements
 *
 * Character-level coordinate mapping for precise cursor positioning.
 * Uses mock element with character spans for accurate measurements.
 * Optimized for cross-browser compatibility and < 50ms performance.
 */

export interface PositionResult {
  index: number;
  distance: number;
  accuracy: 'exact' | 'approximate';
}

/**
 * Find the character index closest to the clicked coordinates
 * @param mockElement - The hidden mock element with character spans
 * @param clickX - Click X coordinate relative to the page
 * @param clickY - Click Y coordinate relative to the page
 * @param editableRect - Bounding rectangle of the target editable element
 * @returns Character index and distance information
 */
export function findCharacterFromClick(
  mockElement: HTMLDivElement,
  clickX: number,
  clickY: number,
  editableRect: { left: number; top: number; width: number; height: number }
): PositionResult {
  // Convert click coordinates to be relative to the mock element's container
  const relativeX = clickX - editableRect.left;
  const relativeY = clickY - editableRect.top;

  console.log('CursorPositioning Debug:', {
    clickX,
    clickY,
    editableRect: {
      left: editableRect.left,
      top: editableRect.top,
      width: editableRect.width,
      height: editableRect.height
    },
    relativeX,
    relativeY
  });

  let bestMatch: PositionResult = { index: 0, distance: Infinity, accuracy: 'approximate' };

  // Get all character spans from the mock element
  const allSpans = mockElement.querySelectorAll('[data-position]');

  if (allSpans.length === 0) {
    console.warn('No character spans found in mock element');
    return { index: 0, distance: 0, accuracy: 'approximate' };
  }

  // Get the mock element's position for relative calculations
  const mockRect = mockElement.getBoundingClientRect();

  for (let i = 0; i < allSpans.length; i++) {
    const span = allSpans[i] as HTMLElement;
    const rect = span.getBoundingClientRect();

    // Calculate position relative to the mock element
    const spanX = rect.left - mockRect.left;
    const spanY = rect.top - mockRect.top;

    // For better accuracy, use the center of each character
    const spanCenterX = spanX + rect.width / 2;
    const spanCenterY = spanY + rect.height / 2;

    // Calculate distance using Euclidean distance
    const distance = Math.sqrt(
      Math.pow(spanCenterX - relativeX, 2) + Math.pow(spanCenterY - relativeY, 2)
    );

    if (distance < bestMatch.distance) {
      const position = parseInt(span.dataset.position || '0');
      bestMatch = {
        index: position,
        distance,
        accuracy: distance < 5 ? 'exact' : 'approximate' // Within 5px is considered exact
      };
    }
  }

  console.log('Best position match:', bestMatch);

  return bestMatch;
}

/**
 * Enhanced positioning for multi-line text with line-aware logic
 * @param mockElement - The hidden mock element with character spans
 * @param clickX - Click X coordinate relative to the page
 * @param clickY - Click Y coordinate relative to the page
 * @param editableRect - Bounding rectangle of the target editable element
 * @param content - The text content for line-based calculations
 * @returns Character index with multi-line optimization
 */
export function findCharacterFromClickMultiline(
  mockElement: HTMLDivElement,
  clickX: number,
  clickY: number,
  editableRect: { left: number; top: number; width: number; height: number },
  _content: string
): PositionResult {
  const relativeX = clickX - editableRect.left;
  const relativeY = clickY - editableRect.top;

  const allSpans = mockElement.querySelectorAll('[data-position]');
  if (allSpans.length === 0) {
    return { index: 0, distance: 0, accuracy: 'approximate' };
  }

  // Group spans by their Y position to identify lines
  const spansByLine = new Map<number, HTMLElement[]>();
  const mockRect = mockElement.getBoundingClientRect();

  for (let i = 0; i < allSpans.length; i++) {
    const span = allSpans[i] as HTMLElement;
    const rect = span.getBoundingClientRect();
    const spanY = Math.round(rect.top - mockRect.top);

    if (!spansByLine.has(spanY)) {
      spansByLine.set(spanY, []);
    }
    spansByLine.get(spanY)!.push(span);
  }

  // Sort lines by Y position
  const sortedLines = Array.from(spansByLine.entries()).sort((a, b) => a[0] - b[0]);

  // Find the line closest to the click Y position
  let closestLineY = sortedLines[0][0];
  let closestLineDistance = Math.abs(relativeY - closestLineY);

  for (const [lineY] of sortedLines) {
    const distance = Math.abs(relativeY - lineY);
    if (distance < closestLineDistance) {
      closestLineDistance = distance;
      closestLineY = lineY;
    }
  }

  // Find best character within the target line
  const targetLineSpans = spansByLine.get(closestLineY) || [];
  let bestMatch: PositionResult = { index: 0, distance: Infinity, accuracy: 'approximate' };

  for (const span of targetLineSpans) {
    const rect = span.getBoundingClientRect();
    const spanX = rect.left - mockRect.left + rect.width / 2; // Use center of character

    const distance = Math.abs(spanX - relativeX);

    if (distance < bestMatch.distance) {
      const position = parseInt(span.dataset.position || '0');
      bestMatch = {
        index: position,
        distance,
        accuracy: distance < 3 ? 'exact' : 'approximate'
      };
    }
  }

  return bestMatch;
}

/**
 * Performance-optimized position finding with early exit conditions
 * @param mockElement - The hidden mock element with character spans
 * @param clickX - Click X coordinate relative to the page
 * @param clickY - Click Y coordinate relative to the page
 * @param editableRect - Bounding rectangle of the target editable element
 * @returns Character index optimized for < 50ms performance
 */
export function findCharacterFromClickFast(
  mockElement: HTMLDivElement,
  clickX: number,
  clickY: number,
  editableRect: { left: number; top: number; width: number; height: number }
): PositionResult {
  const startTime = performance.now();

  const relativeX = clickX - editableRect.left;
  const relativeY = clickY - editableRect.top;

  let bestMatch: PositionResult = { index: 0, distance: Infinity, accuracy: 'approximate' };

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
    const spanX = rect.left - mockRect.left + rect.width / 2;
    const spanY = rect.top - mockRect.top + rect.height / 2;

    const distance = Math.sqrt(Math.pow(spanX - relativeX, 2) + Math.pow(spanY - relativeY, 2));

    if (distance < bestMatch.distance) {
      const position = parseInt(span.dataset.position || '0');
      bestMatch = {
        index: position,
        distance,
        accuracy: distance < 5 ? 'exact' : 'approximate'
      };
    }

    // Early exit if we find a very close match
    if (bestMatch.accuracy === 'exact' && bestMatch.distance < 2) {
      break;
    }
  }

  const duration = performance.now() - startTime;
  performanceMonitor.recordMeasurement(duration);

  if (duration > 50) {
    console.warn(`Cursor positioning took ${duration}ms, exceeding 50ms target`);
  }

  console.log(`Cursor positioning completed in ${duration}ms`, bestMatch);

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
