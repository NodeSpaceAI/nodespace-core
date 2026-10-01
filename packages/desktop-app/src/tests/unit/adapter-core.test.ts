import { describe, it, expect } from 'vitest';
import {
  buildCreateNodeFields,
  buildTaskNodeUpdatePatch,
  encodeInsertPosition,
  normalizeChildrenTree,
  insertPosition,
  unknownTaskNodeUpdateKeys,
} from '$lib/services/adapter-core';

describe('adapter-core: buildCreateNodeFields', () => {
  it('defaults optional fields for a minimal input', () => {
    const fields = buildCreateNodeFields({ id: 'n1', nodeType: 'text', content: 'hi' });
    expect(fields).toEqual({
      id: 'n1',
      nodeType: 'text',
      content: 'hi',
      properties: {},
      mentions: [],
      parentId: null,
      insertPosition: null,
    });
  });

  it('preserves an explicit parentId and insertPosition', () => {
    const fields = buildCreateNodeFields({
      id: 'n1',
      nodeType: 'text',
      content: 'hi',
      parentId: 'parent-1',
      insertPosition: insertPosition.after('sibling-1'),
    });
    expect(fields.parentId).toBe('parent-1');
    expect(fields.insertPosition).toEqual({ type: 'after', siblingId: 'sibling-1' });
  });
});

describe('adapter-core: buildTaskNodeUpdatePatch (tri-state clearable encoding)', () => {
  it('omits a field entirely when absent from the update (no change)', () => {
    const patch = buildTaskNodeUpdatePatch({ status: 'done' });
    expect(patch.priority).toBeUndefined();
    expect(patch.dueDate).toBeUndefined();
  });

  it('encodes null as an explicit clear', () => {
    const patch = buildTaskNodeUpdatePatch({ dueDate: null });
    expect(patch.dueDate).toEqual({ clear: true });
  });

  it('encodes a value as an explicit set', () => {
    const patch = buildTaskNodeUpdatePatch({ dueDate: '2026-01-01T00:00:00Z' });
    expect(patch.dueDate).toEqual({ clear: false, value: '2026-01-01T00:00:00Z' });
  });

  it('passes status straight through (no clear semantics)', () => {
    const patch = buildTaskNodeUpdatePatch({ status: 'in_progress' });
    expect(patch.status).toBe('in_progress');
  });

  it('names the keys of a request body that are not task update fields', () => {
    expect(
      unknownTaskNodeUpdateKeys({ version: 3, status: 'done', priority: null, dueDate: '2026-01-01' })
    ).toEqual([]);
    expect(
      unknownTaskNodeUpdateKeys({ version: 3, content: 'Renamed', properties: { 'custom:x': 1 } })
    ).toEqual(['content', 'properties']);
  });

  it('carries the task schema fields only', () => {
    const patch = buildTaskNodeUpdatePatch({ status: 'done' });
    expect(Object.keys(patch).sort()).toEqual([
      'completedAt',
      'dueDate',
      'priority',
      'startedAt',
      'status'
    ]);
  });

  it('treats a null priority the same as any other clearable field', () => {
    const patch = buildTaskNodeUpdatePatch({ priority: null });
    expect(patch.priority).toEqual({ clear: true });
  });
});

describe('adapter-core: encodeInsertPosition', () => {
  it('encodes beginning/end/after to the proto oneof shape', () => {
    expect(encodeInsertPosition(insertPosition.beginning())).toEqual({ beginning: true });
    expect(encodeInsertPosition(insertPosition.end())).toEqual({ end: true });
    expect(encodeInsertPosition(insertPosition.after('sib'))).toEqual({ after: 'sib' });
  });

  it('encodes null/undefined as the unset oneof (empty object)', () => {
    expect(encodeInsertPosition(null)).toEqual({});
    expect(encodeInsertPosition(undefined)).toEqual({});
  });
});

describe('adapter-core: normalizeChildrenTree', () => {
  it('normalizes an empty object (non-existent parent) to null', () => {
    expect(normalizeChildrenTree({})).toBeNull();
    expect(normalizeChildrenTree(null)).toBeNull();
    expect(normalizeChildrenTree(undefined)).toBeNull();
  });

  it('passes through a populated tree unchanged', () => {
    const tree = { id: 'n1', nodeType: 'text', content: '', version: 1, createdAt: '', modifiedAt: '', children: [] };
    expect(normalizeChildrenTree(tree)).toBe(tree);
  });
});
