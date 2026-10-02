/**
 * Typed core field lookups.
 *
 * `TYPED_CORE_FIELDS` and `TYPED_CORE_DEFAULTS` are generated from Rust's
 * promoted fields (`./generated`, `core_promoted_fields` in
 * `packages/nodespace-types/src/convert.rs`).
 *
 * For every core type with a typed wire shape (`task`, `skill`, the chat
 * subtypes, …), the backend moves each schema-declared core field out of
 * `properties` to a top-level typed field: `due_date` is stored in the `task`
 * bucket and travels as `dueDate`. The frontend reads those fields by their typed key, and
 * `properties` holds extension fields alone. A type with a typed update
 * command is written by typed key too; the chat family has none, so its
 * writes are `properties` patches keyed by the storage name (see
 * `hasTypedUpdate`).
 *
 * One table serves every consumer that has to know the mapping:
 * - the dev-proxy's storage → wire conversion (`storageNodeToApiFields`),
 * - the store's typed-update routing (which `updateNode` changes are typed),
 * - schema-driven forms, which address fields by their schema (storage) name.
 */

import { coreTypeEntry } from './core-node-types';
import { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS } from './generated';
import type { TypedCoreField } from './generated';

export { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS };
export type { TypedCoreField };

/**
 * True when `nodeType`'s typed fields are written through a typed backend
 * update (`updateTaskNode` and its siblings) rather than as a generic
 * `properties` patch. The registry says which types have one.
 */
export function hasTypedUpdate(nodeType: string | undefined): boolean {
  return coreTypeEntry(nodeType)?.typedUpdate === true;
}

/**
 * The typed core field a schema field name maps to on `nodeType`, or
 * `undefined` for an extension field that lives in `properties`.
 */
export function typedCoreField(nodeType: string, fieldName: string): TypedCoreField | undefined {
  return TYPED_CORE_FIELDS[nodeType]?.find((f) => f.storage === fieldName || f.wire === fieldName);
}

/** The typed (top-level) keys of `nodeType`, e.g. `['firstName', 'lastName', 'email']`. */
export function typedCoreKeys(nodeType: string): string[] {
  return (TYPED_CORE_FIELDS[nodeType] ?? []).map((f) => f.wire);
}

/** The typed keys a client may write — `typedCoreKeys` minus the read-only ones. */
export function writableTypedCoreKeys(nodeType: string): string[] {
  return (TYPED_CORE_FIELDS[nodeType] ?? []).filter((f) => !f.readOnly).map((f) => f.wire);
}
