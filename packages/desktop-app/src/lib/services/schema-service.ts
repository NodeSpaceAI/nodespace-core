/**
 * Schema Service - TypeScript Wrapper for Schema Management
 *
 * Provides a type-safe TypeScript wrapper around the Rust backend's schema
 * management operations. Enables frontend components to manage user-defined
 * entity schemas with full type safety.
 *
 * ## Features
 *
 * - Add/remove user fields from schemas
 * - Extend/remove enum values (user_values only)
 * - Get complete schema definitions
 * - Protection level enforcement (core fields protected)
 * - User-friendly error messages
 * - LRU cache for schema definitions (reduces redundant backend calls)
 *
 * ## Usage Example
 *
 * ```typescript
 * import { createSchemaService } from '$lib/services/schema-service';
 *
 * const schemaService = createSchemaService();
 *
 * async function addPriorityField() {
 *   await schemaService.addField('task', {
 *     fieldName: 'priority',
 *     fieldType: 'number',
 *     indexed: false,
 *     required: false,
 *     default: 0,
 *     description: 'Task priority level'
 *   });
 * }
 *
 * async function loadTaskSchema() {
 *   const schema = await schemaService.getSchema('task');
 *   console.log('Task schema version:', schema.version);
 * }
 * ```
 *
 * ## Architecture
 *
 * - Frontend Service (this file) → BackendAdapter (Tauri IPC or HTTP) → SchemaService → NodeService → Database
 * - Uses BackendAdapter pattern for Tauri and HTTP mode support
 * - All operations enforce protection levels
 * - Schema version automatically incremented on changes
 * - LRU cache with max 50 schemas (auto-invalidation on mutations)
 *
 * @see packages/core/src/services/schema_service.rs - Rust implementation
 * @see packages/desktop-app/src/lib/services/backend-adapter.ts - Backend adapter pattern
 */

import { getBackendAdapter, type BackendAdapter } from './backend-adapter';
import {
  SchemaOperationError,
  type SchemaDefinition,
  type AddFieldConfig,
  type AddFieldResult,
  type RemoveFieldResult,
  type ExtendEnumResult,
  type RemoveEnumValueResult
} from '$lib/types/schema';

/**
 * Helper function to convert backend errors to user-friendly messages
 */
function formatSchemaError(error: unknown, operation: string, schemaId: string): string {
  if (typeof error === 'string') {
    return error;
  }

  if (error instanceof Error) {
    // Extract meaningful error messages from backend errors
    const message = error.message;

    // Common error patterns with user-friendly translations
    if (message.includes('not found')) {
      return `Schema '${schemaId}' not found. Please verify the schema ID.`;
    }

    if (message.includes('already exists')) {
      return `Field already exists in schema '${schemaId}'. Field names must be unique.`;
    }

    if (message.includes('protection')) {
      return `Cannot ${operation} protected field. Only user-defined fields can be modified.`;
    }

    if (message.includes('core value')) {
      return `Cannot remove core enum values. Only user-added values can be removed.`;
    }

    if (message.includes('not an enum')) {
      return `Field is not an enum type. Enum operations only work on enum fields.`;
    }

    if (message.includes('not extensible')) {
      return `Enum field is not extensible. Cannot add new values to this enum.`;
    }

    return message;
  }

  return `Unknown error during ${operation} operation on schema '${schemaId}'`;
}

/**
 * Simple LRU Cache for schema definitions
 *
 * Caches schema definitions to reduce redundant backend calls.
 * Automatically evicts least recently used schemas when max size reached.
 */
class LRUCache<K, V> {
  private cache = new Map<K, V>();
  private maxSize: number;

  constructor(maxSize = 50) {
    this.maxSize = maxSize;
  }

  get(key: K): V | undefined {
    if (!this.cache.has(key)) return undefined;

    // Move to end (most recently used)
    const value = this.cache.get(key)!;
    this.cache.delete(key);
    this.cache.set(key, value);
    return value;
  }

  set(key: K, value: V): void {
    // Delete if exists (to reinsert at end)
    if (this.cache.has(key)) {
      this.cache.delete(key);
    }

    // Evict oldest if at capacity
    if (this.cache.size >= this.maxSize) {
      const firstKey = this.cache.keys().next().value;
      if (firstKey !== undefined) {
        this.cache.delete(firstKey);
      }
    }

    this.cache.set(key, value);
  }

  delete(key: K): void {
    this.cache.delete(key);
  }

  clear(): void {
    this.cache.clear();
  }
}

/**
 * Schema Service class
 *
 * Wraps Rust backend schema operations with TypeScript interface
 */
export class SchemaService {
  private adapter: BackendAdapter;
  private schemaCache: LRUCache<string, SchemaDefinition>;

  constructor(adapter?: BackendAdapter) {
    this.adapter = adapter ?? getBackendAdapter();
    this.schemaCache = new LRUCache(50);
  }

  /**
   * Get all schema definitions
   *
   * Retrieves all registered schemas (both core and custom) for plugin auto-registration.
   *
   * @returns Array of all schema definitions with their IDs
   * @throws {SchemaOperationError} If retrieval fails
   *
   * @example
   * ```typescript
   * const schemas = await service.getAllSchemas();
   * schemas.forEach(({ id, isCore }) => {
   *   console.log(`Schema ${id} is ${isCore ? 'core' : 'custom'}`);
   * });
   * ```
   */
  async getAllSchemas(): Promise<Array<SchemaDefinition & { id: string }>> {
    try {
      // Adapter handles Tauri vs HTTP automatically and performs case conversion
      const schemas = await this.adapter.getAllSchemas();

      // Cache all schemas
      for (const schema of schemas) {
        this.schemaCache.set(schema.id, schema);
      }

      return schemas;
    } catch (error) {
      const message = formatSchemaError(error, 'get all', 'schemas');
      throw new SchemaOperationError(message, 'all', 'getAllSchemas', error);
    }
  }

  /**
   * Get a schema definition by schema ID
   *
   * @param schemaId - Schema ID (e.g., "task", "person")
   * @returns Complete schema definition with fields and metadata
   * @throws {SchemaOperationError} If schema not found or retrieval fails
   *
   * @example
   * ```typescript
   * const schema = await service.getSchema('task');
   * console.log(`Task schema version ${schema.version} has ${schema.fields.length} fields`);
   * ```
   */
  async getSchema(schemaId: string): Promise<SchemaDefinition> {
    // Validate input
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'get');
    }

    // Check cache first
    const cached = this.schemaCache.get(schemaId);
    if (cached) {
      return cached;
    }

    try {
      // Adapter handles Tauri vs HTTP automatically and performs case conversion
      const schema = await this.adapter.getSchema(schemaId);

      // Cache the result
      this.schemaCache.set(schemaId, schema);

      return schema;
    } catch (error) {
      const message = formatSchemaError(error, 'get', schemaId);
      throw new SchemaOperationError(message, schemaId, 'get', error);
    }
  }

  /**
   * Add a new field to a schema
   *
   * Only user-protected fields can be added. Core and system fields cannot be
   * added through the schema service. The schema version is automatically incremented.
   *
   * @param schemaId - Schema ID to modify (e.g., "task")
   * @param config - Field configuration
   * @returns Result with new schema version
   * @throws {SchemaOperationError} If field already exists or validation fails
   *
   * @example
   * ```typescript
   * // Add a priority field to task schema
   * const result = await service.addField('task', {
   *   fieldName: 'priority',
   *   fieldType: 'number',
   *   indexed: false,
   *   required: false,
   *   default: 0,
   *   description: 'Task priority level (0-10)'
   * });
   * console.log(`Schema updated to version ${result.newVersion}`);
   * ```
   */
  async addField(schemaId: string, config: AddFieldConfig): Promise<AddFieldResult> {
    // Validate input
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'addField');
    }
    if (!config.fieldName || config.fieldName.trim() === '') {
      throw new SchemaOperationError('Field name cannot be empty', schemaId, 'addField');
    }
    if (!config.fieldType || config.fieldType.trim() === '') {
      throw new SchemaOperationError('Field type cannot be empty', schemaId, 'addField');
    }

    try {
      const result = await this.adapter.addSchemaField(schemaId, config);

      // Invalidate cache since schema was modified
      this.schemaCache.delete(schemaId);

      return result;
    } catch (error) {
      const message = formatSchemaError(error, 'add field', schemaId);
      throw new SchemaOperationError(message, schemaId, 'addField', error);
    }
  }

  /**
   * Remove a field from a schema
   *
   * Only user-protected fields can be removed. Core and system fields are
   * protected and cannot be removed. The schema version is automatically incremented.
   *
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the field to remove
   * @returns Result with new schema version
   * @throws {SchemaOperationError} If field is protected or not found
   *
   * @example
   * ```typescript
   * // Remove a custom field
   * const result = await service.removeField('task', 'priority');
   * console.log(`Field removed, schema now at version ${result.newVersion}`);
   * ```
   */
  async removeField(schemaId: string, fieldName: string): Promise<RemoveFieldResult> {
    // Validate input
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'removeField');
    }
    if (!fieldName || fieldName.trim() === '') {
      throw new SchemaOperationError('Field name cannot be empty', schemaId, 'removeField');
    }

    try {
      const result = await this.adapter.removeSchemaField(schemaId, fieldName);

      // Invalidate cache since schema was modified
      this.schemaCache.delete(schemaId);

      return result;
    } catch (error) {
      const message = formatSchemaError(error, 'remove field', schemaId);
      throw new SchemaOperationError(message, schemaId, 'removeField', error);
    }
  }

  /**
   * Extend an enum field with a new value
   *
   * Adds a value to the `user_values` array of an enum field. The field must
   * be marked as `extensible = true`. Core values cannot be added through this
   * method. The schema version is automatically incremented.
   *
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the enum field
   * @param value - Value to add to user_values
   * @returns Result with new schema version
   * @throws {SchemaOperationError} If field is not an enum, not extensible, or value already exists
   *
   * @example
   * ```typescript
   * // Add "BLOCKED" status to task enum
   * const result = await service.extendEnum('task', 'status', 'BLOCKED');
   * console.log(`Enum extended, schema now at version ${result.newVersion}`);
   * ```
   */
  async extendEnum(schemaId: string, fieldName: string, value: string): Promise<ExtendEnumResult> {
    // Validate input
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'extendEnum');
    }
    if (!fieldName || fieldName.trim() === '') {
      throw new SchemaOperationError('Field name cannot be empty', schemaId, 'extendEnum');
    }
    if (!value || value.trim() === '') {
      throw new SchemaOperationError('Enum value cannot be empty', schemaId, 'extendEnum');
    }

    try {
      const result = await this.adapter.extendSchemaEnum(schemaId, fieldName, value);

      // Invalidate cache since schema was modified
      this.schemaCache.delete(schemaId);

      return result;
    } catch (error) {
      const message = formatSchemaError(error, 'extend enum', schemaId);
      throw new SchemaOperationError(message, schemaId, 'extendEnum', error);
    }
  }

  /**
   * Remove a value from an enum field
   *
   * Removes a value from the `user_values` array of an enum field. Core values
   * cannot be removed - they are protected. The schema version is automatically incremented.
   *
   * @param schemaId - Schema ID to modify
   * @param fieldName - Name of the enum field
   * @param value - Value to remove from user_values
   * @returns Result with new schema version
   * @throws {SchemaOperationError} If value is a core value or not found in user_values
   *
   * @example
   * ```typescript
   * // Remove a custom status
   * const result = await service.removeEnumValue('task', 'status', 'BLOCKED');
   * console.log(`Value removed, schema now at version ${result.newVersion}`);
   * ```
   */
  async removeEnumValue(
    schemaId: string,
    fieldName: string,
    value: string
  ): Promise<RemoveEnumValueResult> {
    // Validate input
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'removeEnumValue');
    }
    if (!fieldName || fieldName.trim() === '') {
      throw new SchemaOperationError('Field name cannot be empty', schemaId, 'removeEnumValue');
    }
    if (!value || value.trim() === '') {
      throw new SchemaOperationError('Enum value cannot be empty', schemaId, 'removeEnumValue');
    }

    try {
      const result = await this.adapter.removeSchemaEnumValue(schemaId, fieldName, value);

      // Invalidate cache since schema was modified
      this.schemaCache.delete(schemaId);

      return result;
    } catch (error) {
      const message = formatSchemaError(error, 'remove enum value', schemaId);
      throw new SchemaOperationError(message, schemaId, 'removeEnumValue', error);
    }
  }

  /**
   * Get all enum values for a field (core + user values combined)
   *
   * Helper method to get the complete list of allowed enum values.
   *
   * @param schemaId - Schema ID
   * @param fieldName - Enum field name
   * @returns Array of all allowed values (core + user)
   * @throws {SchemaOperationError} If schema or field not found
   *
   * @example
   * ```typescript
   * const statusValues = await service.getEnumValues('task', 'status');
   * // Returns: ['OPEN', 'IN_PROGRESS', 'DONE', 'BLOCKED']
   * ```
   */
  async getEnumValues(schemaId: string, fieldName: string): Promise<string[]> {
    const schema = await this.getSchema(schemaId);
    const field = schema.fields.find((f) => f.name === fieldName);

    if (!field) {
      throw new Error(`Field '${fieldName}' not found in schema '${schemaId}'`);
    }

    if (field.type !== 'enum') {
      throw new Error(`Field '${fieldName}' is not an enum field`);
    }

    const values: string[] = [];
    if (field.coreValues) {
      values.push(...field.coreValues);
    }
    if (field.userValues) {
      values.push(...field.userValues);
    }

    return values;
  }

  /**
   * Check if a field can be deleted
   *
   * Helper method to determine if a field is user-protected (can be deleted).
   *
   * @param schemaId - Schema ID
   * @param fieldName - Field name to check
   * @returns true if field can be deleted, false otherwise
   * @throws {SchemaOperationError} If schema or field not found
   */
  async canDeleteField(schemaId: string, fieldName: string): Promise<boolean> {
    const schema = await this.getSchema(schemaId);
    const field = schema.fields.find((f) => f.name === fieldName);

    if (!field) {
      return false;
    }

    return field.protection === 'user';
  }
}

/**
 * Factory function to create a new SchemaService instance
 *
 * @param adapter - Optional backend adapter (defaults to auto-detected adapter)
 * @returns New SchemaService instance
 *
 * @example
 * ```svelte
 * <script lang="ts">
 *   import { createSchemaService } from '$lib/services/schema-service.svelte';
 *
 *   const schemaService = createSchemaService();
 * </script>
 * ```
 */
export function createSchemaService(adapter?: BackendAdapter): SchemaService {
  return new SchemaService(adapter);
}

/**
 * Singleton instance of SchemaService for convenience
 *
 * Use this if you don't need multiple service instances.
 *
 * @example
 * ```svelte
 * <script lang="ts">
 *   import { schemaService } from '$lib/services/schema-service.svelte';
 *
 *   const schema = await schemaService.getSchema('task');
 * </script>
 * ```
 */
export const schemaService = createSchemaService();
