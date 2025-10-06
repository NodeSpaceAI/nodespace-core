/**
 * Unified Test Utilities - Central Export
 *
 * Single import point for all test helpers and fixtures.
 * Provides clean, organized access to testing utilities.
 *
 * Usage:
 * ```typescript
 * // Import helpers
 * import { createTestNode, waitForEffects, createEventLogger } from '@tests/helpers';
 *
 * // Import fixtures
 * import { MOCK_TEXT_NODE, MOCK_AUTOCOMPLETE_RESULTS } from '@tests/helpers';
 * ```
 */

// Re-export all test helpers
export * from './test-helpers';

// Re-export all test fixtures
export * from '../fixtures/test-fixtures';
