/**
 * Tests for Error Types and Utilities
 *
 * Comprehensive test coverage for structured error handling including:
 * - CommandError interface and type guard
 * - Error conversion utilities
 * - Custom error classes (DatabaseInitializationError, NodeOperationError)
 * - gRPC version conflict detection (isVersionConflict)
 */

import { describe, it, expect } from 'vitest';
import {
  type CommandError,
  type VersionConflictData,
  type VersionConflictCommandError,
  type SubtreeAccessDeniedData,
  type SubtreeAccessDeniedCommandError,
  type PlayRuleRejectedData,
  type PlayRuleRejectedCommandError,
  isCommandError,
  toError,
  DatabaseInitializationError,
  NodeOperationError,
  isVersionConflict,
  isSubtreeAccessDenied,
  isPlayRuleRejected
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

describe('isVersionConflict Type Guard (gRPC shape)', () => {
  const createMockNode = (): Node => ({
    id: 'node-123',
    nodeType: 'text',
    content: 'Test content',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 5,
    properties: {}
  });

  const makeConflict = (overrides?: Partial<VersionConflictData>): VersionConflictCommandError => ({
    message: 'Version conflict on node-123: expected 3, got 5',
    code: 'VERSION_CONFLICT',
    details: 'Aborted',
    conflictData: {
      node_id: 'node-123',
      expected: 3,
      actual: 5,
      current_node: createMockNode(),
      ...overrides
    }
  });

  it('identifies valid gRPC VERSION_CONFLICT error', () => {
    expect(isVersionConflict(makeConflict())).toBe(true);
  });

  it('identifies conflict with null current_node (daemon could not fetch)', () => {
    expect(isVersionConflict(makeConflict({ current_node: null }))).toBe(true);
  });

  it('rejects null', () => {
    expect(isVersionConflict(null)).toBe(false);
  });

  it('rejects undefined', () => {
    expect(isVersionConflict(undefined)).toBe(false);
  });

  it('rejects error with wrong code string', () => {
    expect(isVersionConflict({ message: 'err', code: 'NODE_NOT_FOUND', conflictData: { node_id: 'x' } })).toBe(false);
  });

  it('rejects error without conflictData', () => {
    expect(isVersionConflict({ message: 'err', code: 'VERSION_CONFLICT' })).toBe(false);
  });

  it('rejects error with null conflictData', () => {
    expect(isVersionConflict({ message: 'err', code: 'VERSION_CONFLICT', conflictData: null })).toBe(false);
  });

  it('rejects error with conflictData missing node_id', () => {
    expect(isVersionConflict({ message: 'err', code: 'VERSION_CONFLICT', conflictData: { expected: 1, actual: 2 } })).toBe(false);
  });

  it('rejects string values', () => {
    expect(isVersionConflict('error string')).toBe(false);
  });

  it('rejects number values', () => {
    expect(isVersionConflict(42)).toBe(false);
  });

  it('rejects boolean values', () => {
    expect(isVersionConflict(false)).toBe(false);
  });

  it('rejects Error instances', () => {
    expect(isVersionConflict(new Error('test'))).toBe(false);
  });

  it('exposes typed conflictData fields after narrowing', () => {
    const err = makeConflict();
    if (isVersionConflict(err)) {
      expect(err.conflictData.node_id).toBe('node-123');
      expect(err.conflictData.expected).toBe(3);
      expect(err.conflictData.actual).toBe(5);
    }
  });
});

describe('isSubtreeAccessDenied Type Guard (gRPC shape)', () => {
  const makeRefusal = (
    overrides?: Partial<SubtreeAccessDeniedData>
  ): SubtreeAccessDeniedCommandError => ({
    message: 'Delete refused: subtree contains 3 node(s) not accessible to the current actor',
    code: 'SUBTREE_ACCESS_DENIED',
    details: 'FailedPrecondition',
    conflictData: {
      inaccessibleCount: 3,
      ...overrides
    }
  });

  it('identifies a valid SUBTREE_ACCESS_DENIED error', () => {
    expect(isSubtreeAccessDenied(makeRefusal())).toBe(true);
  });

  it('narrows to expose the inaccessibleCount', () => {
    const err = makeRefusal({ inaccessibleCount: 7 });
    if (isSubtreeAccessDenied(err)) {
      expect(err.conflictData.inaccessibleCount).toBe(7);
    } else {
      throw new Error('type guard should have matched');
    }
  });

  it('rejects a VERSION_CONFLICT error (distinct refusal)', () => {
    expect(
      isSubtreeAccessDenied({
        message: 'err',
        code: 'VERSION_CONFLICT',
        conflictData: { node_id: 'x', expected: 1, actual: 2 }
      })
    ).toBe(false);
  });

  it('rejects error with wrong code string', () => {
    expect(
      isSubtreeAccessDenied({
        message: 'err',
        code: 'INVALID_ARGUMENT',
        conflictData: { inaccessibleCount: 1 }
      })
    ).toBe(false);
  });

  it('rejects error without conflictData', () => {
    expect(isSubtreeAccessDenied({ message: 'err', code: 'SUBTREE_ACCESS_DENIED' })).toBe(false);
  });

  it('rejects error with null conflictData', () => {
    expect(
      isSubtreeAccessDenied({ message: 'err', code: 'SUBTREE_ACCESS_DENIED', conflictData: null })
    ).toBe(false);
  });

  it('rejects error with non-numeric inaccessibleCount', () => {
    expect(
      isSubtreeAccessDenied({
        message: 'err',
        code: 'SUBTREE_ACCESS_DENIED',
        conflictData: { inaccessibleCount: '3' }
      })
    ).toBe(false);
  });

  it('rejects null', () => {
    expect(isSubtreeAccessDenied(null)).toBe(false);
  });

  it('rejects undefined', () => {
    expect(isSubtreeAccessDenied(undefined)).toBe(false);
  });

  it('rejects Error instances', () => {
    expect(isSubtreeAccessDenied(new Error('test'))).toBe(false);
  });
});

describe('isPlayRuleRejected Type Guard (gRPC shape)', () => {
  const makeRejection = (
    overrides?: Partial<PlayRuleRejectedData>
  ): PlayRuleRejectedCommandError => ({
    message: "Play rule 'reject-rule' (play play-1) rejected the write to node node-1: no",
    code: 'PLAY_RULE_REJECTED',
    details: 'FailedPrecondition',
    conflictData: {
      node_id: 'node-1',
      play_id: 'play-1',
      rule_name: 'reject-rule',
      message: 'cannot close while children are open',
      ...overrides
    }
  });

  it('identifies a valid PLAY_RULE_REJECTED error', () => {
    expect(isPlayRuleRejected(makeRejection())).toBe(true);
  });

  it('narrows to expose the rejecting rule fields', () => {
    const err = makeRejection({ rule_name: 'other-rule', message: 'custom text' });
    if (isPlayRuleRejected(err)) {
      expect(err.conflictData.rule_name).toBe('other-rule');
      expect(err.conflictData.message).toBe('custom text');
    } else {
      throw new Error('type guard should have matched');
    }
  });

  it('rejects a VERSION_CONFLICT error (distinct refusal)', () => {
    expect(
      isPlayRuleRejected({
        message: 'err',
        code: 'VERSION_CONFLICT',
        conflictData: { node_id: 'x', expected: 1, actual: 2 }
      })
    ).toBe(false);
  });

  it('rejects a SUBTREE_ACCESS_DENIED error (distinct refusal, same status code)', () => {
    expect(
      isPlayRuleRejected({
        message: 'err',
        code: 'SUBTREE_ACCESS_DENIED',
        conflictData: { inaccessibleCount: 3 }
      })
    ).toBe(false);
  });

  it('rejects error with wrong code string', () => {
    expect(
      isPlayRuleRejected({
        message: 'err',
        code: 'INVALID_ARGUMENT',
        conflictData: { message: 'no' }
      })
    ).toBe(false);
  });

  it('rejects error without conflictData', () => {
    expect(isPlayRuleRejected({ message: 'err', code: 'PLAY_RULE_REJECTED' })).toBe(false);
  });

  it('rejects error with null conflictData', () => {
    expect(
      isPlayRuleRejected({ message: 'err', code: 'PLAY_RULE_REJECTED', conflictData: null })
    ).toBe(false);
  });

  it('rejects error with non-string message in conflictData', () => {
    expect(
      isPlayRuleRejected({
        message: 'err',
        code: 'PLAY_RULE_REJECTED',
        conflictData: { message: 5 }
      })
    ).toBe(false);
  });

  it('rejects null', () => {
    expect(isPlayRuleRejected(null)).toBe(false);
  });

  it('rejects undefined', () => {
    expect(isPlayRuleRejected(undefined)).toBe(false);
  });

  it('rejects Error instances', () => {
    expect(isPlayRuleRejected(new Error('test'))).toBe(false);
  });
});

describe('VersionConflictData Interface', () => {
  it('accepts valid version conflict data structure', () => {
    const data: VersionConflictData = {
      node_id: 'node-789',
      expected: 10,
      actual: 15,
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

    expect(data.node_id).toBe('node-789');
    expect(data.expected).toBe(10);
    expect(data.actual).toBe(15);
    expect(data.current_node).toBeDefined();
  });

  it('accepts null current_node (daemon fallback)', () => {
    const data: VersionConflictData = {
      node_id: 'node-000',
      expected: 0,
      actual: 1,
      current_node: null
    };

    expect(data.current_node).toBeNull();
  });

  it('handles large version numbers', () => {
    const data: VersionConflictData = {
      node_id: 'node-large',
      expected: 999999,
      actual: 1000000,
      current_node: null
    };

    expect(data.expected).toBe(999999);
    expect(data.actual).toBe(1000000);
  });
});
