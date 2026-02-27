import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}));

import {
  setDiagnosticLoggingEnabled,
  isDiagnosticLoggingEnabled,
  getLogEntries,
  getRecentLogEntries,
  clearLogEntries,
  getDiagnosticStats,
  exportLogsAsJson,
  withDiagnosticLogging,
  getDatabaseDiagnostics,
  testNodePersistence
} from '$lib/services/diagnostic-logger';

describe('Diagnostic Logger', () => {
  beforeEach(() => {
    clearLogEntries();
    setDiagnosticLoggingEnabled(true);
  });

  describe('enable/disable', () => {
    it('should be enabled by default', () => {
      expect(isDiagnosticLoggingEnabled()).toBe(true);
    });

    it('should toggle enabled state', () => {
      setDiagnosticLoggingEnabled(false);
      expect(isDiagnosticLoggingEnabled()).toBe(false);

      setDiagnosticLoggingEnabled(true);
      expect(isDiagnosticLoggingEnabled()).toBe(true);
    });
  });

  describe('getLogEntries', () => {
    it('should return empty array initially', () => {
      expect(getLogEntries()).toEqual([]);
    });

    it('should return copies (not references)', () => {
      const entries1 = getLogEntries();
      const entries2 = getLogEntries();
      expect(entries1).not.toBe(entries2);
    });
  });

  describe('getRecentLogEntries', () => {
    it('should return last N entries', async () => {
      // Add several entries via withDiagnosticLogging
      for (let i = 0; i < 5; i++) {
        await withDiagnosticLogging(`method-${i}`, () => Promise.resolve(i), []);
      }

      const recent = getRecentLogEntries(3);
      expect(recent).toHaveLength(3);
      expect(recent[0].method).toBe('method-2');
      expect(recent[2].method).toBe('method-4');
    });

    it('should return all if count exceeds entries', async () => {
      await withDiagnosticLogging('only-one', () => Promise.resolve('ok'), []);
      const recent = getRecentLogEntries(50);
      expect(recent).toHaveLength(1);
    });
  });

  describe('clearLogEntries', () => {
    it('should empty all entries', async () => {
      await withDiagnosticLogging('test', () => Promise.resolve('ok'), []);
      expect(getLogEntries().length).toBeGreaterThan(0);

      clearLogEntries();
      expect(getLogEntries()).toEqual([]);
    });
  });

  describe('getDiagnosticStats', () => {
    it('should return zero stats when empty', () => {
      const stats = getDiagnosticStats();
      expect(stats.totalCalls).toBe(0);
      expect(stats.successCalls).toBe(0);
      expect(stats.errorCalls).toBe(0);
      expect(stats.avgDurationMs).toBe(0);
      expect(stats.methodCounts).toEqual({});
    });

    it('should aggregate stats correctly', async () => {
      await withDiagnosticLogging('create', () => Promise.resolve('ok'), []);
      await withDiagnosticLogging('create', () => Promise.resolve('ok'), []);
      try {
        await withDiagnosticLogging('delete', () => Promise.reject(new Error('fail')), []);
      } catch {
        // expected
      }

      const stats = getDiagnosticStats();
      expect(stats.totalCalls).toBe(3);
      expect(stats.successCalls).toBe(2);
      expect(stats.errorCalls).toBe(1);
      expect(stats.methodCounts['create']).toBe(2);
      expect(stats.methodCounts['delete']).toBe(1);
      expect(stats.avgDurationMs).toBeGreaterThanOrEqual(0);
    });
  });

  describe('exportLogsAsJson', () => {
    it('should produce valid JSON with stats and entries', async () => {
      await withDiagnosticLogging('test', () => Promise.resolve('result'), []);

      const json = exportLogsAsJson();
      const parsed = JSON.parse(json);

      expect(parsed).toHaveProperty('exportedAt');
      expect(parsed).toHaveProperty('stats');
      expect(parsed).toHaveProperty('entries');
      expect(parsed.stats.totalCalls).toBe(1);
      expect(parsed.entries).toHaveLength(1);
    });
  });

  describe('withDiagnosticLogging', () => {
    it('should log successful calls', async () => {
      const result = await withDiagnosticLogging('myMethod', () => Promise.resolve(42), ['arg1']);

      expect(result).toBe(42);
      const entries = getLogEntries();
      expect(entries).toHaveLength(1);
      expect(entries[0].method).toBe('myMethod');
      expect(entries[0].status).toBe('success');
      expect(entries[0].durationMs).toBeGreaterThanOrEqual(0);
      expect(entries[0].args).toEqual(['arg1']);
    });

    it('should log errors and rethrow', async () => {
      const error = new Error('test error');

      await expect(
        withDiagnosticLogging('failMethod', () => Promise.reject(error), [])
      ).rejects.toThrow('test error');

      const entries = getLogEntries();
      expect(entries).toHaveLength(1);
      expect(entries[0].method).toBe('failMethod');
      expect(entries[0].status).toBe('error');
      expect(entries[0].error).toBe('test error');
    });

    it('should bypass logging when disabled', async () => {
      setDiagnosticLoggingEnabled(false);

      const result = await withDiagnosticLogging('skipped', () => Promise.resolve('ok'), []);

      expect(result).toBe('ok');
      expect(getLogEntries()).toHaveLength(0);
    });

    it('should truncate long string args', async () => {
      const longStr = 'x'.repeat(300);
      await withDiagnosticLogging('truncTest', () => Promise.resolve('ok'), [longStr]);

      const entries = getLogEntries();
      const arg = entries[0].args[0] as string;
      expect(arg.length).toBeLessThan(300);
      expect(arg).toContain('...');
    });

    it('should truncate long array results', async () => {
      const bigArray = Array.from({ length: 20 }, (_, i) => i);
      await withDiagnosticLogging('arrayTest', () => Promise.resolve(bigArray), []);

      const entries = getLogEntries();
      expect(typeof entries[0].result).toBe('string');
      expect((entries[0].result as string)).toContain('Array with 20 items');
    });
  });

  describe('log buffer trimming', () => {
    it('should trim entries when exceeding MAX_LOG_ENTRIES (500)', async () => {
      // Add 510 entries
      for (let i = 0; i < 510; i++) {
        await withDiagnosticLogging(`method-${i}`, () => Promise.resolve(i), []);
      }

      const entries = getLogEntries();
      expect(entries.length).toBeLessThanOrEqual(500);
      // First entries should be trimmed, last ones kept
      expect(entries[entries.length - 1].method).toBe('method-509');
    });
  });

  describe('Tauri-dependent functions in non-Tauri env', () => {
    it('getDatabaseDiagnostics should return null', async () => {
      const result = await getDatabaseDiagnostics();
      expect(result).toBeNull();
    });

    it('testNodePersistence should return null', async () => {
      const result = await testNodePersistence();
      expect(result).toBeNull();
    });
  });
});
