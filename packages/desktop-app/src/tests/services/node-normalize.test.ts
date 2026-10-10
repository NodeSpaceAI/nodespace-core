import { describe, it, expect } from 'vitest';
import {
  normalizeNodeData,
  mergeProperties,
  promoteTypedFields,
  storageNodeToApiFields
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
    const merged = mergeProperties({ 'custom:x': 'keep', provider: 'native' }, { model: 'm1' });
    expect(merged).toEqual({ 'custom:x': 'keep', provider: 'native', model: 'm1' });
  });

  it('treats a missing existing bag as empty', () => {
    expect(mergeProperties(undefined, { model: 'm1' })).toEqual({ model: 'm1' });
  });
});

describe('promoteTypedFields', () => {
  it('promotes only fields present in a flat native-chat write', () => {
    const changes = { turn_status: 'processing' };
    const promoted = promoteTypedFields('ai-chat-native', changes, changes);
    // provider/model omitted → not promoted (guards against undefined-clobber)
    expect(promoted).toEqual({ turnStatus: 'processing' });
    expect('provider' in promoted).toBe(false);
    expect('model' in promoted).toBe(false);
  });

  it('promotes a PTY chat\'s session fields and not the native ones', () => {
    const changes = { session_status: 'ended', session_id: 's-1' };
    const promoted = promoteTypedFields('ai-chat-pty', changes, changes);
    expect(promoted).toEqual({ sessionStatus: 'ended', sessionId: 's-1' });
    expect(promoteTypedFields('ai-chat-native', changes, changes)).toEqual({});
  });

  it('never promotes a typed-update core type field from a properties write', () => {
    // task/person/project core fields have one home, the top level, and are
    // written through the typed update — a properties write can't carry them.
    expect(promoteTypedFields('task', { status: 'done' }, { status: 'done' })).toEqual({});
  });

  it('returns nothing for a node type with no typed fields', () => {
    expect(promoteTypedFields('text', { foo: 'bar' }, { foo: 'bar' })).toEqual({});
  });

  it('promotes an explicit null value (present but null)', () => {
    const changes = { model: null };
    const promoted = promoteTypedFields('ai-chat-native', changes, changes);
    expect('model' in promoted).toBe(true);
    expect(promoted.model).toBeNull();
  });

  it('promotes a native-chat write using the real snake_case payload shape', () => {
    // Mirrors the actual writes in ai-chat-native-node-viewer.svelte: canonical
    // snake_case property keys, promoted to camelCase top-level fields.
    const changes = {
      turn_status: 'processing',
      provider: 'native',
      model: 'claude-sonnet-5'
    };
    const promoted = promoteTypedFields('ai-chat-native', changes, changes);
    expect(promoted).toEqual({
      turnStatus: 'processing',
      provider: 'native',
      model: 'claude-sonnet-5'
    });
  });

  it('promotes the retype write by the type it converts to', () => {
    // Choosing a terminal harness retypes a native chat; the optimistic node
    // must read as a PTY chat immediately, so the PTY field map applies.
    const changes = { agent: 'claude-code', model: null, session_status: 'active' };
    expect(promoteTypedFields('ai-chat-pty', changes, changes)).toEqual({
      agent: 'claude-code',
      model: null,
      sessionStatus: 'active'
    });
  });
});

describe('storageNodeToApiFields', () => {
  it('promotes a query\'s structured fields and drops one of the wrong shape, as the Rust decoder does', () => {
    const fields = storageNodeToApiFields('query', {
      query: {
        target_type: 'task',
        filters: 'oops',
        limit: 25,
        view_config: { lastView: 'kanban' },
        execution_count: 3
      }
    });
    expect(fields.targetType).toBe('task');
    expect(fields.filters).toEqual([]);
    expect(fields.limit).toBe(25);
    expect(fields.viewConfig).toEqual({ lastView: 'kanban' });
    expect(fields.executionCount).toBe(3);
    expect(fields.properties).toEqual({});
  });

  it('promotes a play\'s switch and suspension, defaulting the switch to on', () => {
    const suspended = storageNodeToApiFields('play', {
      play: {
        rules: [],
        enabled: false,
        suspended_reason: 'action_failed',
        suspended_message: 'boom',
        suspended_at: '2026-10-02T10:00:00Z'
      }
    });
    expect(suspended.enabled).toBe(false);
    expect(suspended.suspendedReason).toBe('action_failed');
    expect(suspended.suspendedMessage).toBe('boom');
    expect(suspended.suspendedAt).toBe('2026-10-02T10:00:00Z');
    expect(suspended.properties).toEqual({});

    // An absent or cleared field takes the default, as the Rust decoder does,
    // and so does a switch of the wrong shape.
    for (const play of [{}, { enabled: null, suspended_at: null }, { enabled: 'yes' }]) {
      const fields = storageNodeToApiFields('play', { play });
      expect(fields.enabled).toBe(true);
      expect(fields.suspendedAt).toBeUndefined();
    }
  });

  it('derives isSeeded for a play from its `_seed` marker, which stays off the wire', () => {
    const seeded = storageNodeToApiFields('play', {
      play: { rules: [] },
      _seed: { default_rules: [] }
    });
    expect(seeded.isSeeded).toBe(true);
    expect(seeded.properties).toEqual({});

    expect(storageNodeToApiFields('play', { play: { rules: [] } }).isSeeded).toBe(false);
    // Only a play carries the field.
    expect('isSeeded' in storageNodeToApiFields('task', { _seed: {} })).toBe(false);
  });

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

  it('merges the subtype and base buckets of a native chat into the wire shape', () => {
    const fields = storageNodeToApiFields('ai-chat-native', {
      'ai-chat': {
        agent: 'nodespace',
        model: 'gemma-4-e4b-q4km',
        summary: 'about gemma',
        last_active: '2026-01-02T00:00:00Z'
      },
      'ai-chat-native': {
        provider: 'native',
        turn_status: 'processing',
        context_tokens: 42
      }
    });
    expect(fields).toEqual({
      agent: 'nodespace',
      model: 'gemma-4-e4b-q4km',
      summary: 'about gemma',
      lastActive: '2026-01-02T00:00:00Z',
      provider: 'native',
      turnStatus: 'processing',
      contextTokens: 42,
      properties: {}
    });
  });

  it('moves a chat message\'s declared fields to typed keys and defaults its role', () => {
    expect(
      storageNodeToApiFields('ai-chat-message', {
        'ai-chat-message': {
          role: 'assistant',
          timestamp: '2026-01-02T00:00:00Z',
          outcome: 'clarified',
          options: ['a', 'b']
        }
      })
    ).toEqual({
      role: 'assistant',
      timestamp: '2026-01-02T00:00:00Z',
      outcome: 'clarified',
      options: ['a', 'b'],
      properties: {}
    });
    expect(storageNodeToApiFields('ai-chat-message', {})).toEqual({
      role: 'user',
      properties: {}
    });
  });

  it('merges the subtype and base buckets of a PTY chat into the wire shape', () => {
    const fields = storageNodeToApiFields('ai-chat-pty', {
      'ai-chat': { agent: 'claude-code', summary: 'did a thing' },
      'ai-chat-pty': {
        session_status: 'ended',
        session_id: 's-1',
        transcript: 'hello',
        exit_code: 0,
        'custom:tag': 'x'
      }
    });
    expect(fields).toEqual({
      agent: 'claude-code',
      summary: 'did a thing',
      sessionStatus: 'ended',
      sessionId: 's-1',
      transcript: 'hello',
      exitCode: 0,
      properties: { 'custom:tag': 'x' }
    });
  });

  it('reads the schema defaults for a chat whose buckets store nothing', () => {
    expect(storageNodeToApiFields('ai-chat-native', {})).toEqual({
      agent: '',
      provider: 'native',
      turnStatus: 'idle',
      contextTokens: 0,
      properties: {}
    });
    expect(storageNodeToApiFields('ai-chat-pty', { 'ai-chat': { agent: 'codex' } })).toEqual({
      agent: 'codex',
      sessionStatus: 'active',
      properties: {}
    });
  });

  it('keeps the two chat subtypes\' fields apart', () => {
    const native = storageNodeToApiFields('ai-chat-native', { 'ai-chat-pty': { session_id: 's' } });
    expect('sessionId' in native).toBe(false);
  });

  it('moves task core fields to typed keys', () => {
    const fields = storageNodeToApiFields('task', {
      task: {
        status: 'in_progress',
        priority: 'high',
        due_date: '2024-12-31',
        started_at: '2024-12-01T08:00:00Z',
        'custom:store': 'Costco'
      }
    });
    expect(fields).toEqual({
      status: 'in_progress',
      priority: 'high',
      dueDate: '2024-12-31',
      startedAt: '2024-12-01',
      requiresSpec: true,
      properties: { 'custom:store': 'Costco' }
    });
  });

  it('mirrors Rust at the edges: only the storage key is read', () => {
    // The conversion reads and removes the storage key alone. A key under the
    // wire spelling is not a core field: it is neither promoted nor removed
    // (and a closed core bucket never stores one).
    expect(storageNodeToApiFields('person', { person: { firstName: 'Ada' } })).toEqual({
      properties: { firstName: 'Ada' }
    });
    // A datetime with an offset reduces to its date, space-separated too; one
    // without an offset passes through, as normalize_date_field leaves it.
    expect(
      storageNodeToApiFields('project', { project: { start_date: '2026-03-01 09:00:00Z' } })
    ).toMatchObject({ startDate: '2026-03-01' });
    expect(
      storageNodeToApiFields('project', { project: { start_date: '2026-03-01T09:00:00' } })
    ).toMatchObject({ startDate: '2026-03-01T09:00:00' });
  });

  it('defaults an unset task status to open and requiresSpec to true, as task_node_to_value does', () => {
    expect(storageNodeToApiFields('task', { task: {} })).toEqual({
      status: 'open',
      requiresSpec: true,
      properties: {}
    });
    expect(storageNodeToApiFields('task', { task: { requires_spec: false } })).toMatchObject({
      requiresSpec: false
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

  it('omits an optional typed field absent from storage rather than promoting undefined', () => {
    const fields = storageNodeToApiFields('ai-chat-native', {
      'ai-chat': { agent: 'nodespace' },
      'ai-chat-native': { messages: [], turn_status: 'idle' }
    });
    expect('model' in fields).toBe(false);
    expect('summary' in fields).toBe(false);
    expect('lastActive' in fields).toBe(false);
  });

  it('returns empty properties for missing or non-object input', () => {
    expect(storageNodeToApiFields('schema', undefined)).toEqual({ properties: {} });
    expect(storageNodeToApiFields('schema', {})).toEqual({ properties: {} });
  });
});
