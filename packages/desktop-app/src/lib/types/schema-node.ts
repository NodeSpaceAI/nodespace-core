/**
 * Type-Safe Schema Node
 *
 * Represents schema definitions with typed top-level fields.
 * This matches the Rust SchemaNode wire shape: the node envelope plus typed
 * top-level fields, NOT buried in properties.
 *
 * ## Serialized Structure
 *
 * The backend /api/schemas/:id endpoint returns SchemaNode with typed fields:
 * ```json
 * {
 *   "id": "task",
 *   "nodeType": "schema",
 *   "content": "Task",
 *   "createdAt": "2025-01-01T00:00:00Z",
 *   "modifiedAt": "2025-01-01T00:00:00Z",
 *   "version": 1,
 *   "lifecycleStatus": "active",
 *   "properties": {},
 *   "isCore": true,
 *   "schemaVersion": 1,
 *   "fields": [...]
 * }
 * ```
 *
 * Note: `description` is optional — it is sourced from child node text and may be absent.
 *
 * @example
 * ```typescript
 * // Type guard
 * if (isSchemaNode(node)) {
 *   console.log(`Core schema: ${node.isCore}`);
 *   console.log(`Has ${node.fields.length} fields`);
 * }
 * ```
 */

import type { NodeEnvelope } from './node';

/**
 * The closed vocabulary of schema field types. `text` is the string type: the
 * backend rejects `string`.
 */
export type SchemaFieldType =
  | 'text'
  | 'number'
  | 'boolean'
  | 'date'
  | 'datetime'
  | 'enum'
  | 'array'
  | 'object';

/**
 * Protection level for schema fields
 *
 * Determines whether a field can be modified or deleted by users.
 */
export type ProtectionLevel = 'core' | 'user' | 'system';

/**
 * Enum value with display label
 *
 * Provides human-readable labels for enum options displayed in UI/MCP clients.
 */
export interface EnumValue {
  /** The actual value stored in the database */
  value: string;

  /** Human-readable display label for UI/MCP clients */
  label: string;
}

/**
 * Definition of a single field in a schema
 *
 * Supports various field types including primitives, enums, arrays, and objects.
 * Enum fields can have protected core values and user-extensible values.
 */
export interface SchemaField {
  /** Field name (must be unique within schema): storage/query key, CEL
   *  selector, titleTemplate token. Changing it is a breaking change to
   *  every call site that references the field. */
  name: string;

  /** Display label shown in every UI surface (table/kanban headers, query
   *  editor, property forms). Always populated in storage — read it
   *  unconditionally via `labelForField()` (`$lib/utils/schema-field-label`),
   *  never with a fallback to `description` or a computed-from-`name`
   *  regex. Derived from `name` at the write boundary (create_schema/
   *  update_schema) when the caller omits it. */
  friendlyName: string;

  /** Field type */
  type: SchemaFieldType;

  /** Protection level determining mutability */
  protection: ProtectionLevel;

  /** Protected enum values (cannot be removed) - enum fields only */
  coreValues?: EnumValue[];

  /** User-extensible enum values (can be added/removed) - enum fields only */
  userValues?: EnumValue[];

  /** Whether this field should be indexed for faster queries */
  indexed: boolean;

  /** Whether this field is required (cannot be null/undefined) */
  required?: boolean;

  /** Whether enum values can be extended by users */
  extensible?: boolean;

  /** Default value for the field */
  default?: unknown;

  /** What the field is for: meaning, purpose, usage, an example where
   *  helpful. LLM-facing prose consumed by the agent for schema
   *  comprehension (schema retrieval embeds this text) — NOT rendered as a
   *  UI label. Prefer more detail over less; there is no UI-brevity cost to
   *  a longer description now that `friendlyName` carries the display
   *  label. */
  description?: string;

  /** For array fields, the type of items in the array */
  itemType?: SchemaFieldType;

  /** Sub-fields of an object field (field.type === 'object') */
  fields?: SchemaField[];

  /** Sub-fields of each object element in an array field (field.itemType === 'object') */
  itemFields?: SchemaField[];

  /**
   * Uniqueness hint: values are expected to be unique among active nodes of
   * the same type. Suggest-don't-block — never enforced as a write-time
   * constraint, surfaced via a read-only lookup (e.g. to flag a likely
   * duplicate before commit).
   */
  unique?: boolean;

  /**
   * Paired with `unique`, compares values case-insensitively (e.g. an email
   * is a claim, not an identity key, and casing should not distinguish two
   * otherwise-identical claims).
   */
  uniqueCaseInsensitive?: boolean;

  /**
   * Machine-bound: persisted and read locally like any other property, but never
   * included in a sync push and ignored if it arrives in a pull. Use when a value
   * denotes state on a particular machine (a resume handle, an absolute path, a
   * device id) or is not safe to transport as-is. Enforced by the sync engine.
   */
  localOnly?: boolean;
}

/**
 * Schema node with typed top-level fields
 *
 * This matches the Rust SchemaNode custom Serialize output.
 * Schema-specific fields are at the top level, NOT in properties.
 *
 * `id` is the schema's type id (e.g., "task", "person"); `properties` is empty,
 * because every stored property is a typed field below.
 */
export interface SchemaNode extends NodeEnvelope {
  /** Always "schema" for schema nodes. */
  nodeType: 'schema';

  // Schema-specific typed fields (NOT in properties)

  /** The parent schema id this type `extends`, when it is a subtype. A type has at most one parent. */
  extends?: string;

  /** An abstract type is never instantiated: no node is created with it as its `nodeType`.
   *  It stays a valid `extends` target and query scope. */
  abstract?: boolean;

  /** Whether this is a core (built-in) schema */
  isCore: boolean;

  /** Schema version (increments on schema changes) */
  schemaVersion: number;

  /** Human-readable schema description (sourced from child node subtree, may be absent) */
  description?: string;

  /** Array of field definitions */
  fields: SchemaField[];

  /** Optional template for computing display title from properties, e.g. "{first_name} {last_name}".
   *  When set, content is read-only and title is interpolated from schema properties. */
  titleTemplate?: string;

  /** Optional template for rendering a compact property summary inline below the node title.
   *  Uses `{field_name}` syntax. Evaluated client-side only — never persisted.
   *  Enum values resolve to labels; dates are human-formatted.
   *  Example: `"{status} · {company}"` → `"Active · Acme Corp"`. */
  propertiesHeaderSummaryTemplate?: string;
}

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
