// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { StructuredShape } from './structured-shape';

/**
 * One core field the backend moves out of `properties` to a top-level typed
 * field: `due_date` is stored in the `task` bucket and travels as `dueDate`.
 */
export type TypedCoreField = {
  /**
   * Schema field name, as stored in the type's bucket (`due_date`).
   */
  storage: string;
  /**
   * Top-level key on the typed node (`dueDate`).
   */
  wire: string;
  /**
   * Dates are normalized to `YYYY-MM-DD` on read.
   */
  date?: boolean;
  /**
   * Not a string: promoted as stored when it has this JSON shape, dropped
   * otherwise, as the Rust decoder drops a malformed field to its default.
   */
  structured?: StructuredShape;
  /**
   * System-managed: read on the wire, never sent in a typed update.
   */
  readOnly?: boolean;
};
