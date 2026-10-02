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
    type: 'text',
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

  it('decides a typed core type by its schema, like any other', () => {
    // `task.status` is required but defaults to `open`, so a task is created
    // right away; `skill.description` has no default, so a skill waits.
    const task = schema('task', [field({ name: 'status', required: true, default: 'open' })]);
    expect(needsUnsavedPlaceholder(task)).toBe(false);
    expect(needsUnsavedPlaceholder(skillLike)).toBe(true);
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
    // A skill's description is a typed field, read from the top level.
    expect(
      missingRequiredFields({ nodeType: 'skill', properties: {}, description: '  ' } as never, required)
    ).toEqual(['description']);
    expect(
      missingRequiredFields({ nodeType: 'skill', properties: {}, description: 'x' } as never, required)
    ).toEqual([]);
    // A user-defined type's field is read from `properties`.
    const noteRequired = [field({ name: 'body', required: true })];
    expect(
      missingRequiredFields({ nodeType: 'note', properties: { body: 'x' } } as never, noteRequired)
    ).toEqual([]);
    expect(
      missingRequiredFields({ nodeType: 'note', properties: {} } as never, noteRequired)
    ).toEqual(['body']);
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
    const typedSpy = vi.spyOn(backendAdapter, 'updateSkillNode');

    // The form writes a typed field at the top level; the create carries it
    // under its storage name, since a node without it would be rejected.
    sharedNodeStore.updateNode(
      node.id,
      { description: 'Summarises a thread', maxIterations: 3 } as never,
      { type: 'viewer', viewerId: 'test' }
    );

    await vi.waitFor(() => expect(createNodeSpy).toHaveBeenCalledTimes(1));
    expect(createNodeSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({
        id: node.id,
        nodeType: 'skill',
        properties: { description: 'Summarises a thread', max_iterations: 3 }
      })
    );
    expect(typedSpy).not.toHaveBeenCalled();
    expect(sharedNodeStore.isUnsavedPlaceholder(node.id)).toBe(false);
  });

  it('creates the skill with the description as typed so far, one keystroke at a time', async () => {
    // The form writes on every keystroke. The first character completes the
    // placeholder and queues its create; the ones typed before that create
    // goes out are staged typed writes. The create must still carry the
    // description, or the backend rejects the node and nothing is ever saved.
    const node = createInstancePlaceholder(skillLike);
    const viewer = { type: 'viewer' as const, viewerId: 'test' };
    const order: string[] = [];
    createNodeSpy.mockImplementation(async (input) => {
      order.push(`create:${JSON.stringify((input as { properties: unknown }).properties)}`);
      return { id: (input as { id: string }).id, placement: null };
    });
    vi.spyOn(backendAdapter, 'updateSkillNode').mockImplementation(async (id, version, update) => {
      order.push(`typed:${JSON.stringify(update)}`);
      return { ...node, ...update, id, version: version + 1 } as never;
    });

    for (const description of ['S', 'Su', 'Sum']) {
      sharedNodeStore.updateNode(node.id, { description } as never, viewer);
    }

    await vi.waitFor(() => expect(order).toHaveLength(2), { timeout: 3000 });
    expect(order).toEqual(['create:{"description":"Sum"}', 'typed:{"description":"Sum"}']);
    expect(createNodeSpy).toHaveBeenCalledTimes(1);
    expect(sharedNodeStore.isNodePersisted(node.id)).toBe(true);
  });

  it('creates a user-defined type with its required property', async () => {
    const note = schema('note', [field({ name: 'body', required: true })]);
    const node = createInstancePlaceholder(note);

    sharedNodeStore.updateNode(
      node.id,
      { properties: { body: 'A thought' } },
      { type: 'viewer', viewerId: 'test' }
    );

    await vi.waitFor(() => expect(createNodeSpy).toHaveBeenCalledTimes(1));
    expect(createNodeSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({ id: node.id, nodeType: 'note', properties: { body: 'A thought' } })
    );
  });
});
