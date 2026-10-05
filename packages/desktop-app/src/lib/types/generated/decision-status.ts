// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a decision stands (ADR-092 §1). A closed vocabulary: the seeded
 * lock compares against `superseded` by name.
 */
export type DecisionStatus = 'proposed' | 'accepted' | 'superseded';
