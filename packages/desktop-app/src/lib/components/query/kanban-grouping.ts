/**
 * Pure helpers for the Kanban view (query-node-viewer).
 *
 * Kept DOM-free and side-effect-free so the grouping / eligibility / write-shape
 * / per-column reveal-set rules can be unit-tested directly, following the
 * project convention of testing extracted logic rather than rendering Svelte
 * components.
 */

import type { Node } from '$lib/types';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import {
  buildFieldWrite,
  resolveFieldValue
} from '$lib/components/schema/schema-field-resolution';

/** Column key used for nodes whose group-by value is unset or unrecognized. */
export const UNASSIGNED = '__unassigned__';

/** A Kanban column derived from an enum field value. */
export interface KanbanColumn {
  /** The stored enum value that defines this column. */
  value: string;
  /** Human-readable label for the column header. */
  label: string;
}

/**
 * Fields a Kanban board can group by.
 *
 * Enum-only: enum fields carry `coreValues`/`userValues`, which give both the
 * complete column set and the display labels. Non-enum fields have unbounded
 * value sets, so columns could only be inferred from values that happen to
 * exist and dragging could not offer a valid target set.
 *
 * Also excludes an enum field if any of its values is literally `UNASSIGNED`
 * (`"__unassigned__"`) — the internal sentinel `groupByColumn` below uses for
 * the "no value" bucket. Nothing stops a schema author from choosing that
 * exact string as a real enum value, but if one did, that value and every
 * genuinely-unset node would collide into the same bucket (indistinguishable,
 * and unreachable by name — the display column is always labeled
 * "Unassigned", never that value's own label) and `displayColumns` would carry
 * two entries with the same key. Rather than a bucketing scheme that has to
 * reconcile that collision, the simpler and safer rule is: a field that could
 * produce it isn't offered as a Kanban grouping choice at all — this field's
 * other values are still visible via List/Table, just not this board.
 */
export function eligibleGroupByFields(schema: SchemaNode | null): SchemaField[] {
  return (schema?.fields ?? []).filter(
    (f) =>
      f.type === 'enum' &&
      !(f.coreValues ?? []).some((v) => v.value === UNASSIGNED) &&
      !(f.userValues ?? []).some((v) => v.value === UNASSIGNED)
  );
}

/**
 * The ordered set of columns for an enum field: its core values followed by its
 * user-extensible values, each mapped to `{ value, label }`.
 */
export function enumColumns(field: SchemaField | undefined | null): KanbanColumn[] {
  if (!field) return [];
  const all = [...(field.coreValues ?? []), ...(field.userValues ?? [])];
  return all.map((ev) => ({ value: ev.value, label: ev.label }));
}

/**
 * Read a node's value for the given field — the same resolution every
 * schema-driven surface uses (`resolveFieldValue`): a typed core field from
 * its top-level typed key, any other field from `properties`. Returns `null`
 * for unset/empty values.
 */
export function readGroupValue(node: Node, field: string): string | null {
  const raw = resolveFieldValue(node, field);
  if (raw === null || raw === undefined || raw === '') return null;
  return String(raw);
}

/**
 * Build the `updateNode` change-set that moves a node into the column identified
 * by `value`, writing the value to wherever `readGroupValue` reads it from
 * (`buildFieldWrite`), so the card re-groups consistently after the store
 * update: a typed core field persists through its type's typed update, any
 * other field as a `properties` write.
 *
 * `value` is `null` for a move to Unassigned — written through as a genuine
 * `null` (clear), not an empty string. Not every field has clear semantics
 * on the backend: a `required` field (task's and project's `status`) has no
 * cleared state, so the caller must never offer Unassigned as a target for
 * one (kanban-view.svelte's `displayColumns` and `moveCard` both guard on
 * `activeField?.required`).
 */
export function resolveFieldWrite(
  node: Node,
  field: string,
  value: string | null
): Partial<Node> {
  return buildFieldWrite(node, field, value);
}

/**
 * Bucket `{ id, value }` items into columns. Every column value gets an entry
 * (even if empty), plus a trailing `UNASSIGNED` bucket for items whose value is
 * `null` or does not match any column. Column order is preserved.
 */
export function groupByColumn(
  items: Array<{ id: string; value: string | null }>,
  columnValues: string[]
): Map<string, string[]> {
  const valid = new Set(columnValues);
  const buckets = new Map<string, string[]>();
  for (const cv of columnValues) buckets.set(cv, []);
  buckets.set(UNASSIGNED, []);

  for (const item of items) {
    const key = item.value !== null && valid.has(item.value) ? item.value : UNASSIGNED;
    buckets.get(key)!.push(item.id);
  }
  return buckets;
}

/**
 * Pick the group-by field to use: the stored selection if it is still an
 * eligible field, otherwise `null`. An arbitrary eligible field is
 * deliberately NOT offered as a fallback — a board grouped by a field the
 * user never chose would be meaningless for the type, so an unset selection
 * means "no board yet", not "the first field found". The caller
 * (kanban-view.svelte) renders the group-by picker with a prompt and
 * withholds the columns until the user actually picks one.
 */
export function resolveActiveGroupBy(
  eligible: SchemaField[],
  stored: string | undefined
): string | null {
  if (stored && eligible.some((f) => f.name === stored)) return stored;
  return null;
}

/**
 * Grow a column's revealed-id set by up to `batch` more ids, in `ids` order,
 * preserving every id already in `revealed` regardless of where it now sits
 * in `ids`. Used to bound Kanban's per-column render (a "+N more" control)
 * without List/Table's flip-page pagination: capping by *position* alone
 * (`ids.slice(0, n)`) can't guarantee an already-shown card stays shown,
 * because a different card joining the column ahead of it in `ids` order
 * would push it past a plain positional cutoff — exactly the "card vanishes
 * out from under an in-progress drag" failure this exists to avoid. Tracking
 * by id instead means a card, once revealed, stays revealed for as long as
 * it remains in this column, independent of how the column's order churns.
 */
export function growRevealed(
  revealed: ReadonlySet<string>,
  ids: string[],
  batch: number
): Set<string> {
  const next = new Set(revealed);
  let added = 0;
  for (const id of ids) {
    if (added >= batch) break;
    if (!next.has(id)) {
      next.add(id);
      added++;
    }
  }
  return next;
}
