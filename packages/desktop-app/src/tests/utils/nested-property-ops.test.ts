/**
 * Unit tests for nested-property-ops — the pure, immutable value transforms that
 * back the recursive nested (object/array) schema-property editor.
 *
 * These cover the operations the editor relies on for correctness: setting and
 * deleting object keys (including at depth, via composition), replacing/deleting
 * array indices, appending items, and building empty values/elements for a field.
 * Every op must return a fresh value and never mutate its input.
 */

import { describe, it, expect, vi } from 'vitest';
import {
  setObjectKey,
  deleteObjectKey,
  replaceArrayIndex,
  deleteArrayIndex,
  addArrayItem,
  makeEmptyValueForField,
  makeEmptyArrayItem,
  editArrayItem,
  isNestedField,
  nestedFieldSummary,
  shiftItemOpenStateOnDelete
} from '$lib/utils/nested-property-ops';
import type { SchemaField } from '$lib/types/schema-node';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

describe('setObjectKey', () => {
  it('sets a key on an object without mutating the input', () => {
    const original = { a: 1 };
    const next = setObjectKey(original, 'b', 2);
    expect(next).toEqual({ a: 1, b: 2 });
    expect(original).toEqual({ a: 1 });
    expect(next).not.toBe(original);
  });

  it('overwrites an existing key', () => {
    expect(setObjectKey({ a: 1 }, 'a', 9)).toEqual({ a: 9 });
  });

  it('treats null/undefined/non-object as an empty object', () => {
    expect(setObjectKey(null, 'a', 1)).toEqual({ a: 1 });
    expect(setObjectKey(undefined, 'a', 1)).toEqual({ a: 1 });
    expect(setObjectKey(42, 'a', 1)).toEqual({ a: 1 });
    expect(setObjectKey(['x'], 'a', 1)).toEqual({ a: 1 });
  });

  it('composes to set a value at depth immutably', () => {
    const value = { address: { city: 'Denver' } };
    const inner = value.address;
    const next = setObjectKey(value, 'address', setObjectKey(inner, 'zip', '80202'));
    expect(next).toEqual({ address: { city: 'Denver', zip: '80202' } });
    // Original untouched at every level.
    expect(value).toEqual({ address: { city: 'Denver' } });
    expect(next.address).not.toBe(inner);
  });
});

describe('deleteObjectKey', () => {
  it('removes a key without mutating the input', () => {
    const original = { a: 1, b: 2 };
    const next = deleteObjectKey(original, 'a');
    expect(next).toEqual({ b: 2 });
    expect(original).toEqual({ a: 1, b: 2 });
  });

  it('is a no-op for a missing key', () => {
    expect(deleteObjectKey({ a: 1 }, 'z')).toEqual({ a: 1 });
  });

  it('deletes a key at depth via composition', () => {
    const value = { address: { city: 'Denver', zip: '80202' } };
    const next = setObjectKey(value, 'address', deleteObjectKey(value.address, 'zip'));
    expect(next).toEqual({ address: { city: 'Denver' } });
    expect(value.address).toEqual({ city: 'Denver', zip: '80202' });
  });
});

describe('replaceArrayIndex', () => {
  it('replaces an element without mutating the input', () => {
    const original = [1, 2, 3];
    const next = replaceArrayIndex(original, 1, 9);
    expect(next).toEqual([1, 9, 3]);
    expect(original).toEqual([1, 2, 3]);
  });

  it('replaces an object element in an array of objects', () => {
    const arr = [{ email: 'a@x.com' }, { email: 'b@x.com' }];
    const next = replaceArrayIndex(arr, 0, setObjectKey(arr[0], 'email', 'c@x.com'));
    expect(next).toEqual([{ email: 'c@x.com' }, { email: 'b@x.com' }]);
    expect(arr[0]).toEqual({ email: 'a@x.com' });
  });

  it('leaves the array unchanged for an out-of-range index', () => {
    expect(replaceArrayIndex([1, 2], 5, 9)).toEqual([1, 2]);
  });

  it('treats a non-array as empty', () => {
    expect(replaceArrayIndex(null, 0, 9)).toEqual([]);
  });
});

describe('deleteArrayIndex', () => {
  it('removes an element without mutating the input', () => {
    const original = ['a', 'b', 'c'];
    const next = deleteArrayIndex(original, 1);
    expect(next).toEqual(['a', 'c']);
    expect(original).toEqual(['a', 'b', 'c']);
  });

  it('is a no-op for an out-of-range index', () => {
    expect(deleteArrayIndex(['a'], 3)).toEqual(['a']);
  });
});

describe('addArrayItem', () => {
  it('appends an item without mutating the input', () => {
    const original = [1, 2];
    const next = addArrayItem(original, 3);
    expect(next).toEqual([1, 2, 3]);
    expect(original).toEqual([1, 2]);
  });

  it('treats null as an empty array', () => {
    expect(addArrayItem(null, 'x')).toEqual(['x']);
  });
});

describe('makeEmptyValueForField', () => {
  it('returns type-appropriate empties', () => {
    expect(makeEmptyValueForField(field({ name: 'o', type: 'object' }))).toEqual({});
    expect(makeEmptyValueForField(field({ name: 'a', type: 'array' }))).toEqual([]);
    expect(makeEmptyValueForField(field({ name: 'b', type: 'boolean' }))).toBe(false);
    expect(makeEmptyValueForField(field({ name: 'n', type: 'number' }))).toBe(0);
    expect(makeEmptyValueForField(field({ name: 's', type: 'text' }))).toBe('');
    expect(makeEmptyValueForField(field({ name: 't', type: 'text' }))).toBe('');
    expect(makeEmptyValueForField(field({ name: 'e', type: 'enum' }))).toBe('');
    expect(makeEmptyValueForField(field({ name: 'd', type: 'date' }))).toBeNull();
  });
});

describe('editArrayItem', () => {
  it('replaces the element at the index', () => {
    const original = ['2026-10-05', '2026-10-06'];
    expect(editArrayItem(original, 1, '2026-10-07')).toEqual(['2026-10-05', '2026-10-07']);
    expect(original).toEqual(['2026-10-05', '2026-10-06']);
  });

  it('keeps a falsy value that is still a value', () => {
    expect(editArrayItem([1], 0, 0)).toEqual([0]);
    expect(editArrayItem([true], 0, false)).toEqual([false]);
    expect(editArrayItem(['a'], 0, '')).toEqual(['']);
  });

  it('changes nothing when the control reports no value', () => {
    expect(editArrayItem(['2026-10-05'], 0, null)).toBeNull();
    expect(editArrayItem(['2026-10-05'], 0, undefined)).toBeNull();
  });
});

describe('makeEmptyArrayItem', () => {
  it('returns an empty object for an array of objects', () => {
    expect(makeEmptyArrayItem(field({ name: 'contacts', type: 'array', itemType: 'object' }))).toEqual({});
  });

  it('returns the scalar empty for an array of scalars', () => {
    expect(makeEmptyArrayItem(field({ name: 'tags', type: 'array', itemType: 'text' }))).toBe('');
    expect(makeEmptyArrayItem(field({ name: 'counts', type: 'array', itemType: 'number' }))).toBe(0);
  });

  it('starts a date or datetime item from the current moment, never null', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(2026, 9, 5, 14, 30, 0));
      expect(makeEmptyArrayItem(field({ name: 'days', type: 'array', itemType: 'date' }))).toBe('2026-10-05');
      expect(makeEmptyArrayItem(field({ name: 'visits', type: 'array', itemType: 'datetime' }))).toBe(
        new Date(2026, 9, 5, 14, 30, 0).toISOString()
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it('starts an enum item from the first declared value', () => {
    const tiers = { name: 'tiers', type: 'array', itemType: 'enum' } as const;
    expect(
      makeEmptyArrayItem(
        field({
          ...tiers,
          coreValues: [{ value: 'gold', label: 'Gold' }],
          userValues: [{ value: 'tin', label: 'Tin' }]
        })
      )
    ).toBe('gold');
    expect(makeEmptyArrayItem(field({ ...tiers, userValues: [{ value: 'tin', label: 'Tin' }] }))).toBe('tin');
    expect(makeEmptyArrayItem(field(tiers))).toBe('');
  });

  it('defaults a missing itemType to an empty string', () => {
    expect(makeEmptyArrayItem(field({ name: 'x', type: 'array' }))).toBe('');
  });
});

describe('isNestedField', () => {
  it('is true only for object and array fields', () => {
    expect(isNestedField(field({ name: 'o', type: 'object' }))).toBe(true);
    expect(isNestedField(field({ name: 'a', type: 'array' }))).toBe(true);
    expect(isNestedField(field({ name: 's', type: 'text' }))).toBe(false);
    expect(isNestedField(field({ name: 'e', type: 'enum' }))).toBe(false);
  });
});

describe('nestedFieldSummary', () => {
  const objectField = field({ name: 'address', type: 'object' });
  const arrayField = field({ name: 'contacts', type: 'array' });

  it('counts populated keys for an object field, singular at one', () => {
    expect(nestedFieldSummary(objectField, {})).toBe('0 fields');
    expect(nestedFieldSummary(objectField, { street: '1 Main' })).toBe('1 field');
    expect(nestedFieldSummary(objectField, { street: '1 Main', city: 'Denver' })).toBe('2 fields');
  });

  it('counts elements for an array field, singular at one', () => {
    expect(nestedFieldSummary(arrayField, [])).toBe('0 items');
    expect(nestedFieldSummary(arrayField, ['a'])).toBe('1 item');
    expect(nestedFieldSummary(arrayField, ['a', 'b'])).toBe('2 items');
  });

  it('treats a missing or wrong-shaped value as empty', () => {
    expect(nestedFieldSummary(objectField, null)).toBe('0 fields');
    expect(nestedFieldSummary(objectField, undefined)).toBe('0 fields');
    expect(nestedFieldSummary(objectField, ['a'])).toBe('0 fields');
    expect(nestedFieldSummary(arrayField, null)).toBe('0 items');
    expect(nestedFieldSummary(arrayField, { a: 1 })).toBe('0 items');
  });
});

describe('shiftItemOpenStateOnDelete', () => {
  it('drops the removed index and shifts higher indices down', () => {
    // Items [0,1,2]; elements 1 and 2 expanded. Delete index 0 → old 1→0, 2→1.
    const open = { 'item-1': true, 'item-2': true };
    expect(shiftItemOpenStateOnDelete(open, 0)).toEqual({ 'item-0': true, 'item-1': true });
  });

  it('drops the removed index itself and leaves lower indices untouched', () => {
    const open = { 'item-0': true, 'item-1': true, 'item-2': true };
    expect(shiftItemOpenStateOnDelete(open, 1)).toEqual({ 'item-0': true, 'item-1': true });
  });

  it('preserves non-item keys and does not mutate the input', () => {
    const open = { 'item-0': true, other: false };
    const result = shiftItemOpenStateOnDelete(open, 0);
    expect(result).toEqual({ other: false });
    expect(open).toEqual({ 'item-0': true, other: false }); // unchanged
  });
});
