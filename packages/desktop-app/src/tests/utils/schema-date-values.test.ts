/**
 * Shared date-field helpers: extracted out of schema-field-leaf.svelte
 * so TaskSchemaForm's collapsed-header "Due: ..." text parses/formats a date
 * value identically to the SchemaFieldLeaf date control itself, instead of
 * keeping its own copy with divergent error handling.
 */
import { describe, it, expect, vi } from 'vitest';
import { CalendarDate } from '@internationalized/date';
import {
  parseScalarDate,
  formatDateDisplay,
  formatDateForStorage,
  parseDateTime,
  dateTimeLocalDay,
  dateTimeLocalTime,
  formatDateTimeDayDisplay,
  withLocalDay,
  withLocalTime,
  formatDateTimeForStorage
} from '$lib/utils/schema-date-values';

// Stored date-times are built from local-time parts so the expectations hold
// in whatever time zone the suite runs.
const stored = (...parts: [number, number, number, number, number, number?]) =>
  new Date(...parts).toISOString();
const RFC3339_UTC = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

describe('parseScalarDate', () => {
  it('returns undefined for null/undefined/empty', () => {
    expect(parseScalarDate(null)).toBeUndefined();
    expect(parseScalarDate(undefined)).toBeUndefined();
    expect(parseScalarDate('')).toBeUndefined();
  });

  it('parses a plain YYYY-MM-DD date', () => {
    const date = parseScalarDate('2026-12-31');
    expect(date?.toString()).toBe('2026-12-31');
  });

  it('extracts the date-only portion from a full ISO8601 timestamp', () => {
    const date = parseScalarDate('2026-12-31T10:00:00Z');
    expect(date?.toString()).toBe('2026-12-31');
  });

  it('returns undefined (not a throw) for an unparseable value', () => {
    expect(parseScalarDate('not-a-date')).toBeUndefined();
  });
});

describe('formatDateDisplay', () => {
  it('returns a placeholder for null/undefined', () => {
    expect(formatDateDisplay(null)).toBe('Pick a date');
    expect(formatDateDisplay(undefined)).toBe('Pick a date');
  });

  it('formats a valid date', () => {
    expect(formatDateDisplay('2026-01-05')).toBe('2026-01-05');
  });

  it('returns the raw value as-is when it fails to parse, rather than throwing', () => {
    expect(formatDateDisplay('garbage')).toBe('garbage');
  });
});

describe('formatDateForStorage', () => {
  it('returns null for undefined', () => {
    expect(formatDateForStorage(undefined)).toBeNull();
  });

  it('formats a DateValue to YYYY-MM-DD, zero-padded', () => {
    const date = parseScalarDate('2026-01-05');
    expect(formatDateForStorage(date)).toBe('2026-01-05');
  });
});

describe('parseDateTime', () => {
  it('returns undefined for null/undefined/empty', () => {
    expect(parseDateTime(null)).toBeUndefined();
    expect(parseDateTime(undefined)).toBeUndefined();
    expect(parseDateTime('')).toBeUndefined();
  });

  it('reads a UTC value and a value with an offset as the same instant', () => {
    expect(parseDateTime('2026-03-01T09:30:00Z')?.getTime()).toBe(Date.UTC(2026, 2, 1, 9, 30));
    expect(parseDateTime('2026-03-01T11:30:00+02:00')?.getTime()).toBe(Date.UTC(2026, 2, 1, 9, 30));
  });

  it('returns undefined (not a throw) for an unparseable value', () => {
    expect(parseDateTime('not-a-date-time')).toBeUndefined();
  });
});

describe('dateTimeLocalDay / dateTimeLocalTime', () => {
  it('show a stored value as its local day and local time of day', () => {
    const raw = stored(2026, 2, 1, 9, 5);
    expect(dateTimeLocalDay(raw)?.toString()).toBe('2026-03-01');
    expect(dateTimeLocalTime(raw)).toBe('09:05');
  });

  it('have nothing to show for a missing or unparseable value', () => {
    expect(dateTimeLocalDay(null)).toBeUndefined();
    expect(dateTimeLocalTime(null)).toBe('');
    expect(dateTimeLocalDay('garbage')).toBeUndefined();
    expect(dateTimeLocalTime('garbage')).toBe('');
  });
});

describe('formatDateTimeDayDisplay', () => {
  it('returns a placeholder for null/undefined', () => {
    expect(formatDateTimeDayDisplay(null)).toBe('Pick a date');
    expect(formatDateTimeDayDisplay(undefined)).toBe('Pick a date');
  });

  it('shows the local day of a stored value', () => {
    expect(formatDateTimeDayDisplay(stored(2026, 0, 5, 23, 59))).toBe('2026-01-05');
  });

  it('returns the raw value as-is when it fails to parse', () => {
    expect(formatDateTimeDayDisplay('garbage')).toBe('garbage');
  });
});

describe('formatDateTimeForStorage', () => {
  it('writes an RFC 3339 date-time in UTC', () => {
    expect(formatDateTimeForStorage(new Date(Date.UTC(2026, 2, 1, 9, 30)))).toBe('2026-03-01T09:30:00.000Z');
  });
});

describe('withLocalDay', () => {
  it('moves a stored value to the picked day, keeping its time of day', () => {
    const next = withLocalDay(stored(2026, 2, 1, 9, 30, 15), new CalendarDate(2026, 4, 20));
    expect(next).toBe(stored(2026, 3, 20, 9, 30, 15));
    expect(next).toMatch(RFC3339_UTC);
  });

  it('starts an unset value at local midnight of the picked day', () => {
    expect(withLocalDay(null, new CalendarDate(2026, 4, 20))).toBe(stored(2026, 3, 20, 0, 0));
  });

  it('clears the value when no day is picked', () => {
    expect(withLocalDay(stored(2026, 2, 1, 9, 30), undefined)).toBeNull();
  });
});

describe('withLocalTime', () => {
  it('moves a stored value to the entered time, keeping its day', () => {
    const next = withLocalTime(stored(2026, 2, 1, 9, 30), '14:45');
    expect(next).toBe(stored(2026, 2, 1, 14, 45));
    expect(next).toMatch(RFC3339_UTC);
  });

  it('starts an unset value on today', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(2026, 9, 5, 8, 0, 0));
      expect(withLocalTime(null, '14:45')).toBe(stored(2026, 9, 5, 14, 45));
    } finally {
      vi.useRealTimers();
    }
  });

  it('reports no change for an emptied or incomplete time', () => {
    const raw = stored(2026, 2, 1, 9, 30);
    expect(withLocalTime(raw, '')).toBeUndefined();
    expect(withLocalTime(raw, '9')).toBeUndefined();
  });
});
