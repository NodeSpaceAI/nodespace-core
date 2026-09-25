/**
 * Generic schema form field resolution.
 *
 * Every transport flattens a node's storage bucket (`properties[nodeType]`) away before it
 * reaches the frontend, so the form reads and writes bare keys for core and user-defined
 * types alike. The backend re-buckets bare keys on write.
 */

import { describe, it, expect } from 'vitest';
import {
  resolveFieldValue,
  buildFieldWrite
} from '$lib/components/schema/schema-field-resolution';

describe('resolveFieldValue', () => {
  it('reads a flat field for a core type', () => {
    const node = { nodeType: 'project', properties: { status: 'planning', priority: 'high' } };

    expect(resolveFieldValue(node, 'status')).toBe('planning');
    expect(resolveFieldValue(node, 'priority')).toBe('high');
  });

  it('reads a flat field for a user-defined schema type', () => {
    const node = {
      nodeType: '7b1c2d3e-4f56-7890-abcd-ef1234567890',
      properties: { capacity: 250 }
    };

    expect(resolveFieldValue(node, 'capacity')).toBe(250);
  });

  it('returns null for an unset field', () => {
    expect(resolveFieldValue({ nodeType: 'project', properties: {} }, 'status')).toBe(null);
    expect(resolveFieldValue({ nodeType: 'project' }, 'status')).toBe(null);
  });
});

describe('buildFieldWrite', () => {
  it('writes flat and preserves sibling fields', () => {
    const node = { nodeType: 'project', properties: { status: 'planning', priority: 'high' } };

    expect(buildFieldWrite(node, 'status', 'active')).toEqual({
      status: 'active',
      priority: 'high'
    });
  });

  it('writes flat when the node has no properties yet', () => {
    expect(buildFieldWrite({ nodeType: 'project' }, 'status', 'active')).toEqual({
      status: 'active'
    });
  });

  it('does not mutate the original properties', () => {
    const properties = { status: 'planning' };
    const node = { nodeType: 'project', properties };

    buildFieldWrite(node, 'status', 'active');

    expect(properties.status).toBe('planning');
  });

  it('round-trips with resolveFieldValue', () => {
    const node = { nodeType: 'venue-uuid', properties: { capacity: 100 } };

    expect(
      resolveFieldValue({ ...node, properties: buildFieldWrite(node, 'capacity', 250) }, 'capacity')
    ).toBe(250);
  });
});
