// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SchemaField } from './schema-field';
import type { SchemaRelationship } from './schema-relationship';

/**
 * Result of `create_schema`: what was persisted, read back from the store.
 */
export type CreateSchemaOutput = {
  /**
   * Id of the created schema (the normalized name).
   */
  schemaId: string;
  isCore: boolean;
  /**
   * Schema version.
   */
  version: number;
  /**
   * The description text written to the schema's child subtree.
   */
  description: string;
  /**
   * The fields the schema declares.
   */
  fields: Array<SchemaField>;
  /**
   * The schema id of the type the schema extends.
   */
  extends?: string;
  /**
   * The relationships the schema declares.
   */
  relationships?: Array<SchemaRelationship>;
  /**
   * E.g. a field name shadowing a reserved core property.
   */
  warnings?: Array<string>;
};
