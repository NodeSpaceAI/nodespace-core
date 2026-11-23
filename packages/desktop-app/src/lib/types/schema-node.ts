/**
 * Schema node helpers.
 *
 * `SchemaNode`, `SchemaField` and their vocabulary are generated from Rust's
 * `nodespace-types` (`./generated`): a schema travels as the node envelope plus
 * typed top-level fields (`isCore`, `fields`, `relationships`, …), with an
 * empty `properties`, and `id` is the schema's type id (`task`, `person`).
 *
 * A field's `friendlyName` is its display label in every UI surface. It is
 * always populated in storage, so read it through `labelForField()`
 * (`$lib/utils/schema-field-label`), never with a fallback to `description` or
 * a label computed from `name`.
 */

import type { SchemaField, SchemaNode } from './generated';

export type {
  EdgeField,
  EnumValue,
  RelationshipCardinality,
  SchemaField,
  SchemaFieldType,
  SchemaNode,
  SchemaProtectionLevel,
  SchemaRelationship
} from './generated';

/**
 * Type guard to check if a value is a SchemaNode
 *
 * Checks for the presence of schema-specific typed fields.
 *
 * @param value - Value to check
 * @returns True if value is a SchemaNode
 */
export function isSchemaNode(value: unknown): value is SchemaNode {
  if (!value || typeof value !== 'object') return false;
  const node = value as Record<string, unknown>;
  // Check for schema-specific fields
  return (
    typeof node.isCore === 'boolean' &&
    typeof node.schemaVersion === 'number' &&
    Array.isArray(node.fields)
  );
}

/**
 * Every field a schema declares: the shipped `fields`, then the `customFields`
 * users added to a built-in type (ADR-112). A form, a table, a title template
 * and a filter read both lists by the same bare names.
 */
export function declaredFields(schema: SchemaNode | null | undefined): SchemaField[] {
  return [...(schema?.fields ?? []), ...(schema?.customFields ?? [])];
}
