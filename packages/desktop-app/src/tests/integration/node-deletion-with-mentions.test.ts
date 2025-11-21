/**
 * Node Deletion with Mentions Integration Test
 *
 * NOTE: This test suite is SKIPPED because it depends on deleted services:
 * - event-bus (deleted in #558)
 * - event-types (deleted in #558)
 *
 * These tests require the event bus which has been deleted.
 * Integration tests are now handled through the Tauri service layer.
 */

import { describe, it, expect } from 'vitest';

describe.skip('Node Deletion with Mentions (SKIPPED - requires deleted event-bus)', () => {
  it('placeholder test - integration tests need rewrite', () => {
    expect(true).toBe(true);
  });
});
