// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The type of a schema field: the one field-type vocabulary, shared by the
 * wire type, the schema validator and the frontend (ADR-086 §7a).
 *
 * `text` is the string type. `string` is not a field type and is refused,
 * with a message that names `text`.
 */
export type SchemaFieldType =
  'text' | 'number' | 'boolean' | 'date' | 'datetime' | 'enum' | 'array' | 'object' | 'link';
