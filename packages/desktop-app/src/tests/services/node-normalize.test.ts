import { describe, it, expect } from 'vitest';
import {
  normalizeNodeData,
  mergeProperties,
  promoteTypedFields,
  storageNodeToApiFields,
  OPTIMISTIC_TYPED_FIELDS
} from '$lib/services/node-normalize';
import type { Node } from '$lib/types/node';

function makeNode(overrides: Partial<Node> = {}): Node {
  return {
    id: 'test-id',
    nodeType: 'text',
    content: 'test content',
    version: 1,
    createdAt: '2024-01-01T00:00:00Z',
    modifiedAt: '2024-01-01T00:00:00Z',
    ...overrides
  } as Node;
}

describe('normalizeNodeData', () => {
  it('returns non-task nodes unchanged', () => {
    const node = makeNode({ nodeType: 'text' });
    expect(normalizeNodeData(node)).toBe(node);
  });

  // The backend (`node_to_typed_value`) promotes type-specific fields to the TOP
  // LEVEL of the node for every transport — see the `wire_contract` tests in
  // `nodespace-types/src/convert.rs`. These tests pin that flat contract on the TS
  // side; the converters intentionally no longer accept the nested `properties.task`
  // shape, which no live producer emits.
  it('passes through a flat task node, preserving promoted fields', () => {
    const node = makeNode({
      nodeType: 'task',
      status: 'done',
      priority: 'high'
    } as Partial<Node>);
    const result = normalizeNodeData(node);
    expect(result.nodeType).toBe('task');
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((result as any).status).toBe('done');
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((result as any).priority).toBe('high');
  });

  it('task node with no status gets default "open"', () => {
    const node = makeNode({ nodeType: 'task' });
    const result = normalizeNodeData(node);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((result as any).status).toBe('open');
  });

  // Drift guard: both sync paths (Tauri + browser) call this single function, so the
  // transformation contract below applies identically to both runtime modes. Adding a
  // future type branch here is the one-place change that covers both paths.
  it('normalizes a full flat task node with status, priority, and dueDate', () => {
    const node = makeNode({
      nodeType: 'task',
      status: 'in_progress',
      priority: 'low',
      dueDate: '2024-12-31'
    } as Partial<Node>);
    const result = normalizeNodeData(node);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const r = result as any;
    expect(r.nodeType).toBe('task');
    expect(r.status).toBe('in_progress');
    expect(r.priority).toBe('low');
    expect(r.dueDate).toBe('2024-12-31');
    expect(r.id).toBe('test-id');
    expect(r.content).toBe('test content');
  });
});

describe('mergeProperties', () => {
  it('merges one level and keeps sibling keys', () => {
    const merged = mergeProperties({ 'capture:x': 'keep', provider: 'native' }, { model: 'm1' });
    expect(merged).toEqual({ 'capture:x': 'keep', provider: 'native', model: 'm1' });
  });

  it('treats a missing existing bag as empty', () => {
    expect(mergeProperties(undefined, { model: 'm1' })).toEqual({ model: 'm1' });
  });
});

describe('promoteTypedFields', () => {
  it('promotes only fields present in a flat ai-chat write', () => {
    const changes = { messages: [{ role: 'user', content: 'hi' }], turn_status: 'processing' };
    const promoted = promoteTypedFields('ai-chat', changes, changes);
    // provider/model/sessionStatus omitted → not promoted (guards against undefined-clobber)
    expect(promoted).toEqual({ messages: changes.messages, turnStatus: 'processing' });
    expect('provider' in promoted).toBe(false);
    expect('model' in promoted).toBe(false);
    expect('sessionStatus' in promoted).toBe(false);
  });

  it('promotes turnStatus and sessionStatus independently', () => {
    const changes = { session_status: 'archived' };
    const promoted = promoteTypedFields('ai-chat', changes, changes);
    expect(promoted).toEqual({ sessionStatus: 'archived' });
    expect('turnStatus' in promoted).toBe(false);
  });

  it('never promotes a typed core type field from a properties write', () => {
    // task/person/project core fields have one home, the top level, and are
    // written through the typed update — a properties write can't carry them.
    expect(promoteTypedFields('task', { status: 'done' }, { status: 'done' })).toEqual({});
  });

  it('returns nothing for a node type with no typed-field map', () => {
    expect(promoteTypedFields('text', { foo: 'bar' }, { foo: 'bar' })).toEqual({});
  });

  it('promotes an explicit null value (present but null)', () => {
    const changes = { model: null };
    const promoted = promoteTypedFields('ai-chat', changes, changes);
    expect('model' in promoted).toBe(true);
    expect(promoted.model).toBeNull();
  });

  it('map stays aligned with the documented promoted types', () => {
    expect(Object.keys(OPTIMISTIC_TYPED_FIELDS).sort()).toEqual(['ai-chat']);
    expect(OPTIMISTIC_TYPED_FIELDS['ai-chat']).toEqual([
      { from: 'turn_status', to: 'turnStatus' },
      { from: 'session_status', to: 'sessionStatus' },
      { from: 'provider', to: 'provider' },
      { from: 'model', to: 'model' },
      { from: 'messages', to: 'messages' }
    ]);
  });

  it('promotes an ai-chat write using the real snake_case payload shape', () => {
    // Mirrors the actual write in ai-chat-node-viewer.svelte: canonical
    // snake_case property keys, promoted to camelCase top-level fields.
    const changes = {
      messages: [{ role: 'user', content: 'hi' }],
      turn_status: 'processing',
      session_status: 'active',
      provider: 'native',
      model: 'claude-sonnet-5'
    };
    const promoted = promoteTypedFields('ai-chat', changes, changes);
    expect(promoted).toEqual({
      messages: changes.messages,
      turnStatus: 'processing',
      sessionStatus: 'active',
      provider: 'native',
      model: 'claude-sonnet-5'
    });
  });
});

describe('storageNodeToApiFields', () => {
  // The browser/dev-proxy HTTP transport (packages/dev-tools/src/dev-proxy.ts)
  // receives a node's `properties` exactly as stored — namespaced under the
  // node's own type. The Tauri IPC layer's `node_to_typed_value`
  // (packages/nodespace-types/src/convert.rs) flattens that bucket and promotes
  // typed fields; this is the proxy's mirror of it, so both transports hand the
  // frontend the same shape.

  it('moves person core fields to typed keys, leaving extension fields', () => {
    const fields = storageNodeToApiFields('person', {
      person: {
        first_name: 'Michael',
        last_name: 'Libio',
        email: 'm@example.com',
        'custom:team': 'Core'
      }
    });
    expect(fields).toEqual({
      firstName: 'Michael',
      lastName: 'Libio',
      email: 'm@example.com',
      properties: { 'custom:team': 'Core' }
    });
  });

  it('moves project core fields, normalizing dates and defaulting status', () => {
    const fields = storageNodeToApiFields('project', {
      project: { start_date: '2026-03-01T09:00:00Z', end_date: '2026-04-30' }
    });
    expect(fields).toEqual({
      status: 'planning',
      startDate: '2026-03-01',
      endDate: '2026-04-30',
      properties: {}
    });
  });

  it('flattens and promotes an ai-chat node in real storage shape', () => {
    const bucket = {
      context_tokens: 0,
      created_nodes: [],
      messages: [],
      provider: 'native',
      model: 'gemma-4-e4b-q4km',
      session_status: 'active',
      turn_status: 'idle'
    };
    const fields = storageNodeToApiFields('ai-chat', { 'ai-chat': bucket });
    expect(fields).toEqual({
      properties: bucket,
      provider: 'native',
      model: 'gemma-4-e4b-q4km',
      sessionStatus: 'active',
      turnStatus: 'idle',
      messages: []
    });
  });

  it('moves task core fields to typed keys, reading either date spelling', () => {
    const fields = storageNodeToApiFields('task', {
      task: {
        status: 'in_progress',
        priority: 'high',
        due_date: '2024-12-31',
        startedAt: '2024-12-01T08:00:00Z',
        'custom:store': 'Costco'
      }
    });
    expect(fields).toEqual({
      status: 'in_progress',
      priority: 'high',
      dueDate: '2024-12-31',
      startedAt: '2024-12-01',
      properties: { 'custom:store': 'Costco' }
    });
  });

  it('mirrors Rust at the edges: typed-key fallback is task-only, and a present null wins', () => {
    // person_node_to_value reads the storage key alone: a stray typed-key
    // spelling is not promoted (and, like every core-field spelling, is dropped).
    expect(storageNodeToApiFields('person', { person: { firstName: 'Ada' } })).toEqual({
      properties: {}
    });
    // task reads `dueDate` first when present at all, even as null — no
    // fallback to `due_date` then, as `.get(wire).or_else(storage)` behaves.
    expect(
      storageNodeToApiFields('task', { task: { dueDate: null, due_date: '2026-05-01' } })
    ).toEqual({ status: 'open', properties: {} });
    // A datetime with an offset reduces to its date, space-separated too; one
    // without an offset passes through, as normalize_date_field leaves it.
    expect(
      storageNodeToApiFields('project', { project: { start_date: '2026-03-01 09:00:00Z' } })
    ).toMatchObject({ startDate: '2026-03-01' });
    expect(
      storageNodeToApiFields('project', { project: { start_date: '2026-03-01T09:00:00' } })
    ).toMatchObject({ startDate: '2026-03-01T09:00:00' });
  });

  it('defaults an unset task status to open, as task_node_to_value does', () => {
    expect(storageNodeToApiFields('task', { task: {} })).toEqual({
      status: 'open',
      properties: {}
    });
  });

  it('keeps object-valued fields inside the own bucket', () => {
    const address = { city: 'Austin' };
    const fields = storageNodeToApiFields('venue', { venue: { address } });
    expect(fields.properties).toEqual({ address });
  });

  it('drops _-prefixed bookkeeping and sibling namespaces', () => {
    const fields = storageNodeToApiFields('person', {
      _schema_version: 1,
      _seed: { id: 'x' },
      text: { dormant: true },
      person: { 'custom:team': 'Core', _internal: 'hidden' }
    });
    expect(fields.properties).toEqual({ 'custom:team': 'Core' });
  });

  it('passes an already-flat bag through, dropping only nested objects and _ keys', () => {
    const fields = storageNodeToApiFields('schema', {
      isCore: true,
      fields: [{ name: 'a' }],
      _schema_version: 2,
      dormant: { x: 1 }
    });
    expect(fields.properties).toEqual({ isCore: true, fields: [{ name: 'a' }] });
  });

  it('omits a typed field absent from storage rather than promoting undefined', () => {
    const fields = storageNodeToApiFields('ai-chat', {
      'ai-chat': { messages: [], turn_status: 'idle', session_status: 'active' }
    });
    expect('model' in fields).toBe(false);
    expect('provider' in fields).toBe(false);
  });

  it('returns empty properties for missing or non-object input', () => {
    expect(storageNodeToApiFields('ai-chat', undefined)).toEqual({ properties: {} });
    expect(storageNodeToApiFields('ai-chat', {})).toEqual({ properties: {} });
  });
});
