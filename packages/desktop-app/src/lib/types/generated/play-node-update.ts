// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { RuleDefinition } from './rule-definition';

/**
 * Partial update for a play's fields.
 *
 * `rules` is replaced whole and has no clear path (an empty list is how a
 * play has no rules); `description` is tri-state: absent leaves it
 * unchanged, `null` clears it, and a string sets it.
 */
export type PlayNodeUpdate = { rules?: Array<RuleDefinition>; description?: string | null };
