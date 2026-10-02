// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EnumValue } from './enum-value';

/**
 * One field's worth of `add_field_values`: the target field and the values
 * to append to its `userValues`.
 */
export type FieldValueAddition = {
  /**
   * Name of the existing field to extend (an `enum` field with
   * `extensible: true`).
   */
  field: string;
  /**
   * Values to append. Each `value` must not already exist among the
   * field's core and user values.
   */
  values: Array<EnumValue>;
};
