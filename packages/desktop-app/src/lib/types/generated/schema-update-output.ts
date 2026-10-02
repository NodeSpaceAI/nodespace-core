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
  /**
   * Plays affected by the change (present when `force` let it through).
   */
  affectedPlays?: Array<string>;
};
