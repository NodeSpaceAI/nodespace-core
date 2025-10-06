/**
 * Shared Test Constants
 *
 * Centralized constants used across integration and unit tests
 */

/**
 * Timeout for async event handlers to complete execution
 * Used when testing asynchronous event processing
 */
export const ASYNC_HANDLER_TIMEOUT_MS = 10;

/**
 * Timeout for async error propagation in EventBus
 * Used when testing error handling in event chains
 */
export const ASYNC_ERROR_PROPAGATION_TIMEOUT_MS = 20;
