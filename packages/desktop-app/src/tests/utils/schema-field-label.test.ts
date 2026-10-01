/**
 * labelForField — the single shared UI label accessor for a schema field.
 *
 * Must read `friendlyName` unconditionally: no fallback to `description`
 * (LLM-facing prose, not label text) and no computed-from-`name` regex.
 */
import { describe, it, expect } from 'vitest';
import type { SchemaField } from '$lib/types/schema-node';
import { labelForField } from '$lib/utils/schema-field-label';

function field(overrides: Partial<SchemaField> = {}): SchemaField {
  return {
    name: 'due_date',
    friendlyName: 'Due date',
    type: 'date',
    protection: 'user',
    indexed: false,
    ...overrides
  };
}

describe('labelForField', () => {
  it('returns friendlyName', () => {
    expect(labelForField(field({ friendlyName: 'Due date' }))).toBe('Due date');
  });

  it('does not fall back to description even when friendlyName differs', () => {
    const f = field({
      friendlyName: 'Email',
      description: 'Email address (optional)'
    });
    expect(labelForField(f)).toBe('Email');
  });

  it('returns an empty string as-is rather than substituting a computed label', () => {
    // friendlyName is guaranteed non-empty in storage (derived at the write
    // boundary), but the helper itself must not paper over a violation of
    // that invariant with a silent fallback — that would reintroduce the
    // null-branching the friendly_name split was meant to remove.
    expect(labelForField(field({ friendlyName: '' }))).toBe('');
  });
});
