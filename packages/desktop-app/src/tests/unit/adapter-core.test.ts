import { describe, it, expect } from 'vitest';
import {
  buildCreateNodeFields,
  normalizeChildrenTree,
  insertPosition,
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
    expect('collectionId' in fields).toBe(false);
  });

  it('carries the collection to file the new node into', () => {
    const fields = buildCreateNodeFields({
      id: 'n1',
      nodeType: 'text',
      content: '',
      collectionId: 'col-1',
    });
    expect(fields.collectionId).toBe('col-1');
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
