/**
 * Tests for Error Types and Utilities
 *
 * Comprehensive test coverage for structured error handling including:
 * - CommandError interface and type guard
 * - Error conversion utilities
 * - Custom error classes (DatabaseInitializationError, NodeOperationError)
 * - MCP version conflict detection
 */

import { describe, it, expect } from 'vitest';
import {
  type CommandError,
  type MCPError,
  type VersionConflictData,
  isCommandError,
  toError,
  DatabaseInitializationError,
  NodeOperationError,
  VERSION_CONFLICT_CODE,
  isVersionConflict
} from '$lib/types/errors';
import type { Node } from '$lib/types/node';

describe('CommandError Type Guard', () => {
  it('identifies valid CommandError objects', () => {
    const error: CommandError = {
      message: 'Something went wrong',
      code: 'NODE_SERVICE_ERROR',
      details: 'Additional details'
    };

    expect(isCommandError(error)).toBe(true);
  });

  it('identifies CommandError with only message', () => {
    const error: CommandError = {
      message: 'Basic error'
    };

    expect(isCommandError(error)).toBe(true);
  });

  it('identifies CommandError with code but no details', () => {
    const error: CommandError = {
      message: 'Error with code',
      code: 'ERROR_CODE'
    };

    expect(isCommandError(error)).toBe(true);
  });

  it('identifies CommandError with details but no code', () => {
    const error: CommandError = {
      message: 'Error with details',
      details: 'Some details'
    };

    expect(isCommandError(error)).toBe(true);
  });

  it('rejects null', () => {
    expect(isCommandError(null)).toBe(false);
  });

  it('rejects undefined', () => {
    expect(isCommandError(undefined)).toBe(false);
  });

  it('rejects objects without message', () => {
    const notError = {
      code: 'SOME_CODE',
      details: 'Details without message'
    };

    expect(isCommandError(notError)).toBe(false);
  });

  it('rejects objects with non-string message', () => {
    const notError = {
      message: 123,
      code: 'CODE'
    };

    expect(isCommandError(notError)).toBe(false);
  });

  it('rejects string values', () => {
    expect(isCommandError('error string')).toBe(false);
  });

  it('rejects number values', () => {
    expect(isCommandError(42)).toBe(false);
  });

  it('rejects boolean values', () => {
    expect(isCommandError(true)).toBe(false);
  });

  it('rejects arrays', () => {
    expect(isCommandError([{ message: 'error' }])).toBe(false);
  });

  it('accepts Error instances (they have message property)', () => {
    // Error instances have a message property, so they pass the guard
    expect(isCommandError(new Error('test'))).toBe(true);
  });
});

describe('toError Conversion Utility', () => {
  describe('Error instance handling', () => {
    it('returns Error instances unchanged', () => {
      const originalError = new Error('Original error');
      const result = toError(originalError);

      expect(result).toBe(originalError);
      expect(result.message).toBe('Original error');
    });

    it('returns custom Error subclasses unchanged', () => {
      const customError = new TypeError('Type error');
      const result = toError(customError);

      expect(result).toBe(customError);
      expect(result.message).toBe('Type error');
      expect(result).toBeInstanceOf(TypeError);
    });

    it('returns DatabaseInitializationError unchanged', () => {
      const dbError = new DatabaseInitializationError('DB failed');
      const result = toError(dbError);

      expect(result).toBe(dbError);
      expect(result).toBeInstanceOf(DatabaseInitializationError);
    });

    it('returns NodeOperationError unchanged', () => {
      const nodeError = new NodeOperationError('Node op failed');
      const result = toError(nodeError);

      expect(result).toBe(nodeError);
      expect(result).toBeInstanceOf(NodeOperationError);
    });
  });

  describe('CommandError handling', () => {
    it('converts CommandError to Error with message', () => {
      const commandError: CommandError = {
        message: 'Command failed',
        code: 'CMD_ERROR'
      };
      const result = toError(commandError);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Command failed');
    });

    it('converts CommandError with details to Error', () => {
      const commandError: CommandError = {
        message: 'Operation failed',
        code: 'OP_ERROR',
        details: 'Detailed info'
      };
      const result = toError(commandError);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Operation failed');
    });

    it('converts CommandError with only message', () => {
      const commandError: CommandError = {
        message: 'Simple error'
      };
      const result = toError(commandError);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Simple error');
    });
  });

  describe('String handling', () => {
    it('converts string to Error', () => {
      const result = toError('String error');

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('String error');
    });

    it('converts empty string to Error', () => {
      const result = toError('');

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('');
    });
  });

  describe('Unknown value handling', () => {
    it('converts null to generic Error', () => {
      const result = toError(null);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts undefined to generic Error', () => {
      const result = toError(undefined);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts number to generic Error', () => {
      const result = toError(42);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts boolean to generic Error', () => {
      const result = toError(true);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts array to generic Error', () => {
      const result = toError(['error', 'array']);

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts plain object to generic Error', () => {
      const result = toError({ someKey: 'someValue' });

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });

    it('converts object with non-string message to generic Error', () => {
      const result = toError({ message: 123 });

      expect(result).toBeInstanceOf(Error);
      expect(result.message).toBe('Unknown error occurred');
    });
  });
});

describe('DatabaseInitializationError', () => {
  it('creates error with message', () => {
    const error = new DatabaseInitializationError('Database connection failed');

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe('Database connection failed');
    expect(error.name).toBe('DatabaseInitializationError');
    expect(error.details).toBeUndefined();
  });

  it('creates error with message and details', () => {
    const error = new DatabaseInitializationError(
      'Database connection failed',
      'Connection timeout after 30 seconds'
    );

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe('Database connection failed');
    expect(error.name).toBe('DatabaseInitializationError');
    expect(error.details).toBe('Connection timeout after 30 seconds');
  });

  it('creates error with empty details', () => {
    const error = new DatabaseInitializationError('Error', '');

    expect(error.message).toBe('Error');
    expect(error.details).toBe('');
  });

  it('has correct prototype chain', () => {
    const error = new DatabaseInitializationError('Test error');

    expect(error).toBeInstanceOf(DatabaseInitializationError);
    expect(error).toBeInstanceOf(Error);
  });

  it('can be caught as Error', () => {
    try {
      throw new DatabaseInitializationError('Test');
    } catch (e) {
      expect(e).toBeInstanceOf(Error);
      expect(e).toBeInstanceOf(DatabaseInitializationError);
      if (e instanceof DatabaseInitializationError) {
        expect(e.name).toBe('DatabaseInitializationError');
      }
    }
  });

  it('preserves stack trace', () => {
    const error = new DatabaseInitializationError('Test');
    expect(error.stack).toBeDefined();
    expect(error.stack).toContain('DatabaseInitializationError');
  });
});

describe('NodeOperationError', () => {
  it('creates error with only message', () => {
    const error = new NodeOperationError('Node operation failed');

    expect(error).toBeInstanceOf(Error);
    expect(error.message).toBe('Node operation failed');
    expect(error.name).toBe('NodeOperationError');
    expect(error.nodeId).toBeUndefined();
    expect(error.operation).toBeUndefined();
  });

  it('creates error with message and nodeId', () => {
    const error = new NodeOperationError('Failed to update node', 'node-123');

    expect(error.message).toBe('Failed to update node');
    expect(error.nodeId).toBe('node-123');
    expect(error.operation).toBeUndefined();
  });

  it('creates error with message, nodeId, and operation', () => {
    const error = new NodeOperationError(
      'Failed to delete node',
      'node-456',
      'delete'
    );

    expect(error.message).toBe('Failed to delete node');
    expect(error.nodeId).toBe('node-456');
    expect(error.operation).toBe('delete');
  });

  it('creates error with message and operation but no nodeId', () => {
    const error = new NodeOperationError('Batch operation failed', undefined, 'batch_create');

    expect(error.message).toBe('Batch operation failed');
    expect(error.nodeId).toBeUndefined();
    expect(error.operation).toBe('batch_create');
  });

  it('has correct prototype chain', () => {
    const error = new NodeOperationError('Test error');

    expect(error).toBeInstanceOf(NodeOperationError);
    expect(error).toBeInstanceOf(Error);
  });

  it('can be caught as Error', () => {
    try {
      throw new NodeOperationError('Test', 'node-1', 'create');
    } catch (e) {
      expect(e).toBeInstanceOf(Error);
      expect(e).toBeInstanceOf(NodeOperationError);
      if (e instanceof NodeOperationError) {
        expect(e.name).toBe('NodeOperationError');
        expect(e.nodeId).toBe('node-1');
        expect(e.operation).toBe('create');
      }
    }
  });

  it('preserves stack trace', () => {
    const error = new NodeOperationError('Test');
    expect(error.stack).toBeDefined();
    expect(error.stack).toContain('NodeOperationError');
  });

  it('handles empty strings for optional parameters', () => {
    const error = new NodeOperationError('Error', '', '');
    expect(error.nodeId).toBe('');
    expect(error.operation).toBe('');
  });
});

describe('VERSION_CONFLICT_CODE Constant', () => {
  it('has correct value', () => {
    expect(VERSION_CONFLICT_CODE).toBe(-32005);
  });

  it('is a number', () => {
    expect(typeof VERSION_CONFLICT_CODE).toBe('number');
  });
});

describe('isVersionConflict Type Guard', () => {
  const createMockNode = (): Node => ({
    id: 'node-123',
    nodeType: 'text',
    content: 'Test content',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 5,
    properties: {}
  });

  it('identifies valid version conflict error', () => {
    const versionConflictData: VersionConflictData = {
      type: 'VersionConflict',
      node_id: 'node-123',
      expected_version: 3,
      actual_version: 5,
      current_node: createMockNode()
    };

    const error: MCPError = {
      code: VERSION_CONFLICT_CODE,
      message: 'Version conflict detected',
      data: versionConflictData
    };

    expect(isVersionConflict(error)).toBe(true);
  });

  it('identifies version conflict with complete data', () => {
    const error: MCPError = {
      code: -32005,
      message: 'Conflict',
      data: {
        type: 'VersionConflict',
        node_id: 'node-456',
        expected_version: 1,
        actual_version: 2,
        current_node: createMockNode()
      }
    };

    expect(isVersionConflict(error)).toBe(true);
  });

  it('rejects null', () => {
    expect(isVersionConflict(null)).toBe(false);
  });

  it('rejects undefined', () => {
    expect(isVersionConflict(undefined)).toBe(false);
  });

  it('rejects error with wrong code', () => {
    const error = {
      code: -32000,
      message: 'Different error',
      data: {
        type: 'VersionConflict',
        node_id: 'node-123',
        expected_version: 3,
        actual_version: 5,
        current_node: createMockNode()
      }
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error without data', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Error without data'
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error with null data', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Error with null data',
      data: null
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error with wrong data type string', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Wrong type',
      data: {
        type: 'DifferentError',
        node_id: 'node-123',
        expected_version: 3,
        actual_version: 5
      }
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error with missing type field', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Missing type',
      data: {
        node_id: 'node-123',
        expected_version: 3,
        actual_version: 5
      }
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error with non-object data', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'String data',
      data: 'some string'
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects error with array data', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Array data',
      data: ['array', 'items']
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects string values', () => {
    expect(isVersionConflict('error string')).toBe(false);
  });

  it('rejects number values', () => {
    expect(isVersionConflict(VERSION_CONFLICT_CODE)).toBe(false);
  });

  it('rejects boolean values', () => {
    expect(isVersionConflict(false)).toBe(false);
  });

  it('rejects plain objects without required structure', () => {
    const error = {
      someKey: 'someValue',
      code: VERSION_CONFLICT_CODE
    };

    expect(isVersionConflict(error)).toBe(false);
  });

  it('rejects Error instances', () => {
    expect(isVersionConflict(new Error('test'))).toBe(false);
  });

  it('handles partial data structure', () => {
    const error = {
      code: VERSION_CONFLICT_CODE,
      message: 'Partial data',
      data: {
        type: 'VersionConflict'
        // Missing other required fields
      }
    };

    // Should still pass type guard since it only checks type field
    expect(isVersionConflict(error)).toBe(true);
  });
});

describe('MCPError Interface', () => {
  it('accepts valid MCPError structure', () => {
    const error: MCPError = {
      code: -32000,
      message: 'Generic error'
    };

    expect(error.code).toBe(-32000);
    expect(error.message).toBe('Generic error');
    expect(error.data).toBeUndefined();
  });

  it('accepts MCPError with version conflict data', () => {
    const versionConflictData: VersionConflictData = {
      type: 'VersionConflict',
      node_id: 'node-123',
      expected_version: 1,
      actual_version: 2,
      current_node: {
        id: 'node-123',
        nodeType: 'text',
        content: 'Test',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 2,
        properties: {}
      }
    };

    const error: MCPError = {
      code: VERSION_CONFLICT_CODE,
      message: 'Version conflict',
      data: versionConflictData
    };

    expect(error.code).toBe(VERSION_CONFLICT_CODE);
    expect(error.data).toBeDefined();
    if (isVersionConflict(error)) {
      expect(error.data.type).toBe('VersionConflict');
      expect(error.data.node_id).toBe('node-123');
      expect(error.data.expected_version).toBe(1);
      expect(error.data.actual_version).toBe(2);
    }
  });

  it('accepts MCPError with unknown data', () => {
    const error: MCPError = {
      code: -32001,
      message: 'Custom error',
      data: { customField: 'custom value' }
    };

    expect(error.code).toBe(-32001);
    expect(error.data).toEqual({ customField: 'custom value' });
  });
});

describe('VersionConflictData Interface', () => {
  it('accepts valid version conflict data structure', () => {
    const data: VersionConflictData = {
      type: 'VersionConflict',
      node_id: 'node-789',
      expected_version: 10,
      actual_version: 15,
      current_node: {
        id: 'node-789',
        nodeType: 'task',
        content: 'Task content',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 15,
        properties: {}
      }
    };

    expect(data.type).toBe('VersionConflict');
    expect(data.node_id).toBe('node-789');
    expect(data.expected_version).toBe(10);
    expect(data.actual_version).toBe(15);
    expect(data.current_node).toBeDefined();
    expect(data.current_node.version).toBe(15);
  });

  it('handles version conflict with zero versions', () => {
    const data: VersionConflictData = {
      type: 'VersionConflict',
      node_id: 'node-000',
      expected_version: 0,
      actual_version: 1,
      current_node: {
        id: 'node-000',
        nodeType: 'text',
        content: 'Content',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      }
    };

    expect(data.expected_version).toBe(0);
    expect(data.actual_version).toBe(1);
  });

  it('handles version conflict with large version numbers', () => {
    const data: VersionConflictData = {
      type: 'VersionConflict',
      node_id: 'node-large',
      expected_version: 999999,
      actual_version: 1000000,
      current_node: {
        id: 'node-large',
        nodeType: 'text',
        content: 'Content',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1000000,
        properties: {}
      }
    };

    expect(data.expected_version).toBe(999999);
    expect(data.actual_version).toBe(1000000);
  });
});
