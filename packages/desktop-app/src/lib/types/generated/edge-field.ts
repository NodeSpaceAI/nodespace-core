// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EnumValue } from './enum-value';
import type { SchemaFieldType } from './schema-field-type';

export type EdgeField = {
  name: string;
  type: SchemaFieldType;
  /**
   * The closed set of values an `enum` edge field admits, each with a display
   * label. Required on an `enum` field and rejected on any other type.
   *
   * Deliberately narrower than [`SchemaField`], which also carries
   * `user_values` and `extensible`: an edge enum is a fixed vocabulary. The
   * motivating case is an access-control role on an edge (owner/editor/viewer),
   * where a user-extensible value set would mean a permission level nothing
   * downstream knows how to check. Add the extensible half only if a real
   * use case for it appears.
   */
  coreValues?: Array<EnumValue>;
  indexed?: boolean;
  required?: boolean;
  default?: unknown;
  targetType?: string;
  description?: string;
};
