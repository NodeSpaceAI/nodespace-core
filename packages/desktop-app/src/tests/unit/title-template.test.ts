import { describe, it, expect } from 'vitest';
import { evaluateTitleTemplate, evaluateSummaryTemplate } from '$lib/utils/title-template';
import type { SchemaField } from '$lib/types/schema-node';

describe('evaluateTitleTemplate', () => {
  it('interpolates a basic two-token template', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: 'Alice', last_name: 'Smith' })).toBe('Alice Smith');
  });

  it('returns empty string when all tokens are missing', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', {})).toBe('');
  });

  it('returns empty string when all tokens are null', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: null, last_name: null })).toBe('');
  });

  it('returns empty string when all tokens are undefined', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: undefined, last_name: undefined })).toBe('');
  });

  it('handles a partially filled template (one token present)', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: 'Alice' })).toBe('Alice');
  });

  it('collapses whitespace when a middle token is empty', () => {
    // "Alice  Smith" → trimmed/collapsed to "Alice Smith"
    expect(evaluateTitleTemplate('{first_name} {middle} {last_name}', { first_name: 'Alice', last_name: 'Smith' })).toBe('Alice Smith');
  });

  it('trims leading and trailing whitespace', () => {
    expect(evaluateTitleTemplate('  {first_name}  ', { first_name: 'Alice' })).toBe('Alice');
  });

  it('coerces numeric values to string', () => {
    expect(evaluateTitleTemplate('Invoice #{number}', { number: 42 })).toBe('Invoice #42');
  });

  it('coerces boolean values to string', () => {
    expect(evaluateTitleTemplate('Active: {active}', { active: true })).toBe('Active: true');
  });

  it('handles repeated tokens', () => {
    expect(evaluateTitleTemplate('{name} ({name})', { name: 'Alice' })).toBe('Alice (Alice)');
  });

  it('returns the template literal text when there are no tokens', () => {
    expect(evaluateTitleTemplate('No tokens here', {})).toBe('No tokens here');
  });

  // Token names are not restricted to word characters — the pattern scans to the
  // next '}', matching the Rust scanner in packages/core/src/utils/markdown.rs.
  // These cases previously passed through literally here while the backend
  // resolved them, so the optimistic title and the persisted title disagreed.
  it('resolves hyphenated token names', () => {
    expect(evaluateTitleTemplate('{first-name}', { 'first-name': 'Alice' })).toBe('Alice');
  });

  it('resolves namespace-prefixed token names', () => {
    expect(evaluateTitleTemplate('{custom:capacity}', { 'custom:capacity': 120 })).toBe('120');
  });

  it('handles empty template string', () => {
    expect(evaluateTitleTemplate('', { first_name: 'Alice' })).toBe('');
  });

  // Mirrors interpolate_title_template_with_schema (packages/core/src/utils/markdown.rs),
  // which compute_title() actually calls — that function resolves an enum
  // field's stored value to its label, so the client-side preview must too,
  // or it flashes the raw value ("p1") before the backend's round trip
  // replaces it with the label ("P1 - Critical").
  describe('enum resolution (fields argument)', () => {
    const severityField: SchemaField = {
      name: 'severity',
      friendlyName: 'Severity',
      type: 'enum',
      protection: 'user',
      indexed: false,
      coreValues: [
        { value: 'p1', label: 'P1 - Critical' },
        { value: 'p2', label: 'P2 - High' }
      ],
      userValues: [{ value: 'custom', label: 'Custom Severity' }]
    };

    it('resolves an enum core value to its label when fields is given', () => {
      expect(
        evaluateTitleTemplate('{severity}: Disk full', { severity: 'p1' }, [severityField])
      ).toBe('P1 - Critical: Disk full');
    });

    it('resolves an enum user value to its label', () => {
      expect(
        evaluateTitleTemplate('{severity}', { severity: 'custom' }, [severityField])
      ).toBe('Custom Severity');
    });

    it('falls back to the raw value for an enum entry not in coreValues/userValues', () => {
      expect(
        evaluateTitleTemplate('{severity}', { severity: 'unknown_val' }, [severityField])
      ).toBe('unknown_val');
    });

    it('leaves a non-enum field raw even when fields is given', () => {
      const subjectField: SchemaField = {
        name: 'subject',
        friendlyName: 'Subject',
        type: 'string',
        protection: 'user',
        indexed: false
      };
      expect(
        evaluateTitleTemplate('{subject}', { subject: 'Disk full' }, [subjectField])
      ).toBe('Disk full');
    });

    it('falls back to the raw value when fields is omitted (backward compatible)', () => {
      expect(evaluateTitleTemplate('{severity}', { severity: 'p1' })).toBe('p1');
    });

    // Unlike evaluateSummaryTemplate, title interpolation never reformats a
    // date — the backend's title computation doesn't either, so doing so
    // here would make the client preview disagree with the persisted title.
    it('does not reformat a date field, unlike evaluateSummaryTemplate', () => {
      const dueDateField: SchemaField = {
        name: 'due_date',
        friendlyName: 'Due date',
        type: 'date',
        protection: 'user',
        indexed: false
      };
      expect(
        evaluateTitleTemplate('{due_date}', { due_date: '2026-06-30' }, [dueDateField])
      ).toBe('2026-06-30');
    });
  });
});

describe('evaluateSummaryTemplate', () => {
  const statusField: SchemaField = {
    name: 'status',
    friendlyName: 'Status',
    type: 'enum',
    protection: 'user',
    indexed: false,
    coreValues: [
      { value: 'active', label: 'Active' },
      { value: 'on_hold', label: 'On Hold' },
    ],
    userValues: [{ value: 'custom', label: 'Custom Status' }],
  };

  const companyField: SchemaField = {
    name: 'company',
    friendlyName: 'Company',
    type: 'text',
    protection: 'user',
    indexed: false,
  };

  const dueDateField: SchemaField = {
    name: 'due_date',
    friendlyName: 'Due date',
    type: 'date',
    protection: 'user',
    indexed: false,
  };

  it('interpolates plain text fields without modification', () => {
    expect(
      evaluateSummaryTemplate('{company}', { company: 'Acme Corp' }, [companyField])
    ).toBe('Acme Corp');
  });

  it('resolves enum core values to labels', () => {
    expect(
      evaluateSummaryTemplate('{status}', { status: 'active' }, [statusField])
    ).toBe('Active');
  });

  it('resolves enum user values to labels', () => {
    expect(
      evaluateSummaryTemplate('{status}', { status: 'custom' }, [statusField])
    ).toBe('Custom Status');
  });

  it('falls back to raw value for unknown enum entries', () => {
    expect(
      evaluateSummaryTemplate('{status}', { status: 'unknown_val' }, [statusField])
    ).toBe('unknown_val');
  });

  it('formats date fields as human-readable strings', () => {
    const result = evaluateSummaryTemplate('{due_date}', { due_date: '2026-06-30' }, [dueDateField]);
    expect(result).toMatch(/Jun/); // locale-formatted month name
    expect(result).toMatch(/2026/);
  });

  it('combines enum and text tokens in a separator template', () => {
    expect(
      evaluateSummaryTemplate('{status} · {company}', { status: 'on_hold', company: 'Acme Corp' }, [
        statusField,
        companyField,
      ])
    ).toBe('On Hold · Acme Corp');
  });

  it('returns only literal separators when all tokens are missing', () => {
    // Tokens resolve to empty string, but literal separator characters remain
    expect(evaluateSummaryTemplate('{status} · {company}', {}, [statusField, companyField])).toBe('·');
  });

  it('returns empty string when template has only tokens and all are missing', () => {
    expect(evaluateSummaryTemplate('{status}{company}', {}, [statusField, companyField])).toBe('');
  });

  it('collapses whitespace when a token is missing', () => {
    expect(
      evaluateSummaryTemplate('{status} · {company}', { status: 'active' }, [statusField, companyField])
    ).toBe('Active ·');
  });

  it('handles empty template', () => {
    expect(evaluateSummaryTemplate('', { status: 'active' }, [statusField])).toBe('');
  });

  it('leaves non-date string values on unknown fields as-is', () => {
    const textField: SchemaField = {
      name: 'note',
      friendlyName: 'Note',
      type: 'text',
      protection: 'user',
      indexed: false
    };
    expect(evaluateSummaryTemplate('{note}', { note: 'hello' }, [textField])).toBe('hello');
  });

  it('resolves namespace-prefixed token names, including enum labels', () => {
    const namespacedEnum: SchemaField = {
      name: 'custom:status',
      friendlyName: 'Status',
      type: 'enum',
      protection: 'user',
      indexed: false,
      userValues: [{ value: 'on_hold', label: 'On Hold' }]
    };
    expect(
      evaluateSummaryTemplate('{custom:status}', { 'custom:status': 'on_hold' }, [namespacedEnum])
    ).toBe('On Hold');
  });
});
