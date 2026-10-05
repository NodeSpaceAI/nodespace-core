/**
 * Pure, immutable value transforms for nested (object/array) schema properties.
 *
 * These helpers never mutate their inputs — each returns a fresh value with the
 * requested change applied. They are deliberately store-agnostic so the recursive
 * editor can rebuild a whole top-level property value immutably from leaf edits,
 * and so the transforms are trivially unit-testable in isolation.
 */

import type { SchemaField } from '$lib/types/schema-node';
import { getEnumValues } from '$lib/utils/schema-enum-values';

/** Treat a possibly null/undefined value as an object record for safe spreading. */
function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/** Treat a possibly null/undefined value as an array for safe indexing. */
function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? (value as unknown[]) : [];
}

/**
 * Return a copy of the object `value` with `key` set to `newValue`.
 * A null/undefined/non-object `value` is treated as an empty object.
 */
export function setObjectKey(value: unknown, key: string, newValue: unknown): Record<string, unknown> {
  return { ...asRecord(value), [key]: newValue };
}

/**
 * Return a copy of the object `value` with `key` removed.
 * A null/undefined/non-object `value` yields an empty object.
 */
export function deleteObjectKey(value: unknown, key: string): Record<string, unknown> {
  const next = { ...asRecord(value) };
  delete next[key];
  return next;
}

/**
 * Return a copy of the array `value` with element at `index` replaced by `newValue`.
 * An out-of-range index leaves the array unchanged (defensive; the editor only
 * ever replaces indices it just rendered).
 */
export function replaceArrayIndex(value: unknown, index: number, newValue: unknown): unknown[] {
  const arr = [...asArray(value)];
  if (index < 0 || index >= arr.length) return arr;
  arr[index] = newValue;
  return arr;
}

/** Return a copy of the array `value` with the element at `index` removed. */
export function deleteArrayIndex(value: unknown, index: number): unknown[] {
  const arr = [...asArray(value)];
  if (index < 0 || index >= arr.length) return arr;
  arr.splice(index, 1);
  return arr;
}

/**
 * The array `value` after a control edits its scalar element at `index`, or
 * `null` when the edit changes nothing. An element has no empty state (it is
 * removed, never cleared), so a control reporting "no value" — a date picker
 * whose selected day is clicked again — leaves the array as it is.
 */
export function editArrayItem(value: unknown, index: number, item: unknown): unknown[] | null {
  if (item === null || item === undefined) return null;
  return replaceArrayIndex(value, index, item);
}

/** Return a copy of the array `value` with `item` appended. */
export function addArrayItem(value: unknown, item: unknown): unknown[] {
  return [...asArray(value), item];
}

/**
 * A sensible empty/default value for a leaf/nested field, used when adding an
 * array element or initializing a missing sub-value. Object → {}, array → [],
 * boolean → false, number → 0, text/enum → '', everything else → null.
 */
export function makeEmptyValueForField(field: SchemaField): unknown {
  switch (field.type) {
    case 'object':
      return {};
    case 'array':
      return [];
    case 'boolean':
      return false;
    case 'number':
      return 0;
    case 'text':
    case 'enum':
      return '';
    default:
      return null;
  }
}

/**
 * A sensible empty element for a new array item, based on the array field's
 * `itemType`. An `object` item type yields `{}`; a scalar item type yields the
 * empty value for that scalar (mirrors {@link makeEmptyValueForField}).
 *
 * The new item is saved as it is added, and an element of a typed array must
 * be of its item type, so the types with no empty value start from a real one:
 * a `date` from today, a `datetime` from now, an `enum` from its first value.
 */
export function makeEmptyArrayItem(field: SchemaField): unknown {
  switch (field.itemType) {
    case 'object':
      return {};
    case 'date': {
      const now = new Date();
      const month = String(now.getMonth() + 1).padStart(2, '0');
      const day = String(now.getDate()).padStart(2, '0');
      return `${now.getFullYear()}-${month}-${day}`;
    }
    case 'datetime':
      return new Date().toISOString();
    case 'enum':
      return getEnumValues(field)[0]?.value ?? '';
    default:
      // Reuse the scalar defaults by mapping itemType onto a synthetic field type.
      return makeEmptyValueForField({ ...field, type: field.itemType ?? 'text' });
  }
}

/** True when a field renders as a nested editor (object with sub-fields, or an array). */
export function isNestedField(field: SchemaField): boolean {
  return field.type === 'object' || field.type === 'array';
}

/**
 * Compact summary of a nested field's current value, shown on the trigger that
 * opens the nested editor: the element count for an array, the populated-key
 * count for an object. A missing/wrong-shaped value counts as zero.
 */
export function nestedFieldSummary(field: SchemaField, value: unknown): string {
  if (field.type === 'array') {
    const count = asArray(value).length;
    return `${count} ${count === 1 ? 'item' : 'items'}`;
  }
  const count = Object.keys(asRecord(value)).length;
  return `${count} ${count === 1 ? 'field' : 'fields'}`;
}

/**
 * Shift index-keyed open/collapse state (`item-<n>`) to follow array content
 * after the element at `removedIndex` is deleted: the removed key is dropped and
 * every higher index shifts down by one. Non-`item-<n>` keys are preserved.
 *
 * The nested editor keys both its array `{#each}` and its per-element expand
 * state by index; without this shift, deleting a middle element would leave the
 * wrong element appearing expanded.
 */
export function shiftItemOpenStateOnDelete(
  openState: Record<string, boolean>,
  removedIndex: number
): Record<string, boolean> {
  const next: Record<string, boolean> = {};
  for (const [key, isOpen] of Object.entries(openState)) {
    const match = key.match(/^item-(\d+)$/);
    if (!match) {
      next[key] = isOpen;
      continue;
    }
    const index = Number(match[1]);
    if (index === removedIndex) continue; // drop the removed element's state
    next[index > removedIndex ? `item-${index - 1}` : key] = isOpen;
  }
  return next;
}
