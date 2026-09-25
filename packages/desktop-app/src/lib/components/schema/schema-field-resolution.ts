/**
 * Field-value resolution for schema-driven UI (the generic properties form,
 * Kanban grouping, viewer field edits).
 *
 * A schema field lives in one of two places on a node, decided by the node's
 * type, never by the value's shape:
 *
 * - **Typed core field** — a core type's schema-declared field (`task.status`,
 *   `person.first_name`, `project.start_date`; see `TYPED_CORE_FIELDS`). It
 *   travels as a top-level typed field (`node.status`, `node.firstName`,
 *   `node.startDate`) and is written through the type's typed update.
 * - **Extension field** — every other field: a `custom:` field on a core type
 *   or any field of a user-defined type. It lives flat in `node.properties`.
 *
 * Extracted from generic-schema-form.svelte so it is unit-testable without
 * rendering the component.
 */

import type { Node } from '$lib/types';
import { typedCoreField } from '$lib/types/typed-core-fields';

export interface FieldValueSource {
  nodeType: string;
  properties?: Record<string, unknown>;
}

/**
 * Read a schema field's value.
 *
 * @returns the stored value, or `null` when the field is unset
 */
export function resolveFieldValue(node: FieldValueSource, fieldName: string): unknown {
  const typed = typedCoreField(node.nodeType, fieldName);
  if (typed) {
    return (node as unknown as Record<string, unknown>)[typed.wire] ?? null;
  }
  return node.properties?.[fieldName] ?? null;
}

/**
 * Build the `sharedNodeStore.updateNode` changes that write `fieldName = value`
 * to wherever `resolveFieldValue` reads it from.
 *
 * A typed core field becomes a top-level typed change (`{ startDate: … }`),
 * which the store routes through the type's typed update; an empty string is
 * sent as `null` (clear), since the typed updates validate their values and
 * `""` is not a valid date or enum value. An extension field becomes a flat
 * `properties` write that carries the node's other extension fields along —
 * the persistence queue keeps only a node's newest write, so a lone-field
 * patch could drop a queued sibling edit.
 */
export function buildFieldWrite(
  node: FieldValueSource,
  fieldName: string,
  value: unknown
): Partial<Node> {
  const typed = typedCoreField(node.nodeType, fieldName);
  if (typed) {
    return { [typed.wire]: value === '' ? null : value } as Partial<Node>;
  }
  return { properties: { ...(node.properties ?? {}), [fieldName]: value } };
}
