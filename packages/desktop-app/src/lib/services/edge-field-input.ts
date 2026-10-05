/**
 * Relationship EDGE-FIELD input helpers (pure logic).
 *
 * A typed relationship's `edge_fields` are declared per relationship, so an
 * editor renders whatever that relationship happens to declare. These helpers
 * map a declared field's `type` onto the input to render and marshal values in
 * and out of it. Shared by the relationship panel's add form and its per-row
 * edge-property editor so both render a given field type identically.
 *
 * Kept free of Svelte/DOM imports so it is unit-testable in isolation (see
 * `src/tests/unit/edge-field-input.test.ts`).
 */

import type { RawEdgeField } from './relationship-grouping';

export type EdgeInputKind = 'number' | 'boolean' | 'date' | 'datetime' | 'enum' | 'text';

/** Which kind of input a declared edge field should render as. */
export function edgeInputKind(field: RawEdgeField): EdgeInputKind {
  switch (field.type) {
    case 'number':
      return 'number';
    case 'boolean':
      return 'boolean';
    case 'date':
      return 'date';
    case 'datetime':
      // A whole-day `date` input would silently drop the time component.
      return 'datetime';
    case 'enum':
      // A declared value set renders as a picker. Falling back to free text
      // when it is somehow absent keeps an existing edge editable rather
      // than presenting an empty dropdown with no way out; the backend
      // requires coreValues on an enum declaration, so this is defensive.
      return (field.coreValues?.length ?? 0) > 0 ? 'enum' : 'text';
    case 'text':
    case 'array':
    case 'object':
    case 'link':
      // A link is never declared on an edge: the schema write refuses it.
      return 'text';
  }
}

/** Native input `type` for a text-like edge-field kind. */
export function edgeInputType(kind: EdgeInputKind): 'date' | 'datetime-local' | 'text' {
  if (kind === 'date') return 'date';
  if (kind === 'datetime') return 'datetime-local';
  return 'text';
}

/** Parse a numeric input's raw string; blank and unparseable both mean "no value". */
export function coerceNumber(raw: string): number | null {
  if (raw.trim() === '') return null;
  const n = Number(raw);
  return Number.isNaN(n) ? null : n;
}

export function toInputString(value: unknown): string {
  if (value === null || value === undefined) return '';
  return String(value);
}

/**
 * Format a stored value for a `datetime-local` input (`YYYY-MM-DDTHH:mm`),
 * preserving the time a plain `date` input would drop. Accepts an ISO string
 * (with or without a trailing `Z`/offset) or anything `Date` can parse; returns
 * `''` for an unparseable/empty value. Values the input already yields (naive
 * local `YYYY-MM-DDTHH:mm`) pass straight back through, so a save→reload round
 * trip does not drift.
 */
export function toDateTimeLocalString(value: unknown): string {
  const raw = toInputString(value).trim();
  if (raw === '') return '';
  const isoish = raw.match(/^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2})/);
  if (isoish) return `${isoish[1]}T${isoish[2]}`;
  const parsed = new Date(raw);
  if (Number.isNaN(parsed.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${parsed.getFullYear()}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())}T${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
}

/** Value string for a text-like edge input, formatted for its `type`. */
export function edgeInputValue(kind: EdgeInputKind, value: unknown): string {
  return kind === 'datetime' ? toDateTimeLocalString(value) : toInputString(value);
}

/**
 * Humanize an edge-field name for its input's label (`due_by` → `due by`).
 *
 * Deliberately NOT `humanizeName` from `relationship-grouping.ts`, which Title
 * Cases every word. Callers here pair this with a `capitalize` CSS class, so the
 * result renders as `Due by` — sentence case, which is what a form label wants.
 * Routing this through `humanizeName` would yield `Due By`, capitalized twice
 * over. The near-duplication is the point; consolidating the two silently
 * changes how every edge-field label reads.
 */
export function formatEdgeFieldLabel(name: string): string {
  return name.replace(/[_-]+/g, ' ');
}
