/**
 * Unit tests for test database mode utilities
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { shouldLogDatabaseErrors, isTestEnvironment } from '$lib/utils/test-environment';
import { shouldUseDatabase, getTestModeName } from './should-use-database';

describe('Test Database Mode Utilities', () => {
  // Store original environment variables
  const originalEnv = { ...process.env };

  beforeEach(() => {
    // Reset environment before each test
    vi.unstubAllEnvs();
  });

  afterEach(() => {
    // Restore original environment
    process.env = { ...originalEnv };
  });

  describe('shouldUseDatabase', () => {
    it('returns false when TEST_USE_DATABASE is not set', () => {
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      expect(shouldUseDatabase()).toBe(false);
    });

    it('returns false when TEST_USE_DATABASE is set to false', () => {
      vi.stubEnv('TEST_USE_DATABASE', 'false');
      expect(shouldUseDatabase()).toBe(false);
    });

    it('returns true when TEST_USE_DATABASE is set to true', () => {
      vi.stubEnv('TEST_USE_DATABASE', 'true');
      expect(shouldUseDatabase()).toBe(true);
    });
  });

  describe('isTestEnvironment', () => {
    it('returns true when NODE_ENV is test', () => {
      vi.stubEnv('NODE_ENV', 'test');
      expect(isTestEnvironment()).toBe(true);
    });

    it('returns false when NODE_ENV is production', () => {
      vi.stubEnv('NODE_ENV', 'production');
      expect(isTestEnvironment()).toBe(false);
    });

    it('returns false when NODE_ENV is development', () => {
      vi.stubEnv('NODE_ENV', 'development');
      expect(isTestEnvironment()).toBe(false);
    });

    it('returns false when NODE_ENV is not set', () => {
      vi.stubEnv('NODE_ENV', undefined);
      expect(isTestEnvironment()).toBe(false);
    });
  });

  describe('shouldLogDatabaseErrors', () => {
    it('returns true in production (not test environment)', () => {
      vi.stubEnv('NODE_ENV', 'production');
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      expect(shouldLogDatabaseErrors()).toBe(true);
    });

    it('returns true in development (not test environment)', () => {
      vi.stubEnv('NODE_ENV', 'development');
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      expect(shouldLogDatabaseErrors()).toBe(true);
    });

    it('returns false in test mode without database', () => {
      vi.stubEnv('NODE_ENV', 'test');
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      expect(shouldLogDatabaseErrors()).toBe(false);
    });

    it('returns false in test mode with TEST_USE_DATABASE=false', () => {
      vi.stubEnv('NODE_ENV', 'test');
      vi.stubEnv('TEST_USE_DATABASE', 'false');
      expect(shouldLogDatabaseErrors()).toBe(false);
    });

    it('returns true in test mode with TEST_USE_DATABASE=true', () => {
      vi.stubEnv('NODE_ENV', 'test');
      vi.stubEnv('TEST_USE_DATABASE', 'true');
      expect(shouldLogDatabaseErrors()).toBe(true);
    });
  });

  describe('getTestModeName', () => {
    it('returns "in-memory" when TEST_USE_DATABASE is not set', () => {
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      expect(getTestModeName()).toBe('in-memory');
    });

    it('returns "in-memory" when TEST_USE_DATABASE is false', () => {
      vi.stubEnv('TEST_USE_DATABASE', 'false');
      expect(getTestModeName()).toBe('in-memory');
    });

    it('returns "database" when TEST_USE_DATABASE is true', () => {
      vi.stubEnv('TEST_USE_DATABASE', 'true');
      expect(getTestModeName()).toBe('database');
    });
  });

  describe('Integration: Conditional logging pattern', () => {
    it('suppresses errors in in-memory test mode', () => {
      vi.stubEnv('NODE_ENV', 'test');
      vi.stubEnv('TEST_USE_DATABASE', undefined);

      // Simulate error handling pattern from service classes
      const error = new Error('Database write failed');
      const loggedErrors: Error[] = [];

      if (shouldLogDatabaseErrors()) {
        loggedErrors.push(error);
      }

      expect(loggedErrors).toHaveLength(0);
    });

    it('logs errors in production', () => {
      vi.stubEnv('NODE_ENV', 'production');
      vi.stubEnv('TEST_USE_DATABASE', undefined);

      // Simulate error handling pattern from service classes
      const error = new Error('Database write failed');
      const loggedErrors: Error[] = [];

      if (shouldLogDatabaseErrors()) {
        loggedErrors.push(error);
      }

      expect(loggedErrors).toHaveLength(1);
      expect(loggedErrors[0]).toBe(error);
    });

    it('logs errors in database integration test mode', () => {
      vi.stubEnv('NODE_ENV', 'test');
      vi.stubEnv('TEST_USE_DATABASE', 'true');

      // Simulate error handling pattern from service classes
      const error = new Error('Database write failed');
      const loggedErrors: Error[] = [];

      if (shouldLogDatabaseErrors()) {
        loggedErrors.push(error);
      }

      expect(loggedErrors).toHaveLength(1);
      expect(loggedErrors[0]).toBe(error);
    });

    it('always tracks errors in test environment regardless of database mode', () => {
      vi.stubEnv('NODE_ENV', 'test');

      const testErrors: Error[] = [];
      const error = new Error('Database write failed');

      // Test with in-memory mode
      vi.stubEnv('TEST_USE_DATABASE', undefined);
      if (isTestEnvironment()) {
        testErrors.push(error);
      }
      expect(testErrors).toHaveLength(1);

      // Test with database mode
      vi.stubEnv('TEST_USE_DATABASE', 'true');
      const error2 = new Error('Another error');
      if (isTestEnvironment()) {
        testErrors.push(error2);
      }
      expect(testErrors).toHaveLength(2);
    });
  });
});
