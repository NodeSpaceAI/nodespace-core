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
 * Timeout for async error propagation
 * Used when testing error handling in async chains
 */
export const ASYNC_ERROR_PROPAGATION_TIMEOUT_MS = 20;

/**
 * Mirrors `SimplePersistenceCoordinator.DEBOUNCE_MS` in
 * `$lib/services/shared-node-store.svelte.ts`, which is private and so cannot be
 * imported. A test that needs a debounced write to have fired waits
 * `PERSISTENCE_DEBOUNCE_MS + DEBOUNCE_SETTLE_MS`, never a hand-picked round
 * number — when the production constant moves, this is the single line to
 * follow it.
 */
export const PERSISTENCE_DEBOUNCE_MS = 500;

/**
 * Margin added to `PERSISTENCE_DEBOUNCE_MS` when a test must wait on real time
 * for the debounced write to land. Covers timer-fire jitter and the microtasks
 * between the timer firing and the mocked backend call being observable — not a
 * guess at how slow the machine might be. Prefer fake timers over this wait
 * wherever the code under test tolerates them.
 */
export const DEBOUNCE_SETTLE_MS = 50;

/**
 * Wait long enough for a debounced persist to have fired and settled.
 * Derived, not guessed: see `PERSISTENCE_DEBOUNCE_MS`.
 */
export const DEBOUNCED_WRITE_WAIT_MS = PERSISTENCE_DEBOUNCE_MS + DEBOUNCE_SETTLE_MS;

/**
 * Mirrors the internal 5s timeout `SimplePersistenceCoordinator.flushPending()`
 * races pending operations against. Tests that assert the timeout fires should
 * advance fake timers by this amount rather than sleeping past it.
 */
export const FLUSH_PENDING_TIMEOUT_MS = 5000;

/**
 * Ceiling on how long a test teardown will wait for an in-flight flush before
 * giving up and resetting the store anyway. This races a real flush, so a
 * healthy test never waits the full duration — it exists so that a test which
 * deliberately leaves an operation hanging cannot stall the suite.
 */
export const TEARDOWN_FLUSH_CEILING_MS = 1000;

/**
 * Upper bound for `vi.waitFor` when waiting on a multi-step persistence cascade
 * (debounced write -> OCC rejection -> fallback resync -> queued follow-up).
 *
 * This is a CEILING, not a delay: vi.waitFor polls and returns as soon as its
 * condition holds, so a passing test costs only as long as the cascade actually
 * takes. It needs to exceed the slowest path, which is why it is generous — a
 * test that hits this value has genuinely hung and should fail.
 */
export const CASCADE_SETTLE_TIMEOUT_MS = 3000;
