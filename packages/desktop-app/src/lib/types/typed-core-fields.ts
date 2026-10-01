/**
 * Typed core field lookups.
 *
 * `TYPED_CORE_FIELDS` and `TYPED_CORE_DEFAULTS` are generated from Rust's
 * promoted fields (`./generated`, `core_promoted_fields` in
 * `packages/nodespace-types/src/convert.rs`).
 *
 * For `task`, `person`, `project` and `query`, the backend moves each
 * schema-declared core field out of `properties` to a top-level typed field:
 * `due_date` is stored in the `task` bucket and travels as `dueDate`. The
 * frontend reads and writes those fields only by their typed key; `properties`
 * holds extension fields alone.
 *
 * One table serves every consumer that has to know the mapping:
 * - the dev-proxy's storage → wire conversion (`storageNodeToApiFields`),
 * - the store's typed-update routing (which `updateNode` changes are typed),
 * - schema-driven forms, which address fields by their schema (storage) name.
 */

import { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS } from './generated';
import type { TypedCoreField } from './generated';

export { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS };
export type { TypedCoreField };

/** True when `nodeType` has typed core fields. */
export function hasTypedCoreFields(nodeType: string | undefined): boolean {
  return nodeType !== undefined && nodeType in TYPED_CORE_FIELDS;
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
