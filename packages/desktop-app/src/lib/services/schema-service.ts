/**
 * Schema Service - TypeScript Wrapper for Schema Management
 *
 * Provides a type-safe TypeScript wrapper around the Rust backend's schema
 * management MCP tools. Enables frontend components to manage user-defined
 * entity schemas with full type safety.
 *
 * ## Features
 *
 * - Add/remove user fields from schemas
 * - Extend/remove enum values (user_values only)
 * - Get complete schema definitions
 * - Protection level enforcement (core fields protected)
 * - User-friendly error messages
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
 * - Frontend Service (this file) → Tauri IPC → MCP Tools → SchemaService → NodeService → Database
 * - All operations enforce protection levels
 * - Schema version automatically incremented on changes
 * - Stateless design (no caching) - backend is fast enough for current needs
 *
 * @see packages/core/src/services/schema_service.rs - Rust implementation
 * @see packages/core/src/mcp/handlers/schema.rs - MCP tool handlers
 */

import { invoke } from '@tauri-apps/api/core';
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
 * Schema Service class
 *
 * Wraps Rust backend MCP schema tools with TypeScript interface
 */
export class SchemaService {
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
    // Validate input before expensive IPC call
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'get');
    }

    try {
      const result = await invoke<{ schema: unknown; schemaId: string; success: boolean }>(
        'mcp_call_tool',
        {
          name: 'get_schema_definition',
          arguments: { schema_id: schemaId }
        }
      );

      if (!result.success || !result.schema) {
        throw new Error(`Failed to retrieve schema '${schemaId}'`);
      }

      // Convert from snake_case (Rust) to camelCase (TypeScript)
      const schema = result.schema as {
        is_core: boolean;
        version: number;
        description: string;
        fields: Array<{
          name: string;
          type: string;
          protection: string;
          core_values?: string[];
          user_values?: string[];
          indexed: boolean;
          required?: boolean;
          extensible?: boolean;
          default?: unknown;
          description?: string;
          item_type?: string;
        }>;
      };

      return {
        isCore: schema.is_core,
        version: schema.version,
        description: schema.description,
        fields: schema.fields.map((field) => ({
          name: field.name,
          type: field.type,
          protection: field.protection as 'core' | 'user' | 'system',
          coreValues: field.core_values,
          userValues: field.user_values,
          indexed: field.indexed,
          required: field.required,
          extensible: field.extensible,
          default: field.default,
          description: field.description,
          itemType: field.item_type
        }))
      };
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
    // Validate input before expensive IPC call
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
      const result = await invoke<{
        schema_id: string;
        new_version: number;
        success: boolean;
      }>('mcp_call_tool', {
        name: 'add_schema_field',
        arguments: {
          schema_id: schemaId,
          field_name: config.fieldName,
          field_type: config.fieldType,
          indexed: config.indexed ?? false,
          required: config.required,
          default: config.default,
          description: config.description,
          item_type: config.itemType,
          enum_values: config.enumValues,
          extensible: config.extensible
        }
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: result.success
      };
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
    // Validate input before expensive IPC call
    if (!schemaId || schemaId.trim() === '') {
      throw new SchemaOperationError('Schema ID cannot be empty', schemaId, 'removeField');
    }
    if (!fieldName || fieldName.trim() === '') {
      throw new SchemaOperationError('Field name cannot be empty', schemaId, 'removeField');
    }

    try {
      const result = await invoke<{
        schema_id: string;
        new_version: number;
        success: boolean;
      }>('mcp_call_tool', {
        name: 'remove_schema_field',
        arguments: {
          schema_id: schemaId,
          field_name: fieldName
        }
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: result.success
      };
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
    // Validate input before expensive IPC call
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
      const result = await invoke<{
        schema_id: string;
        new_version: number;
        success: boolean;
      }>('mcp_call_tool', {
        name: 'extend_schema_enum',
        arguments: {
          schema_id: schemaId,
          field_name: fieldName,
          value: value
        }
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: result.success
      };
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
    // Validate input before expensive IPC call
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
      const result = await invoke<{
        schema_id: string;
        new_version: number;
        success: boolean;
      }>('mcp_call_tool', {
        name: 'remove_schema_enum_value',
        arguments: {
          schema_id: schemaId,
          field_name: fieldName,
          value: value
        }
      });

      return {
        schemaId: result.schema_id,
        newVersion: result.new_version,
        success: result.success
      };
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
export function createSchemaService(): SchemaService {
  return new SchemaService();
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
