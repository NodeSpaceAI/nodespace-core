/**
 * NodeType Persistence Tests
 *
 * NOTE: This test suite is SKIPPED because it depends on deleted services:
 * - persistence-coordinator (deleted in #558)
 *
 * These tests require the PersistenceCoordinator which has been deleted.
 * Integration tests are now handled through the Tauri service layer.
 */

import { describe, it, expect } from 'vitest';

describe.skip('NodeType Persistence (SKIPPED - requires deleted persistence-coordinator)', () => {
  it('placeholder test - integration tests need rewrite', () => {
    expect(true).toBe(true);
  });
});
