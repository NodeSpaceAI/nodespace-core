// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { RelativeDateAnchor } from './relative-date-anchor';

/**
 * A date named relative to the day the query runs, in place of a fixed
 * [`QueryFilter::value`]: `{"anchor": "today"}` is today, and
 * `{"anchor": "today", "offset_days": -7}` a week ago (ADR-091).
 *
 * It is stored as written and resolved each time the query runs, so a saved
 * "due this week" stays true next week.
 */
export type RelativeDate = {
  anchor: RelativeDateAnchor;
  /**
   * Days after the anchor; negative for days before it. Absent is the
   * anchor day itself.
   */
  offset_days?: number;
};
