// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A single field rename within `update_schema`.
 *
 * Two different renames share this shape:
 * - **identity rename** (`from` != `to`): rekeys `name`, migrates every
 *   existing node's property data, and breaks `titleTemplate`, CEL and
 *   query-filter references to the old name.
 * - **display rename** (`from` == `to`, `friendlyName` set): changes only the
 *   display label and migrates nothing. Both may be combined in one entry.
 */
export type FieldRename = {
  /**
   * Current field name.
   */
  from: string;
  /**
   * New field name (the same value as `from` for a display-only rename).
   */
  to: string;
  /**
   * New display label. Omit to leave `friendlyName` exactly as stored,
   * including when it was derived from the old `name`.
   */
  friendlyName?: string;
};
