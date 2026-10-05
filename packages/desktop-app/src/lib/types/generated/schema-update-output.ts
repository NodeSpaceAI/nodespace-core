// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Result of `update_schema`.
 */
export type SchemaUpdateOutput = {
  schemaId: string;
  success: boolean;
  fieldsAdded?: number;
  fieldsRemoved?: number;
  fieldsRenamed?: number;
  fieldValuesAdded?: number;
  relationshipsAdded?: number;
  relationshipsRemoved?: number;
  contextPathsAdded?: number;
  contextPathsRemoved?: number;
  /**
   * Every context path, on any schema, that names a relationship that no
   * longer resolves, each as `"<schema id>: <path>"`. Looked for after an
   * update that took a relationship away, and present when there is one. A
   * context read of that type fails until the path is removed or the
   * relationship is declared again.
   */
  strandedContextPaths?: Array<string>;
  /**
   * Plays affected by the change (present when `force` let it through).
   */
  affectedPlays?: Array<string>;
};
