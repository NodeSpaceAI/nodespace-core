/**
 * Typed core fields per core node type — the frontend mirror of Rust's
 * `promoted_fields` (`packages/nodespace-types/src/convert.rs`).
 *
 * For `task`, `person` and `project`, the backend moves each schema-declared
 * core field out of `properties` to a top-level typed field: `due_date` is
 * stored in the `task` bucket and travels as `dueDate`. The frontend reads and
 * writes those fields only by their typed key; `properties` holds extension
 * fields alone.
 *
 * One registry serves every consumer that has to know the mapping:
 * - the dev-proxy's storage → wire conversion (`storageNodeToApiFields`),
 * - the store's typed-update routing (which `updateNode` changes are typed),
 * - schema-driven forms, which address fields by their schema (storage) name.
 *
 * Keep in sync with `promoted_fields` when a core type's field set changes.
 */

export interface TypedCoreField {
  /** Schema field name, as stored in the type's bucket (`due_date`). */
  storage: string;
  /** Top-level key on the typed node (`dueDate`). */
  wire: string;
  /** Dates are normalized to `YYYY-MM-DD` on read. */
  date?: boolean;
}

export const TYPED_CORE_FIELDS: Readonly<Record<string, readonly TypedCoreField[]>> = {
  task: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'due_date', wire: 'dueDate', date: true },
    { storage: 'started_at', wire: 'startedAt', date: true },
    { storage: 'completed_at', wire: 'completedAt', date: true }
  ],
  person: [
    { storage: 'first_name', wire: 'firstName' },
    { storage: 'last_name', wire: 'lastName' },
    { storage: 'email', wire: 'email' }
  ],
  project: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'start_date', wire: 'startDate', date: true },
    { storage: 'end_date', wire: 'endDate', date: true }
  ]
};

/**
 * Values the backend fills when a stored node has none — `task_node_to_value`
 * and `project_node_to_value` default `status`; nothing else is defaulted.
 */
export const TYPED_CORE_DEFAULTS: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  task: { status: 'open' },
  project: { status: 'planning' }
};

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
