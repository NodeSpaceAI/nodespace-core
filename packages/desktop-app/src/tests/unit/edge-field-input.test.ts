import { describe, it, expect } from 'vitest';
import {
  coerceNumber,
  edgeInputKind,
  edgeInputType,
  edgeInputValue,
  formatEdgeFieldLabel,
  toDateTimeLocalString,
  toInputString
} from '$lib/services/edge-field-input';
import type { RawEdgeField } from '$lib/services/relationship-grouping';
import type { SchemaFieldType } from '$lib/types/generated';

const field = (type: SchemaFieldType): RawEdgeField => ({ name: 'f', type });

describe('edge-field-input: edgeInputKind', () => {
  it('maps a number field to a number input', () => {
    expect(edgeInputKind(field('number'))).toBe('number');
  });

  it('maps a boolean field to a checkbox', () => {
    expect(edgeInputKind(field('boolean'))).toBe('boolean');
  });

  it('keeps datetime distinct from date so the time component survives', () => {
    expect(edgeInputKind(field('date'))).toBe('date');
    expect(edgeInputKind(field('datetime'))).toBe('datetime');
  });

  it('renders text, an enum with no declared values, and the structured types as text', () => {
    expect(edgeInputKind(field('text'))).toBe('text');
    expect(edgeInputKind(field('enum'))).toBe('text');
    expect(edgeInputKind(field('array'))).toBe('text');
    expect(edgeInputKind(field('object'))).toBe('text');
  });
});

describe('edge-field-input: edgeInputType', () => {
  it('picks the native input type for each kind', () => {
    expect(edgeInputType('date')).toBe('date');
    expect(edgeInputType('datetime')).toBe('datetime-local');
    expect(edgeInputType('text')).toBe('text');
  });
});

describe('edge-field-input: coerceNumber', () => {
  it('parses a numeric string', () => {
    expect(coerceNumber('42')).toBe(42);
    expect(coerceNumber('-3.5')).toBe(-3.5);
  });

  it('treats blank and unparseable input as no value', () => {
    expect(coerceNumber('')).toBeNull();
    expect(coerceNumber('   ')).toBeNull();
    expect(coerceNumber('abc')).toBeNull();
  });
});

describe('edge-field-input: toInputString', () => {
  it('renders null and undefined as an empty input', () => {
    expect(toInputString(null)).toBe('');
    expect(toInputString(undefined)).toBe('');
  });

  it('stringifies primitives', () => {
    expect(toInputString('hi')).toBe('hi');
    expect(toInputString(7)).toBe('7');
    expect(toInputString(false)).toBe('false');
  });
});

describe('edge-field-input: toDateTimeLocalString', () => {
  it('keeps the time component an ISO timestamp carries', () => {
    expect(toDateTimeLocalString('2026-03-04T09:30:00Z')).toBe('2026-03-04T09:30');
    expect(toDateTimeLocalString('2026-03-04 09:30:00')).toBe('2026-03-04T09:30');
  });

  it('round-trips the value the input itself produces without drift', () => {
    const fromInput = '2026-03-04T09:30';
    expect(toDateTimeLocalString(fromInput)).toBe(fromInput);
  });

  it('yields an empty string for blank or unparseable values', () => {
    expect(toDateTimeLocalString('')).toBe('');
    expect(toDateTimeLocalString(null)).toBe('');
    expect(toDateTimeLocalString('not a date')).toBe('');
  });
});

describe('edge-field-input: edgeInputValue', () => {
  it('formats only datetime values specially', () => {
    expect(edgeInputValue('datetime', '2026-03-04T09:30:00Z')).toBe('2026-03-04T09:30');
    expect(edgeInputValue('date', '2026-03-04')).toBe('2026-03-04');
    expect(edgeInputValue('text', 'owner')).toBe('owner');
  });
});

describe('edge-field-input: formatEdgeFieldLabel', () => {
  it('turns separators into spaces for the input label', () => {
    expect(formatEdgeFieldLabel('due_by')).toBe('due by');
    expect(formatEdgeFieldLabel('read-only')).toBe('read only');
  });
});
