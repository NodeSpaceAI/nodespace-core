// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * One condition of a rule: a CEL expression over the triggering node, and
 * what it requires in the author's words. A bare expression does not decode.
 */
export type RuleCondition = { expr: string; description: string };
