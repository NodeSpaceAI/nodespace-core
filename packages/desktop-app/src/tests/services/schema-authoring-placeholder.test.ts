import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import {
  requiredFieldsWithoutDefault,
  missingRequiredFields,
  needsUnsavedPlaceholder,
  createInstancePlaceholder
} from '$lib/services/schema-authoring';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';

function field(overrides: Partial<SchemaField> & { name: string }): SchemaField {
  return {
    type: 'string',
    protection: 'user',
    indexed: false,
    ...overrides
  } as SchemaField;
}

function schema(id: string, fields: SchemaField[]): SchemaNode {
  return {
    id,
    nodeType: 'schema',
    content: id,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    description: '',
    fields
  } as SchemaNode;
}

const skillLike = schema('skill', [
  field({ name: 'description', required: true }),
  field({ name: 'tool_whitelist', required: true, default: [], type: 'array' }),
  field({ name: 'notes' })
]);

describe('placeholder eligibility', () => {
  it('lists only required fields that have no default', () => {
    expect(requiredFieldsWithoutDefault(skillLike).map((f) => f.name)).toEqual(['description']);
  });

  it('needs a placeholder only when such a field exists', () => {
    expect(needsUnsavedPlaceholder(skillLike)).toBe(true);
    expect(needsUnsavedPlaceholder(schema('note', [field({ name: 'body' })]))).toBe(false);
    expect(
      needsUnsavedPlaceholder(schema('note', [field({ name: 'body', required: true, default: '' })]))
    ).toBe(false);
    expect(needsUnsavedPlaceholder(null)).toBe(false);
  });

  it('keeps the immediate create for typed core types', () => {
    const task = schema('task', [field({ name: 'status', required: true })]);
    expect(needsUnsavedPlaceholder(task)).toBe(false);
  });

  it('ignores required system-managed fields', () => {
    const s = schema('x', [field({ name: 'internal', required: true, protection: 'system' })]);
    expect(needsUnsavedPlaceholder(s)).toBe(false);
  });

  it('reports a blank or absent value as missing', () => {
    const required = requiredFieldsWithoutDefault(skillLike);
    const base = { nodeType: 'skill' } as never;
    expect(missingRequiredFields({ ...(base as object), properties: {} } as never, required)).toEqual([
      'description'
    ]);
    expect(
      missingRequiredFields({ nodeType: 'skill', properties: { description: '  ' } } as never, required)
    ).toEqual(['description']);
    expect(
      missingRequiredFields({ nodeType: 'skill', properties: { description: 'x' } } as never, required)
    ).toEqual([]);
  });
});

describe('createInstancePlaceholder', () => {
  let createNodeSpy: MockInstance<typeof backendAdapter.createNode>;

  beforeEach(() => {
    createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async (input) => ({ id: (input as { id: string }).id, placement: null }));
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);
  });

  afterEach(() => {
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  it('adds a seeded instance to the store without writing to the backend', async () => {
    const node = createInstancePlaceholder(skillLike);
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(node.nodeType).toBe('skill');
    expect(node.content).toBe('Untitled Skill');
    expect(sharedNodeStore.getNode(node.id)).toBeDefined();
    expect(sharedNodeStore.isUnsavedPlaceholder(node.id)).toBe(true);
    expect(createNodeSpy).not.toHaveBeenCalled();
  });

  it('is created once the required description is filled', async () => {
    const node = createInstancePlaceholder(skillLike);

    sharedNodeStore.updateNode(
      node.id,
      { properties: { description: 'Summarises a thread' } },
      { type: 'viewer', viewerId: 'test' }
    );

    await vi.waitFor(() => expect(createNodeSpy).toHaveBeenCalledTimes(1));
    expect(createNodeSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({
        id: node.id,
        nodeType: 'skill',
        properties: { description: 'Summarises a thread' }
      })
    );
  });
});
