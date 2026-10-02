// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { RuleDefinition } from './rule-definition';

/**
 * Partial update for a play's fields.
 *
 * `rules` is replaced whole and has no clear path (an empty list is how a
 * play has no rules); `description` is tri-state: absent leaves it
 * unchanged, `null` clears it, and a string sets it. `enabled` is the user's
 * switch; writing `true` also clears a suspension. The suspension fields are
 * the engine's, so the update does not carry them.
 */
export type PlayNodeUpdate = {
  rules?: Array<RuleDefinition>;
  description?: string | null;
  enabled?: boolean;
};
