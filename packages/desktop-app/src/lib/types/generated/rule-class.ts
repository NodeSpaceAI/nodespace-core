// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * How a rule executes (ADR-060).
 *
 * - `Reactive` rules run asynchronously, after the triggering write commits,
 *   on every device that observes the event.
 * - `Invariant` rules run synchronously inside the triggering transaction,
 *   fail-closed, on the originating device only. Save-time validation holds
 *   them to the eligibility rules of ADR-060 §2.
 */
export type RuleClass = 'invariant' | 'reactive';
