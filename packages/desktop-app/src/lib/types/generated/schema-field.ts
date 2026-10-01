// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EnumValue } from './enum-value';
import type { SchemaFieldType } from './schema-field-type';
import type { SchemaProtectionLevel } from './schema-protection-level';

export type SchemaField = {
  /**
   * Unique key within the schema: storage/query key, CEL selector,
   * titleTemplate token. Changing it is a breaking change to every call
   * site that references the field.
   */
  name: string;
  /**
   * Display label shown in every UI surface (table/kanban headers, query
   * editor, property forms). Always populated in storage — every reader
   * uses it unconditionally, with no fallback to `description` and no
   * null-branching. Not required on input to `create_schema`/
   * `update_schema`: when omitted (empty string), the write boundary
   * derives it from `name` via [`derive_friendly_name`] before the field
   * is persisted.
   */
  friendlyName: string;
  type: SchemaFieldType;
  protection: SchemaProtectionLevel;
  coreValues?: Array<EnumValue>;
  userValues?: Array<EnumValue>;
  indexed: boolean;
  required?: boolean;
  extensible?: boolean;
  default?: unknown;
  /**
   * What the field is for: meaning, purpose, usage, an example where
   * helpful. Consumed by the model for schema comprehension (schema
   * retrieval embeds this text) — NOT rendered as a UI label. Prefer more
   * detail over less; there is no UI-brevity cost to a longer description
   * now that [`SchemaField::friendly_name`] carries the display label.
   */
  description?: string;
  /**
   * The element type of an `array` field, from the same vocabulary.
   */
  itemType?: SchemaFieldType;
  fields?: Array<SchemaField>;
  itemFields?: Array<SchemaField>;
  /**
   * Marks this field as a uniqueness hint: values are expected to be unique
   * among active nodes of the same type. This is a suggest-don't-block rule,
   * not an enforced constraint — writes are never rejected on a collision
   * (two offline devices can each validly create the same value). Uniqueness
   * is scoped per-database (ADR-053) and surfaced via a read-only lookup.
   */
  unique?: boolean;
  /**
   * When paired with `unique`, compares values case-insensitively (e.g. an
   * email is a claim, not an identity key, and casing should not distinguish
   * two otherwise-identical claims).
   */
  uniqueCaseInsensitive?: boolean;
  /**
   * Marks this property as machine-bound. A `localOnly` property is persisted
   * and read locally like any other — normal for local reads, writes, and the
   * UI — but is never included in a sync push, and is ignored if it arrives in
   * a pull. It survives its own device's restarts and is simply absent on other
   * devices (never a stale value from elsewhere). Use it when a value denotes
   * state on a particular machine, such that transporting it means nothing or
   * something false elsewhere (a resume handle, an absolute path, a device id,
   * a local port), or when the content is not safe to transport as-is. Enforced
   * by the sync engine, which consults this classification when building the
   * push payload and when applying a pull.
   */
  localOnly?: boolean;
};
