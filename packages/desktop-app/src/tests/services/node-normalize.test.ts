import { describe, it, expect } from 'vitest';
import {
  normalizeNodeData,
  mergeProperties,
  promoteTypedFields
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

describe('normalizeNodeData for the core subtypes of task and project', () => {
  it('converts an issue to its own typed shape, never a task\'s', () => {
    const node = makeNode({ nodeType: 'issue' }) as unknown as Record<string, unknown>;
    node.status = 'ready_to_merge';
    node.pullRequest = { title: 'PR 7', url: 'https://example.com/pr/7' };
    const typed = normalizeNodeData(node as unknown as Node) as unknown as Record<string, unknown>;
    expect(typed.nodeType).toBe('issue');
    expect(typed.status).toBe('ready_to_merge');
    expect(typed.pullRequest).toEqual({ title: 'PR 7', url: 'https://example.com/pr/7' });

    const bare = normalizeNodeData(makeNode({ nodeType: 'issue' })) as unknown as Record<
      string,
      unknown
    >;
    expect(bare.status).toBe('open');
  });

  it('converts a software project to its own typed shape, defaulting the status', () => {
    const node = makeNode({ nodeType: 'software-project' }) as unknown as Record<string, unknown>;
    node.checkoutPath = '/work/core';
    const typed = normalizeNodeData(node as unknown as Node) as unknown as Record<string, unknown>;
    expect(typed.nodeType).toBe('software-project');
    expect(typed.status).toBe('planning');
    expect(typed.checkoutPath).toBe('/work/core');
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
