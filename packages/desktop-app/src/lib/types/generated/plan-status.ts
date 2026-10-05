// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a plan stands (ADR-092 §1). A closed vocabulary, as for a spec: the
 * seeded rules compare against `approved` and `superseded` by name.
 */
export type PlanStatus = 'draft' | 'approved' | 'superseded';
