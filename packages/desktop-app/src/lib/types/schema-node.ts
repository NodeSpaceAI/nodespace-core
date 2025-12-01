/**
 * Type-Safe Schema Node
 *
 * Represents schema definitions with typed top-level fields.
 * This matches the Rust SchemaNode custom Serialize output - fields are at the top level,
 * NOT buried in properties.
 *
 * ## Serialized Structure
 *
 * The backend /api/schemas/:id endpoint returns SchemaNode with typed fields:
 * ```json
 * {
 *   "id": "task",
 *   "content": "Task",
 *   "createdAt": "2025-01-01T00:00:00Z",
 *   "modifiedAt": "2025-01-01T00:00:00Z",
 *   "version": 1,
 *   "isCore": true,
 *   "schemaVersion": 1,
 *   "description": "Task schema",
 *   "fields": [...]
 * }
 * ```
 *
 * Note: nodeType is NOT included in the response - it's implicit (always "schema").
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
  /** Field name (must be unique within schema) */
  name: string;

  /** Field type (e.g., "string", "number", "boolean", "enum", "array", "object") */
  type: string;

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

  /** Human-readable field description */
  description?: string;

  /** For array fields, the type of items in the array */
  itemType?: string;
}

/**
 * Schema node with typed top-level fields
 *
 * This matches the Rust SchemaNode custom Serialize output.
 * Schema-specific fields are at the top level, NOT in properties.
 *
 * **Important**: The `nodeType` field is optional because the /api/schemas/:id
 * endpoint may omit it (it's implicit - always "schema" for this endpoint).
 * Use `isSchemaNode()` for runtime validation which handles both cases.
 */
export interface SchemaNode {
  /** Unique identifier (same as schema type, e.g., "task", "person") */
  id: string;

  /** Always "schema" for schema nodes. Optional - may be omitted in API responses where it's implicit. */
  nodeType?: 'schema';

  /** Schema name (same as id) */
  content: string;

  /** Creation timestamp */
  createdAt: string;

  /** Last modification timestamp */
  modifiedAt: string;

  /** Node version for OCC */
  version: number;

  // Schema-specific typed fields (NOT in properties)

  /** Whether this is a core (built-in) schema */
  isCore: boolean;

  /** Schema version (increments on schema changes) */
  schemaVersion: number;

  /** Human-readable schema description */
  description: string;

  /** Array of field definitions */
  fields: SchemaField[];
}

/**
 * Type guard to check if a value is a SchemaNode
 *
 * Checks for the presence of schema-specific typed fields.
 * Note: Does NOT check nodeType because the /api/schemas/:id endpoint
 * returns SchemaNode without the nodeType field (it's implicit).
 *
 * @param value - Value to check
 * @returns True if value is a SchemaNode
 */
export function isSchemaNode(value: unknown): value is SchemaNode {
  if (!value || typeof value !== 'object') return false;
  const node = value as Record<string, unknown>;
  // Check for schema-specific fields (nodeType is implicit from endpoint)
  return (
    typeof node.isCore === 'boolean' &&
    typeof node.schemaVersion === 'number' &&
    Array.isArray(node.fields)
  );
}
