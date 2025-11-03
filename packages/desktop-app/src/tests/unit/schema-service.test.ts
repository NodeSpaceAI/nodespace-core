/**
 * Schema Service Tests
 *
 * Tests for the TypeScript schema service wrapper. Uses Vitest mocking
 * to simulate Tauri IPC calls without requiring the full Rust backend.
 *
 * ## Test Coverage
 *
 * - Schema retrieval
 * - Field addition (success and error cases)
 * - Field removal (success and error cases)
 * - Enum extension
 * - Enum value removal
 * - Helper methods (getEnumValues, canDeleteField)
 * - Error handling and user-friendly messages
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SchemaService, createSchemaService } from '$lib/services/schema-service.svelte';
import type { SchemaDefinition } from '$lib/types/schema';

// Mock Tauri's invoke function
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}));

import { invoke } from '@tauri-apps/api/core';
const mockInvoke = vi.mocked(invoke);

describe('SchemaService', () => {
  let service: SchemaService;

  beforeEach(() => {
    service = createSchemaService();
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('getSchema', () => {
    it('should retrieve a schema definition', async () => {
      const mockSchema = {
        is_core: true,
        version: 2,
        description: 'Task tracking schema',
        fields: [
          {
            name: 'status',
            type: 'enum',
            protection: 'core',
            core_values: ['OPEN', 'IN_PROGRESS', 'DONE'],
            user_values: ['BLOCKED'],
            indexed: true,
            required: true,
            extensible: true,
            default: 'OPEN',
            description: 'Task status'
          },
          {
            name: 'priority',
            type: 'number',
            protection: 'user',
            indexed: false,
            required: false,
            default: 0,
            description: 'Task priority'
          }
        ]
      };

      mockInvoke.mockResolvedValueOnce({
        schema: mockSchema,
        schemaId: 'task',
        success: true
      });

      const result = await service.getSchema('task');

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'get_schema_definition',
        arguments: { schema_id: 'task' }
      });

      expect(result).toEqual({
        isCore: true,
        version: 2,
        description: 'Task tracking schema',
        fields: [
          {
            name: 'status',
            type: 'enum',
            protection: 'core',
            coreValues: ['OPEN', 'IN_PROGRESS', 'DONE'],
            userValues: ['BLOCKED'],
            indexed: true,
            required: true,
            extensible: true,
            default: 'OPEN',
            description: 'Task status',
            itemType: undefined
          },
          {
            name: 'priority',
            type: 'number',
            protection: 'user',
            coreValues: undefined,
            userValues: undefined,
            indexed: false,
            required: false,
            extensible: undefined,
            default: 0,
            description: 'Task priority',
            itemType: undefined
          }
        ]
      });
    });

    it('should throw user-friendly error when schema not found', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('Schema not found'));

      await expect(service.getSchema('nonexistent')).rejects.toThrow(
        "Schema 'nonexistent' not found"
      );
    });
  });

  describe('addField', () => {
    it('should add a new field to a schema', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema_id: 'task',
        new_version: 3,
        success: true
      });

      const result = await service.addField('task', {
        fieldName: 'due_date',
        fieldType: 'string',
        indexed: false,
        required: false,
        description: 'Task due date'
      });

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'add_schema_field',
        arguments: {
          schema_id: 'task',
          field_name: 'due_date',
          field_type: 'string',
          indexed: false,
          required: false,
          default: undefined,
          description: 'Task due date',
          item_type: undefined,
          enum_values: undefined,
          extensible: undefined
        }
      });

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should handle field already exists error', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error("Field 'priority' already exists in schema 'task'")
      );

      await expect(
        service.addField('task', {
          fieldName: 'priority',
          fieldType: 'number'
        })
      ).rejects.toThrow('Field already exists in schema');
    });

    it('should add enum field with values', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema_id: 'task',
        new_version: 3,
        success: true
      });

      await service.addField('task', {
        fieldName: 'category',
        fieldType: 'enum',
        enumValues: ['BUG', 'FEATURE', 'ENHANCEMENT'],
        extensible: true,
        required: false,
        default: 'FEATURE'
      });

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'add_schema_field',
        arguments: {
          schema_id: 'task',
          field_name: 'category',
          field_type: 'enum',
          indexed: false,
          required: false,
          default: 'FEATURE',
          description: undefined,
          item_type: undefined,
          enum_values: ['BUG', 'FEATURE', 'ENHANCEMENT'],
          extensible: true
        }
      });
    });
  });

  describe('removeField', () => {
    it('should remove a user field from schema', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema_id: 'task',
        new_version: 3,
        success: true
      });

      const result = await service.removeField('task', 'priority');

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'remove_schema_field',
        arguments: {
          schema_id: 'task',
          field_name: 'priority'
        }
      });

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when removing core field', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error("Cannot remove field 'status' with protection level Core")
      );

      await expect(service.removeField('task', 'status')).rejects.toThrow(
        'Cannot remove field protected field'
      );
    });

    it('should throw error when field not found', async () => {
      mockInvoke.mockRejectedValueOnce(new Error("Field 'nonexistent' not found in schema 'task'"));

      await expect(service.removeField('task', 'nonexistent')).rejects.toThrow('not found');
    });
  });

  describe('extendEnum', () => {
    it('should add value to enum field', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema_id: 'task',
        new_version: 3,
        success: true
      });

      const result = await service.extendEnum('task', 'status', 'BLOCKED');

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'extend_schema_enum',
        arguments: {
          schema_id: 'task',
          field_name: 'status',
          value: 'BLOCKED'
        }
      });

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when field is not enum', async () => {
      mockInvoke.mockRejectedValueOnce(new Error("Field 'priority' is not an enum (type: number)"));

      await expect(service.extendEnum('task', 'priority', 'HIGH')).rejects.toThrow('not an enum');
    });

    it('should throw error when enum not extensible', async () => {
      mockInvoke.mockRejectedValueOnce(new Error("Enum field 'status' is not extensible"));

      await expect(service.extendEnum('task', 'status', 'ARCHIVED')).rejects.toThrow(
        'not extensible'
      );
    });

    it('should throw error when value already exists', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error("Value 'BLOCKED' already exists in enum 'status'")
      );

      await expect(service.extendEnum('task', 'status', 'BLOCKED')).rejects.toThrow(
        'already exists'
      );
    });
  });

  describe('removeEnumValue', () => {
    it('should remove user value from enum', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema_id: 'task',
        new_version: 3,
        success: true
      });

      const result = await service.removeEnumValue('task', 'status', 'BLOCKED');

      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        name: 'remove_schema_enum_value',
        arguments: {
          schema_id: 'task',
          field_name: 'status',
          value: 'BLOCKED'
        }
      });

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when removing core value', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error("Cannot remove core value 'OPEN' from enum 'status'")
      );

      await expect(service.removeEnumValue('task', 'status', 'OPEN')).rejects.toThrow(
        'Cannot remove core enum values'
      );
    });

    it('should throw error when value not found', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error("Value 'ARCHIVED' not found in user values of enum 'status'")
      );

      await expect(service.removeEnumValue('task', 'status', 'ARCHIVED')).rejects.toThrow(
        'not found'
      );
    });
  });

  describe('getEnumValues', () => {
    it('should return all enum values (core + user)', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
        version: 2,
        description: 'Task schema',
        fields: [
          {
            name: 'status',
            type: 'enum',
            protection: 'core',
            coreValues: ['OPEN', 'IN_PROGRESS', 'DONE'],
            userValues: ['BLOCKED', 'WAITING'],
            indexed: true,
            required: true
          }
        ]
      };

      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: mockSchema.isCore,
          version: mockSchema.version,
          description: mockSchema.description,
          fields: [
            {
              name: 'status',
              type: 'enum',
              protection: 'core',
              core_values: ['OPEN', 'IN_PROGRESS', 'DONE'],
              user_values: ['BLOCKED', 'WAITING'],
              indexed: true,
              required: true
            }
          ]
        },
        schemaId: 'task',
        success: true
      });

      const values = await service.getEnumValues('task', 'status');

      expect(values).toEqual(['OPEN', 'IN_PROGRESS', 'DONE', 'BLOCKED', 'WAITING']);
    });

    it('should throw error when field not found', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: true,
          version: 1,
          description: 'Task schema',
          fields: []
        },
        schemaId: 'task',
        success: true
      });

      await expect(service.getEnumValues('task', 'nonexistent')).rejects.toThrow(
        "Field 'nonexistent' not found"
      );
    });

    it('should throw error when field is not enum', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: true,
          version: 1,
          description: 'Task schema',
          fields: [
            {
              name: 'priority',
              type: 'number',
              protection: 'user',
              indexed: false
            }
          ]
        },
        schemaId: 'task',
        success: true
      });

      await expect(service.getEnumValues('task', 'priority')).rejects.toThrow('not an enum field');
    });
  });

  describe('canDeleteField', () => {
    it('should return true for user-protected fields', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: true,
          version: 1,
          description: 'Task schema',
          fields: [
            {
              name: 'priority',
              type: 'number',
              protection: 'user',
              indexed: false
            }
          ]
        },
        schemaId: 'task',
        success: true
      });

      const canDelete = await service.canDeleteField('task', 'priority');

      expect(canDelete).toBe(true);
    });

    it('should return false for core-protected fields', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: true,
          version: 1,
          description: 'Task schema',
          fields: [
            {
              name: 'status',
              type: 'enum',
              protection: 'core',
              indexed: true
            }
          ]
        },
        schemaId: 'task',
        success: true
      });

      const canDelete = await service.canDeleteField('task', 'status');

      expect(canDelete).toBe(false);
    });

    it('should return false when field not found', async () => {
      mockInvoke.mockResolvedValueOnce({
        schema: {
          is_core: true,
          version: 1,
          description: 'Task schema',
          fields: []
        },
        schemaId: 'task',
        success: true
      });

      const canDelete = await service.canDeleteField('task', 'nonexistent');

      expect(canDelete).toBe(false);
    });
  });

  describe('error handling', () => {
    it('should format validation errors', async () => {
      mockInvoke.mockRejectedValueOnce(
        new Error('Cannot add core-protected fields. Field has protection: Core')
      );

      await expect(
        service.addField('task', {
          fieldName: 'test',
          fieldType: 'string'
        })
      ).rejects.toThrow('Cannot add field protected field');
    });

    it('should format unknown errors', async () => {
      mockInvoke.mockRejectedValueOnce('Unexpected error string');

      await expect(service.getSchema('task')).rejects.toThrow('Unexpected error string');
    });

    it('should handle non-Error objects', async () => {
      mockInvoke.mockRejectedValueOnce({ someProperty: 'value' });

      await expect(service.getSchema('task')).rejects.toThrow('Unknown error');
    });
  });

  describe('factory and singleton', () => {
    it('should create new service instance via factory', () => {
      const service1 = createSchemaService();
      const service2 = createSchemaService();

      expect(service1).toBeInstanceOf(SchemaService);
      expect(service2).toBeInstanceOf(SchemaService);
      expect(service1).not.toBe(service2);
    });
  });
});
