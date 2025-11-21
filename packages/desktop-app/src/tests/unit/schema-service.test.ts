/**
 * Schema Service Tests
 *
 * Tests for the TypeScript schema service wrapper. Uses Vitest mocking
 * to simulate BackendAdapter without requiring the full Rust backend.
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
import { SchemaService, createSchemaService } from '$lib/services/schema-service';
import type {
  SchemaDefinition,
  AddFieldResult,
  RemoveFieldResult,
  ExtendEnumResult,
  RemoveEnumValueResult
} from '$lib/types/schema';

import type { AddFieldConfig } from '$lib/types/schema';

// Mock BackendAdapter interface matching schema-service.ts
interface BackendAdapter {
  getAllSchemas: () => Promise<Array<SchemaDefinition & { id: string }>>;
  getSchema: (schemaId: string) => Promise<SchemaDefinition>;
  addSchemaField: (schemaId: string, config: AddFieldConfig) => Promise<AddFieldResult>;
  removeSchemaField: (schemaId: string, fieldName: string) => Promise<RemoveFieldResult>;
  extendSchemaEnum: (schemaId: string, fieldName: string, newValues: string[]) => Promise<ExtendEnumResult>;
  removeSchemaEnumValue: (schemaId: string, fieldName: string, value: string) => Promise<RemoveEnumValueResult>;
}

// Mock BackendAdapter
const mockAdapter: BackendAdapter = {
  getAllSchemas: vi.fn(),
  getSchema: vi.fn(),
  addSchemaField: vi.fn(),
  removeSchemaField: vi.fn(),
  extendSchemaEnum: vi.fn(),
  removeSchemaEnumValue: vi.fn()
};

describe('SchemaService', () => {
  let service: SchemaService;

  beforeEach(() => {
    // Create service with mocked adapter
    service = new SchemaService(mockAdapter);
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('getSchema', () => {
    it('should retrieve a schema definition', async () => {
      const mockSchema: SchemaDefinition = {
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

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      const result = await service.getSchema('task');

      expect(mockAdapter.getSchema).toHaveBeenCalledWith('task');

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
      });
    });

    it('should throw user-friendly error when schema not found', async () => {
      vi.mocked(mockAdapter.getSchema).mockRejectedValueOnce(new Error('Schema not found'));

      await expect(service.getSchema('nonexistent')).rejects.toThrow(
        "Schema 'nonexistent' not found"
      );
    });
  });

  describe('addField', () => {
    it('should add a new field to a schema', async () => {
      const mockResult: AddFieldResult = {
        schemaId: 'task',
        newVersion: 3,
        success: true
      };

      vi.mocked(mockAdapter.addSchemaField).mockResolvedValueOnce(mockResult);

      const result = await service.addField('task', {
        fieldName: 'due_date',
        fieldType: 'string',
        indexed: false,
        required: false,
        description: 'Task due date'
      });

      expect(mockAdapter.addSchemaField).toHaveBeenCalledWith('task', {
        fieldName: 'due_date',
        fieldType: 'string',
        indexed: false,
        required: false,
        description: 'Task due date'
      });

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should handle field already exists error', async () => {
      vi.mocked(mockAdapter.addSchemaField).mockRejectedValueOnce(
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
      const mockResult: AddFieldResult = {
        schemaId: 'task',
        newVersion: 3,
        success: true
      };

      vi.mocked(mockAdapter.addSchemaField).mockResolvedValueOnce(mockResult);

      await service.addField('task', {
        fieldName: 'category',
        fieldType: 'enum',
        enumValues: ['BUG', 'FEATURE', 'ENHANCEMENT'],
        extensible: true,
        required: false,
        default: 'FEATURE'
      });

      expect(mockAdapter.addSchemaField).toHaveBeenCalledWith('task', {
        fieldName: 'category',
        fieldType: 'enum',
        enumValues: ['BUG', 'FEATURE', 'ENHANCEMENT'],
        extensible: true,
        required: false,
        default: 'FEATURE'
      });
    });
  });

  describe('removeField', () => {
    it('should remove a user field from schema', async () => {
      const mockResult: RemoveFieldResult = {
        schemaId: 'task',
        newVersion: 3,
        success: true
      };

      vi.mocked(mockAdapter.removeSchemaField).mockResolvedValueOnce(mockResult);

      const result = await service.removeField('task', 'priority');

      expect(mockAdapter.removeSchemaField).toHaveBeenCalledWith('task', 'priority');

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when removing core field', async () => {
      vi.mocked(mockAdapter.removeSchemaField).mockRejectedValueOnce(
        new Error("Cannot remove field 'status' with protection level Core")
      );

      await expect(service.removeField('task', 'status')).rejects.toThrow(
        'Cannot remove field protected field'
      );
    });

    it('should throw error when field not found', async () => {
      vi.mocked(mockAdapter.removeSchemaField).mockRejectedValueOnce(
        new Error("Field 'nonexistent' not found in schema 'task'")
      );

      await expect(service.removeField('task', 'nonexistent')).rejects.toThrow('not found');
    });
  });

  describe('extendEnum', () => {
    it('should add value to enum field', async () => {
      const mockResult: ExtendEnumResult = {
        schemaId: 'task',
        newVersion: 3,
        success: true
      };

      vi.mocked(mockAdapter.extendSchemaEnum).mockResolvedValueOnce(mockResult);

      const result = await service.extendEnum('task', 'status', 'BLOCKED');

      expect(mockAdapter.extendSchemaEnum).toHaveBeenCalledWith('task', 'status', ['BLOCKED']);

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when field is not enum', async () => {
      vi.mocked(mockAdapter.extendSchemaEnum).mockRejectedValueOnce(
        new Error("Field 'priority' is not an enum (type: number)")
      );

      await expect(service.extendEnum('task', 'priority', 'HIGH')).rejects.toThrow('not an enum');
    });

    it('should throw error when enum not extensible', async () => {
      vi.mocked(mockAdapter.extendSchemaEnum).mockRejectedValueOnce(
        new Error("Enum field 'status' is not extensible")
      );

      await expect(service.extendEnum('task', 'status', 'ARCHIVED')).rejects.toThrow(
        'not extensible'
      );
    });

    it('should throw error when value already exists', async () => {
      vi.mocked(mockAdapter.extendSchemaEnum).mockRejectedValueOnce(
        new Error("Value 'BLOCKED' already exists in enum 'status'")
      );

      await expect(service.extendEnum('task', 'status', 'BLOCKED')).rejects.toThrow(
        'already exists'
      );
    });
  });

  describe('removeEnumValue', () => {
    it('should remove user value from enum', async () => {
      const mockResult: RemoveEnumValueResult = {
        schemaId: 'task',
        newVersion: 3,
        success: true
      };

      vi.mocked(mockAdapter.removeSchemaEnumValue).mockResolvedValueOnce(mockResult);

      const result = await service.removeEnumValue('task', 'status', 'BLOCKED');

      expect(mockAdapter.removeSchemaEnumValue).toHaveBeenCalledWith('task', 'status', 'BLOCKED');

      expect(result).toEqual({
        schemaId: 'task',
        newVersion: 3,
        success: true
      });
    });

    it('should throw error when removing core value', async () => {
      vi.mocked(mockAdapter.removeSchemaEnumValue).mockRejectedValueOnce(
        new Error("Cannot remove core value 'OPEN' from enum 'status'")
      );

      await expect(service.removeEnumValue('task', 'status', 'OPEN')).rejects.toThrow(
        'Cannot remove core enum values'
      );
    });

    it('should throw error when value not found', async () => {
      vi.mocked(mockAdapter.removeSchemaEnumValue).mockRejectedValueOnce(
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

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      const values = await service.getEnumValues('task', 'status');

      expect(values).toEqual(['OPEN', 'IN_PROGRESS', 'DONE', 'BLOCKED', 'WAITING']);
    });

    it('should throw error when field not found', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
        version: 1,
        description: 'Task schema',
        fields: []
      };

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      await expect(service.getEnumValues('task', 'nonexistent')).rejects.toThrow(
        "Field 'nonexistent' not found"
      );
    });

    it('should throw error when field is not enum', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
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
      };

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      await expect(service.getEnumValues('task', 'priority')).rejects.toThrow('not an enum field');
    });
  });

  describe('canDeleteField', () => {
    it('should return true for user-protected fields', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
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
      };

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      const canDelete = await service.canDeleteField('task', 'priority');

      expect(canDelete).toBe(true);
    });

    it('should return false for core-protected fields', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
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
      };

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      const canDelete = await service.canDeleteField('task', 'status');

      expect(canDelete).toBe(false);
    });

    it('should return false when field not found', async () => {
      const mockSchema: SchemaDefinition = {
        isCore: true,
        version: 1,
        description: 'Task schema',
        fields: []
      };

      vi.mocked(mockAdapter.getSchema).mockResolvedValueOnce(mockSchema);

      const canDelete = await service.canDeleteField('task', 'nonexistent');

      expect(canDelete).toBe(false);
    });
  });

  describe('error handling', () => {
    it('should format validation errors', async () => {
      vi.mocked(mockAdapter.addSchemaField).mockRejectedValueOnce(
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
      vi.mocked(mockAdapter.getSchema).mockRejectedValueOnce('Unexpected error string');

      await expect(service.getSchema('task')).rejects.toThrow('Unexpected error string');
    });

    it('should handle non-Error objects', async () => {
      vi.mocked(mockAdapter.getSchema).mockRejectedValueOnce({ someProperty: 'value' });

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
