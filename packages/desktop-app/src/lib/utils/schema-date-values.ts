/**
 * Shared date-field helpers: parsing a backend date string (YYYY-MM-DD or a
 * full ISO8601 timestamp) into a `DateValue`, formatting one for display, and
 * formatting a picked `DateValue` back into storage form. The date-time
 * helpers do the same for a `datetime` field, which stores one RFC 3339
 * instant and is shown and edited in the user's local time zone.
 *
 * Single source of truth so every surface that shows a date field's value (a
 * SchemaFieldLeaf date control, a collapsed-header summary, …) parses and
 * displays it identically instead of each keeping its own copy.
 */
import { CalendarDate, parseDate, type DateValue } from '@internationalized/date';
import { createLogger } from './logger';

const log = createLogger('SchemaDateValues');

/** Parse a backend date value (handles both YYYY-MM-DD and full ISO8601 strings). */
export function parseScalarDate(raw: string | null | undefined): DateValue | undefined {
  if (!raw) return undefined;
  try {
    // Extract just the date part (YYYY-MM-DD) if it's a full ISO8601 string
    const dateOnly = raw.includes('T') ? raw.split('T')[0] : raw;
    return parseDate(dateOnly);
  } catch (error) {
    log.warn(`Failed to parse date value "${raw}":`, error);
    return undefined;
  }
}

/** Human-readable display for a date field's current value, or a "Pick a date" placeholder. */
export function formatDateDisplay(raw: string | null | undefined): string {
  if (!raw) return 'Pick a date';
  const date = parseScalarDate(raw);
  return date ? date.toString() : raw;
}

/** Format a picked DateValue back into the YYYY-MM-DD storage form the backend expects. */
export function formatDateForStorage(dateValue: DateValue | undefined): string | null {
  if (!dateValue) return null;
  return `${dateValue.year}-${String(dateValue.month).padStart(2, '0')}-${String(dateValue.day).padStart(2, '0')}`;
}

/**
 * Parse a stored `datetime` value (an RFC 3339 date-time) into the instant it
 * names, or undefined when there is none to show.
 */
export function parseDateTime(raw: string | null | undefined): Date | undefined {
  if (!raw) return undefined;
  const instant = new Date(raw);
  if (Number.isNaN(instant.getTime())) {
    log.warn(`Failed to parse date-time value "${raw}"`);
    return undefined;
  }
  return instant;
}

/** The day a stored date-time falls on in the user's local time zone. */
export function dateTimeLocalDay(raw: string | null | undefined): DateValue | undefined {
  const instant = parseDateTime(raw);
  if (!instant) return undefined;
  return new CalendarDate(instant.getFullYear(), instant.getMonth() + 1, instant.getDate());
}

/** A stored date-time's local time of day as `HH:mm`, the form a time input takes; `''` when unset. */
export function dateTimeLocalTime(raw: string | null | undefined): string {
  const instant = parseDateTime(raw);
  if (!instant) return '';
  return `${pad2(instant.getHours())}:${pad2(instant.getMinutes())}`;
}

/** Human-readable local day of a date-time field's value, or a "Pick a date" placeholder. */
export function formatDateTimeDayDisplay(raw: string | null | undefined): string {
  if (!raw) return 'Pick a date';
  return dateTimeLocalDay(raw)?.toString() ?? raw;
}

/**
 * The stored date-time moved to the picked local day, keeping its time of day
 * (midnight when there was no value yet). No day picked clears the value.
 */
export function withLocalDay(raw: string | null | undefined, day: DateValue | undefined): string | null {
  if (!day) return null;
  const instant = parseDateTime(raw);
  return formatDateTimeForStorage(
    new Date(
      day.year,
      day.month - 1,
      day.day,
      instant?.getHours() ?? 0,
      instant?.getMinutes() ?? 0,
      instant?.getSeconds() ?? 0
    )
  );
}

/**
 * The stored date-time moved to the entered local time (`HH:mm`), keeping its
 * day (today when there was no value yet). The input holds hours and minutes,
 * so the result is on the whole minute. An incomplete time changes nothing,
 * reported as undefined, and neither does a stored value that can't be read:
 * its day is unknown, and today would replace it.
 */
export function withLocalTime(raw: string | null | undefined, time: string): string | undefined {
  const match = time.match(/^(\d{2}):(\d{2})/);
  if (!match) return undefined;
  const instant = raw ? parseDateTime(raw) : new Date();
  if (!instant) return undefined;
  return formatDateTimeForStorage(
    new Date(
      instant.getFullYear(),
      instant.getMonth(),
      instant.getDate(),
      Number(match[1]),
      Number(match[2])
    )
  );
}

/** Format an instant as the RFC 3339 date-time (UTC) a `datetime` field stores. */
export function formatDateTimeForStorage(instant: Date): string {
  return instant.toISOString();
}

function pad2(n: number): string {
  return String(n).padStart(2, '0');
}
