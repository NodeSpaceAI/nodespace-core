/**
 * Schema Management Types
 *
 * TypeScript definitions for the schema system. These types mirror the Rust
 * backend schema definitions exactly to ensure type safety across the stack.
 *
 * ## Schema Protection Levels
 *
 * - `core`: Cannot be modified or deleted (UI components depend on these fields)
 * - `user`: Fully modifiable/deletable by users
 * - `system`: Auto-managed internal fields, read-only
 *
 * @see packages/core/src/models/schema.rs - Rust definitions
 */

/**
 * Protection level for schema fields
 *
 * Determines whether a field can be modified or deleted by users.
 */
export type ProtectionLevel = 'core' | 'user' | 'system';

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
  coreValues?: string[];

  /** User-extensible enum values (can be added/removed) - enum fields only */
  userValues?: string[];

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
 * Complete schema definition
 *
 * Stored as a node with `node_type = "schema"` and `id = type_name`.
 */
export interface SchemaDefinition {
  /** Whether this is a core schema (protected) */
  isCore: boolean;

  /** Schema version (increments on changes) */
  version: number;

  /** Human-readable schema description */
  description: string;

  /** Array of field definitions */
  fields: SchemaField[];
}

/**
 * Configuration for adding a new field to a schema
 *
 * Used when calling `SchemaService.addField()`. Only user-protected
 * fields can be added through the schema service.
 */
export interface AddFieldConfig {
  /** Field name */
  fieldName: string;

  /** Field type (e.g., "string", "number", "boolean", "enum", "array") */
  fieldType: string;

  /** Whether this field should be indexed */
  indexed?: boolean;

  /** Whether this field is required */
  required?: boolean;

  /** Default value for the field */
  default?: unknown;

  /** Field description */
  description?: string;

  /** For array fields, the type of items in the array */
  itemType?: string;

  /** For enum fields, the allowed values (added to user_values) */
  enumValues?: string[];

  /** For enum fields, whether users can extend with more values */
  extensible?: boolean;
}

/**
 * Result of adding a field to a schema
 */
export interface AddFieldResult {
  /** ID of the modified schema */
  schemaId: string;

  /** New schema version after adding the field */
  newVersion: number;

  /** Operation success flag */
  success: boolean;
}

/**
 * Result of removing a field from a schema
 */
export interface RemoveFieldResult {
  /** ID of the modified schema */
  schemaId: string;

  /** New schema version after removing the field */
  newVersion: number;

  /** Operation success flag */
  success: boolean;
}

/**
 * Result of extending an enum field
 */
export interface ExtendEnumResult {
  /** ID of the modified schema */
  schemaId: string;

  /** New schema version after extending the enum */
  newVersion: number;

  /** Operation success flag */
  success: boolean;
}

/**
 * Result of removing an enum value
 */
export interface RemoveEnumValueResult {
  /** ID of the modified schema */
  schemaId: string;

  /** New schema version after removing the value */
  newVersion: number;

  /** Operation success flag */
  success: boolean;
}

/**
 * Custom error for schema operations
 */
export class SchemaOperationError extends Error {
  constructor(
    message: string,
    public readonly schemaId: string,
    public readonly operation: string,
    public readonly originalError?: unknown
  ) {
    super(message);
    this.name = 'SchemaOperationError';

    // Maintain proper stack trace (only available in V8)
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, SchemaOperationError);
    }
  }
}
