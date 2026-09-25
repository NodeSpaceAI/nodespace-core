/**
 * Schema field resolution.
 *
 * A core type's schema-declared fields are typed top-level fields (project's
 * `start_date` is `node.startDate`); every other field is an extension field,
 * flat in `properties`. Reads and writes must agree on which slot a field uses.
 */

import { describe, it, expect } from 'vitest';
import {
  resolveFieldValue,
  buildFieldWrite
} from '$lib/components/schema/schema-field-resolution';

describe('resolveFieldValue', () => {
  it('reads a core type field from its typed top-level key', () => {
    const node = {
      nodeType: 'project',
      status: 'active',
      startDate: '2026-03-01',
      properties: {}
    };

    expect(resolveFieldValue(node, 'status')).toBe('active');
    expect(resolveFieldValue(node, 'start_date')).toBe('2026-03-01');
  });

  it('never reads a core type field from properties', () => {
    const node = { nodeType: 'person', properties: { first_name: 'stale copy' } };

    expect(resolveFieldValue(node, 'first_name')).toBe(null);
  });

  it('reads an extension field on a core type from properties', () => {
    const node = { nodeType: 'task', status: 'open', properties: { 'custom:store': 'Costco' } };

    expect(resolveFieldValue(node, 'custom:store')).toBe('Costco');
  });

  it('reads a flat field for a user-defined schema type, even one named like a core field', () => {
    const node = {
      nodeType: '7b1c2d3e-4f56-7890-abcd-ef1234567890',
      properties: { capacity: 250, status: 'booked' }
    };

    expect(resolveFieldValue(node, 'capacity')).toBe(250);
    expect(resolveFieldValue(node, 'status')).toBe('booked');
  });

  it('returns null for an unset field', () => {
    expect(resolveFieldValue({ nodeType: 'project', properties: {} }, 'priority')).toBe(null);
    expect(resolveFieldValue({ nodeType: 'venue' }, 'capacity')).toBe(null);
  });
});

describe('buildFieldWrite', () => {
  it('writes a core type field as a typed top-level change', () => {
    const node = { nodeType: 'project', status: 'planning', properties: { 'custom:x': 1 } };

    expect(buildFieldWrite(node, 'start_date', '2026-03-01')).toEqual({
      startDate: '2026-03-01'
    });
  });

  it('clears a typed field with null rather than an empty string', () => {
    const node = { nodeType: 'person', properties: {} };

    expect(buildFieldWrite(node, 'email', '')).toEqual({ email: null });
  });

  it('writes an extension field flat and preserves sibling extension fields', () => {
    const node = { nodeType: 'venue', properties: { capacity: 100, city: 'Austin' } };

    expect(buildFieldWrite(node, 'capacity', 250)).toEqual({
      properties: { capacity: 250, city: 'Austin' }
    });
  });

  it('does not mutate the original properties', () => {
    const properties = { capacity: 100 };
    const node = { nodeType: 'venue', properties };

    buildFieldWrite(node, 'capacity', 250);

    expect(properties.capacity).toBe(100);
  });

  it('round-trips with resolveFieldValue for both slots', () => {
    const venue = { nodeType: 'venue', properties: { capacity: 100 } };
    const project = { nodeType: 'project', status: 'planning', properties: {} };

    expect(
      resolveFieldValue({ ...venue, ...buildFieldWrite(venue, 'capacity', 250) }, 'capacity')
    ).toBe(250);
    expect(
      resolveFieldValue({ ...project, ...buildFieldWrite(project, 'status', 'active') }, 'status')
    ).toBe('active');
  });
});
