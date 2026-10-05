import { describe, it, expect } from 'vitest';
import {
  buildCreateNodeFields,
  buildProjectNodeUpdatePatch,
  buildTaskNodeUpdatePatch,
  encodeInsertPosition,
  normalizeChildrenTree,
  insertPosition,
  typedUpdateFieldNames,
  unknownTypedUpdateKeys,
} from '$lib/services/adapter-core';
import { writableTypedCoreKeys } from '$lib/types/typed-core-fields';

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

  it('names the keys of a request body that are not fields of the typed update', () => {
    expect(
      unknownTypedUpdateKeys('task', {
        version: 3,
        status: 'done',
        priority: null,
        dueDate: '2026-01-01'
      })
    ).toEqual([]);
    expect(
      unknownTypedUpdateKeys('task', {
        version: 3,
        content: 'Renamed',
        properties: { 'custom:x': 1 }
      })
    ).toEqual(['content', 'properties']);
    expect(unknownTypedUpdateKeys('person', { version: 1, firstName: 'Ada' })).toEqual([]);
    // A task field is not a person field, and an inherited Object key is not a field.
    expect(unknownTypedUpdateKeys('person', { dueDate: null, toString: 'x' })).toEqual([
      'dueDate',
      'toString'
    ]);
    expect(unknownTypedUpdateKeys('project', { endDate: null, content: 'Renamed' })).toEqual([
      'content'
    ]);
  });

  // The store sends a type's writable typed core keys; the dev-proxy and the
  // Tauri command refuse anything outside the update's fields. A key in one
  // list and not the other is a write the backend rejects.
  it.each(['task', 'person', 'project'] as const)(
    'the %s update fields are the keys the store sends for it',
    (nodeType) => {
      expect(typedUpdateFieldNames(nodeType).sort()).toEqual(
        [...writableTypedCoreKeys(nodeType)].sort()
      );
    }
  );

  it('carries the task schema fields only', () => {
    const patch = buildTaskNodeUpdatePatch({ status: 'done' });
    expect(Object.keys(patch).sort()).toEqual([
      'commits',
      'completedAt',
      'dueDate',
      'priority',
      'pullRequest',
      'startedAt',
      'status'
    ]);
  });

  it('encodes the pull request and the commit list as JSON values, null as a clear', () => {
    const link = { title: 'Add review status', url: 'https://example.com/pr/1' };
    const commits = [{ title: 'abc', url: 'https://example.com/c/abc' }];

    const set = buildTaskNodeUpdatePatch({ pullRequest: link, commits });
    expect(set.pullRequest).toEqual({ clear: false, valueJson: JSON.stringify(link) });
    expect(set.commits).toEqual({ clear: false, valueJson: JSON.stringify(commits) });

    const cleared = buildTaskNodeUpdatePatch({ pullRequest: null, commits: null });
    expect(cleared.pullRequest).toEqual({ clear: true, valueJson: '' });
    expect(cleared.commits).toEqual({ clear: true, valueJson: '' });

    const absent = buildTaskNodeUpdatePatch({ status: 'in_review' });
    expect(absent.pullRequest).toBeUndefined();
    expect(absent.commits).toBeUndefined();
  });

  it('encodes the project repository the same way', () => {
    const link = { title: 'nodespace-core', url: 'https://example.com/repo' };
    expect(buildProjectNodeUpdatePatch({ repository: link }).repository).toEqual({
      clear: false,
      valueJson: JSON.stringify(link)
    });
    expect(buildProjectNodeUpdatePatch({ repository: null }).repository).toEqual({
      clear: true,
      valueJson: ''
    });
    expect(buildProjectNodeUpdatePatch({ status: 'active' }).repository).toBeUndefined();
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
